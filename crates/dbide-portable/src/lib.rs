//! Explicit portable configuration, without credentials, SQL buffers, or database data.
//!
//! Restoration creates a new directory and activates nothing. Files are published
//! independently, with `portable-complete.json` last; this is not a multi-file
//! filesystem transaction. Callers may open the new configuration after success.
use dbide_profiles::{ConnectionProfile, DatabaseEngine, ProfileStore, TlsMode, WorkspaceState};
use dbide_settings::{AppSettings, Locale, SettingsStore};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fmt, fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_BUNDLE_BYTES: usize = 2 * 1024 * 1024;
const FORMAT: &str = "rust-database-ide";
const KEYCHAIN_NOTICE: &str = "Profile IDs are preserved. Passwords are excluded; an explicit Keychain load on the same computer can match an existing profile ID.";
const DATA_NOTICE: &str = "This is configuration only. Database files, CA certificate files, query history, SQL buffers, results, project paths, and credentials are not copied.";

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Malformed,
    UnsupportedVersion,
    TooLarge,
    Invalid(&'static str),
    NotRegularFile,
    DestinationExists,
    RelativeRootRequired,
    ForeignPathRequiresRemap,
    Store(String),
}
impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::Malformed => "malformed",
            Self::UnsupportedVersion => "unsupported_version",
            Self::TooLarge => "too_large",
            Self::Invalid(_) => "invalid",
            Self::NotRegularFile => "not_regular_file",
            Self::DestinationExists => "destination_exists",
            Self::RelativeRootRequired => "relative_root_required",
            Self::ForeignPathRequiresRemap => "foreign_path_requires_remap",
            Self::Store(_) => "store",
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "Portable configuration I/O failed: {e}"),
            Self::Malformed => f.write_str("Portable configuration is malformed or contains unsupported fields"),
            Self::UnsupportedVersion => f.write_str("Portable configuration schema version is unsupported; original preserved"),
            Self::TooLarge => f.write_str("Portable configuration exceeds its size limit"),
            Self::Invalid(reason) => write!(f, "Portable configuration is invalid: {reason}"),
            Self::NotRegularFile => f.write_str("Portable configuration paths must be regular files, not symlinks or special files"),
            Self::DestinationExists => f.write_str("Destination already exists; nothing was overwritten"),
            Self::RelativeRootRequired => f.write_str("Relative paths require an explicitly selected absolute remap root"),
            Self::ForeignPathRequiresRemap => f.write_str("A path belongs to another operating system; provide an explicit external path mapping"),
            Self::Store(e) => write!(f, "Portable configuration store failed: {e}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PathPlatform {
    Unix,
    Windows,
}
impl PathPlatform {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortablePath {
    Relative {
        components: Vec<String>,
    },
    Absolute {
        platform: PathPlatform,
        value: String,
    },
    Memory,
}
impl PortablePath {
    fn validate(&self, allow_memory: bool) -> Result<(), Error> {
        match self {
            Self::Relative { components } => {
                if components.is_empty()
                    || components.len() > 256
                    || components.iter().any(|c| !valid_component(c))
                    || components.iter().map(|c| c.len() + 1).sum::<usize>() > 4096
                {
                    return Err(Error::Invalid("invalid relative path components"));
                }
            }
            Self::Absolute { platform, value } => {
                if absolute_platform(value) != Some(*platform)
                    || !valid_path_text(value)
                    || path_segments(value).any(|p| p == "." || p == "..")
                {
                    return Err(Error::Invalid("invalid absolute path descriptor"));
                }
            }
            Self::Memory if !allow_memory => {
                return Err(Error::Invalid("memory path is only valid for SQLite"));
            }
            Self::Memory => {}
        }
        Ok(())
    }
    fn original_text(&self) -> String {
        match self {
            Self::Relative { components } => components.join("/"),
            Self::Absolute { value, .. } => value.clone(),
            Self::Memory => ":memory:".into(),
        }
    }
    fn capture(value: &str, base: &Path, allow_memory: bool) -> Result<Self, Error> {
        if value == ":memory:" && allow_memory {
            return Ok(Self::Memory);
        }
        if !valid_path_text(value) {
            return Err(Error::Invalid("invalid path text"));
        }
        let result = if let Some(platform) = absolute_platform(value) {
            if platform == PathPlatform::current() {
                if let Ok(relative) = Path::new(value).strip_prefix(base) {
                    let components = native_components(relative)?;
                    Self::Relative { components }
                } else {
                    Self::Absolute {
                        platform,
                        value: value.into(),
                    }
                }
            } else {
                Self::Absolute {
                    platform,
                    value: value.into(),
                }
            }
        } else {
            // Parse on the source platform. On Unix a backslash is part of a
            // filename, not a separator: reject that nonportable component
            // rather than silently pointing the restored profile elsewhere.
            // Check raw segments before Path::components normalizes `.` away.
            if path_segments(value)
                .any(|segment| segment.is_empty() || segment == "." || segment == "..")
            {
                return Err(Error::Invalid(
                    "relative path contains empty components or traversal",
                ));
            }
            Self::Relative {
                components: native_components(Path::new(value))?,
            }
        };
        result.validate(allow_memory)?;
        Ok(result)
    }
    fn resolve(
        &self,
        root: Option<&Path>,
        mappings: &[ExternalPathMapping],
    ) -> Result<String, Error> {
        let path = match self {
            Self::Memory => return Ok(":memory:".into()),
            Self::Relative { components } => {
                let mut path = root.ok_or(Error::RelativeRootRequired)?.to_path_buf();
                for component in components {
                    path.push(component);
                }
                path
            }
            Self::Absolute { platform, value } => {
                if let Some(mapping) = mappings.iter().find(|m| m.original == *value) {
                    mapping.destination.clone()
                } else if *platform != PathPlatform::current() {
                    return Err(Error::ForeignPathRequiresRemap);
                } else {
                    PathBuf::from(value)
                }
            }
        };
        validate_absolute_root(&path)?;
        path.into_os_string()
            .into_string()
            .map_err(|_| Error::Invalid("paths must be UTF-8"))
    }
}
fn valid_path_text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}
fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.len() <= 4096
        && !value.contains(['/', '\\', ':'])
        && !value.chars().any(char::is_control)
        && value.trim() == value
}
fn path_segments(value: &str) -> impl Iterator<Item = &str> {
    value.split(['/', '\\'])
}
fn absolute_platform(value: &str) -> Option<PathPlatform> {
    let bytes = value.as_bytes();
    if (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\'))
        || value.starts_with("\\\\")
    {
        Some(PathPlatform::Windows)
    } else if value.starts_with('/') {
        Some(PathPlatform::Unix)
    } else {
        None
    }
}
fn native_components(path: &Path) -> Result<Vec<String>, Error> {
    path.components()
        .map(|c| match c {
            Component::Normal(c) => c
                .to_str()
                .filter(|s| valid_component(s))
                .map(str::to_owned)
                .ok_or(Error::Invalid("invalid relative path component")),
            _ => Err(Error::Invalid("relative path contains a root or traversal")),
        })
        .collect()
}
fn validate_absolute_root(root: &Path) -> Result<(), Error> {
    let text = root.to_str().ok_or(Error::Invalid("paths must be UTF-8"))?;
    if !root.is_absolute()
        || absolute_platform(text) != Some(PathPlatform::current())
        || !valid_path_text(text)
        || path_segments(text).any(|c| c == "." || c == "..")
    {
        return Err(Error::Invalid(
            "explicit base and destination paths must be absolute without traversal",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PortableProfile {
    pub id: String,
    pub name: String,
    pub engine: DatabaseEngine,
    pub sqlite_path: Option<PortablePath>,
    pub sqlite_create: bool,
    pub read_only: bool,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub tls_mode: TlsMode,
    pub root_cert_path: Option<PortablePath>,
    pub connect_timeout_secs: u64,
}
impl PortableProfile {
    fn capture(p: &ConnectionProfile, base: &Path) -> Result<Self, Error> {
        Ok(Self {
            id: p.id.clone(),
            name: p.name.clone(),
            engine: p.engine,
            sqlite_path: p
                .sqlite_path
                .as_deref()
                .map(|v| PortablePath::capture(v, base, p.engine == DatabaseEngine::Sqlite))
                .transpose()?,
            sqlite_create: p.sqlite_create,
            read_only: p.read_only,
            host: p.host.clone(),
            port: p.port,
            database: p.database.clone(),
            username: p.username.clone(),
            tls_mode: p.tls_mode,
            root_cert_path: p
                .root_cert_path
                .as_deref()
                .map(|v| PortablePath::capture(v, base, false))
                .transpose()?,
            connect_timeout_secs: p.connect_timeout_secs,
        })
    }
    fn as_profile(
        &self,
        sqlite_path: Option<String>,
        root_cert_path: Option<String>,
    ) -> ConnectionProfile {
        ConnectionProfile {
            id: self.id.clone(),
            name: self.name.clone(),
            engine: self.engine,
            sqlite_path,
            sqlite_create: self.sqlite_create,
            read_only: self.read_only,
            host: self.host.clone(),
            port: self.port,
            database: self.database.clone(),
            username: self.username.clone(),
            tls_mode: self.tls_mode,
            root_cert_path,
            connect_timeout_secs: self.connect_timeout_secs,
        }
    }
    fn paths(&self) -> impl Iterator<Item = &PortablePath> {
        self.sqlite_path.iter().chain(&self.root_cert_path)
    }
}

// AppSettings itself intentionally accepts future keys/defaults in its local
// store. Portable imports require a complete whitelist instead.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictSettings {
    locale: Locale,
    editor_font_size: u16,
    editor_tab_width: u8,
    table_page_size: u16,
    sidebar_width: u16,
    editor_height: u16,
}
fn strict_settings<'de, D: serde::Deserializer<'de>>(d: D) -> Result<AppSettings, D::Error> {
    let s = StrictSettings::deserialize(d)?;
    Ok(AppSettings {
        locale: s.locale,
        editor_font_size: s.editor_font_size,
        editor_tab_width: s.editor_tab_width,
        table_page_size: s.table_page_size,
        sidebar_width: s.sidebar_width,
        editor_height: s.editor_height,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub format: String,
    pub schema_version: u32,
    pub profiles_schema_version: u32,
    pub settings_schema_version: u32,
    pub profiles: Vec<PortableProfile>,
    pub selected_profile_id: Option<String>,
    #[serde(deserialize_with = "strict_settings")]
    pub app_settings: AppSettings,
    pub recovery_enabled: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct Inspection {
    pub profile_count: usize,
    pub selected_profile_id: Option<String>,
    pub app_settings: AppSettings,
    pub recovery_enabled: bool,
    pub relative_path_count: usize,
    pub external_path_count: usize,
    pub warnings: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct RestorePlan {
    pub state: WorkspaceState,
    pub app_settings: AppSettings,
    pub recovery_enabled: bool,
    pub warnings: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct RestoreReport {
    pub directory: PathBuf,
    pub profile_path: PathBuf,
    pub profile_count: usize,
    pub warnings: Vec<String>,
}
/// An exact external path replacement, never a guessed basename conversion.
#[derive(Clone, Debug)]
pub struct ExternalPathMapping {
    pub original: String,
    pub destination: PathBuf,
}

impl Bundle {
    pub fn capture(
        state: &WorkspaceState,
        settings: AppSettings,
        recovery_enabled: bool,
        base: &Path,
    ) -> Result<Self, Error> {
        if state.schema_version != dbide_profiles::SCHEMA_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        if state.profiles.len() > dbide_profiles::MAX_PROFILES {
            return Err(Error::Invalid("profile count exceeds limit"));
        }
        validate_absolute_root(base)?;
        let bundle = Self {
            format: FORMAT.into(),
            schema_version: SCHEMA_VERSION,
            profiles_schema_version: dbide_profiles::SCHEMA_VERSION,
            settings_schema_version: dbide_settings::SCHEMA_VERSION,
            profiles: state
                .profiles
                .iter()
                .map(|p| PortableProfile::capture(p, base))
                .collect::<Result<_, _>>()?,
            selected_profile_id: state.selected_profile_id.clone(),
            app_settings: settings,
            recovery_enabled,
        };
        bundle.validate()?;
        bundle.encoded()?;
        Ok(bundle)
    }
    pub fn read(path: &Path) -> Result<Self, Error> {
        let bytes = read_regular(path, MAX_BUNDLE_BYTES)?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| Error::Malformed)?;
        // Identify future versions before decoding a strict current schema.
        for (key, supported) in [
            ("schema_version", SCHEMA_VERSION),
            ("profiles_schema_version", dbide_profiles::SCHEMA_VERSION),
            ("settings_schema_version", dbide_settings::SCHEMA_VERSION),
        ] {
            match value.get(key).and_then(serde_json::Value::as_u64) {
                Some(v) if v == u64::from(supported) => {}
                Some(_) => return Err(Error::UnsupportedVersion),
                None => return Err(Error::Malformed),
            }
        }
        // Decode original bytes to reject duplicate fields as well as unknowns.
        let bundle: Self = serde_json::from_slice(&bytes).map_err(|_| Error::Malformed)?;
        bundle.validate()?;
        Ok(bundle)
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.schema_version != SCHEMA_VERSION
            || self.profiles_schema_version != dbide_profiles::SCHEMA_VERSION
            || self.settings_schema_version != dbide_settings::SCHEMA_VERSION
        {
            return Err(Error::UnsupportedVersion);
        }
        if self.format != FORMAT {
            return Err(Error::Invalid("unrecognized format"));
        }
        if !self.app_settings.is_valid() {
            return Err(Error::Invalid("invalid application settings"));
        }
        let mut state = WorkspaceState {
            selected_profile_id: self.selected_profile_id.clone(),
            ..Default::default()
        };
        for p in &self.profiles {
            if let Some(path) = &p.sqlite_path {
                path.validate(p.engine == DatabaseEngine::Sqlite)?;
            }
            if let Some(path) = &p.root_cert_path {
                path.validate(false)?;
            }
            state.profiles.push(p.as_profile(
                p.sqlite_path.as_ref().map(PortablePath::original_text),
                p.root_cert_path.as_ref().map(PortablePath::original_text),
            ));
        }
        state
            .validate()
            .map_err(|_| Error::Invalid("invalid connection profiles, IDs, or selection"))?;
        if serde_json::to_vec(&state)
            .map_err(|_| Error::Malformed)?
            .len()
            > dbide_profiles::MAX_FILE_BYTES
        {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
    fn encoded(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| Error::Malformed)?;
        if bytes.len() > MAX_BUNDLE_BYTES {
            return Err(Error::TooLarge);
        }
        Ok(bytes)
    }
    /// Creates a new JSON file only. Existing files/directories/symlinks survive.
    pub fn write_new(&self, path: &Path) -> Result<(), Error> {
        write_new_file(path, &self.encoded()?)
    }
    pub fn inspect(&self) -> Inspection {
        let paths: Vec<_> = self
            .profiles
            .iter()
            .flat_map(PortableProfile::paths)
            .collect();
        let relative_path_count = paths
            .iter()
            .filter(|p| matches!(p, PortablePath::Relative { .. }))
            .count();
        let external_path_count = paths
            .iter()
            .filter(|p| matches!(p, PortablePath::Absolute { .. }))
            .count();
        let mut warnings = vec![DATA_NOTICE.into(), KEYCHAIN_NOTICE.into()];
        if relative_path_count != 0 {
            warnings.push("Relative file references need an explicit remap root on import.".into());
        }
        if external_path_count != 0 {
            warnings.push("External absolute file references are retained only on a compatible operating system; verify their locations or map them explicitly.".into());
        }
        Inspection {
            profile_count: self.profiles.len(),
            selected_profile_id: self.selected_profile_id.clone(),
            app_settings: self.app_settings,
            recovery_enabled: self.recovery_enabled,
            relative_path_count,
            external_path_count,
            warnings,
        }
    }
    pub fn plan(&self, remap_root: Option<&Path>) -> Result<RestorePlan, Error> {
        self.plan_with_mappings(remap_root, &[])
    }
    pub fn plan_with_mappings(
        &self,
        remap_root: Option<&Path>,
        mappings: &[ExternalPathMapping],
    ) -> Result<RestorePlan, Error> {
        self.validate()?;
        if let Some(root) = remap_root {
            validate_absolute_root(root)?;
        }
        let external_values: HashSet<_> = self
            .profiles
            .iter()
            .flat_map(PortableProfile::paths)
            .filter_map(|p| match p {
                PortablePath::Absolute { value, .. } => Some(value.as_str()),
                _ => None,
            })
            .collect();
        let mut seen = HashSet::new();
        for mapping in mappings {
            if !external_values.contains(mapping.original.as_str())
                || !seen.insert(mapping.original.as_str())
            {
                return Err(Error::Invalid("unknown or duplicate external path mapping"));
            }
            validate_absolute_root(&mapping.destination)?;
        }
        let mut state = WorkspaceState {
            selected_profile_id: self.selected_profile_id.clone(),
            ..Default::default()
        };
        let mut warnings = self.inspect().warnings;
        for p in &self.profiles {
            let sqlite = p
                .sqlite_path
                .as_ref()
                .map(|v| v.resolve(remap_root, mappings))
                .transpose()?;
            let cert = p
                .root_cert_path
                .as_ref()
                .map(|v| v.resolve(remap_root, mappings))
                .transpose()?;
            for (field, path) in [
                ("SQLite", sqlite.as_ref()),
                ("CA certificate", cert.as_ref()),
            ] {
                if let Some(path) = path.filter(|s| *s != ":memory:") {
                    match fs::symlink_metadata(path) {
                        Ok(meta) if meta.file_type().is_file() => {}
                        Ok(_) => warnings.push(format!("Profile {}: {field} reference is not a regular file; review before connecting.", p.id)),
                        Err(e) if e.kind() == io::ErrorKind::NotFound => warnings.push(format!("Profile {}: {field} file is missing; move the file separately or edit the path before connecting.", p.id)),
                        Err(_) => warnings.push(format!("Profile {}: {field} path could not be inspected; review before connecting.", p.id)),
                    }
                }
            }
            state.profiles.push(p.as_profile(sqlite, cert));
        }
        state
            .validate()
            .map_err(|_| Error::Invalid("resolved profiles are invalid"))?;
        // Include whitespace overhead used by ProfileStore's pretty encoding.
        if serde_json::to_vec_pretty(&state)
            .map_err(|_| Error::Malformed)?
            .len()
            > dbide_profiles::MAX_FILE_BYTES
        {
            return Err(Error::TooLarge);
        }
        Ok(RestorePlan {
            state,
            app_settings: self.app_settings,
            recovery_enabled: self.recovery_enabled,
            warnings,
        })
    }
    pub fn restore_new_directory(
        &self,
        destination: &Path,
        remap_root: Option<&Path>,
    ) -> Result<RestoreReport, Error> {
        self.restore_new_directory_with_mappings(destination, remap_root, &[])
    }
    pub fn restore_new_directory_with_mappings(
        &self,
        destination: &Path,
        remap_root: Option<&Path>,
        mappings: &[ExternalPathMapping],
    ) -> Result<RestoreReport, Error> {
        // Complete all validation/path planning before any destination creation.
        let plan = self.plan_with_mappings(remap_root, mappings)?;
        validate_absolute_root(destination)?;
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(destination).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                Error::DestinationExists
            } else {
                Error::Io(e)
            }
        })?;
        let owned_directory = match fs::symlink_metadata(destination) {
            Ok(metadata) => metadata,
            Err(error) => {
                let _ = fs::remove_dir(destination);
                return Err(error.into());
            }
        };
        let profile_path = destination.join("profiles.json");
        let result = (|| {
            ProfileStore::open(&profile_path)
                .and_then(|s| s.save(&plan.state))
                .map_err(|e| Error::Store(e.to_string()))?;
            SettingsStore::for_profile_path(&profile_path)
                .save(&plan.app_settings)
                .map_err(|e| Error::Store(e.to_string()))?;
            write_new_file(
                &destination.join("editor-preferences.json"),
                &serde_json::to_vec_pretty(&RecoveryPreference {
                    enabled: plan.recovery_enabled,
                })
                .map_err(|_| Error::Malformed)?,
            )?;
            write_new_file(
                &destination.join("portable-complete.json"),
                br#"{"format":"rust-database-ide","schema_version":1,"complete":true}"#,
            )?;
            #[cfg(unix)]
            fs::File::open(destination)?.sync_all()?;
            Ok(RestoreReport {
                directory: destination.to_path_buf(),
                profile_path,
                profile_count: plan.state.profiles.len(),
                warnings: plan.warnings,
            })
        })();
        if result.is_err() {
            cleanup_owned_directory(destination, &owned_directory);
        }
        result
    }
}
pub fn read(path: &Path) -> Result<Bundle, Error> {
    Bundle::read(path)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryPreference {
    enabled: bool,
}
/// Shared parser for the existing GUI's opt-in recovery preference. Never reads
/// workspace.json or any SQL buffers. A missing preference means disabled.
pub fn read_recovery_enabled(profile_path: &Path) -> Result<bool, Error> {
    match read_regular(
        &profile_path.with_file_name("editor-preferences.json"),
        4096,
    ) {
        Ok(bytes) => serde_json::from_slice::<RecoveryPreference>(&bytes)
            .map(|p| p.enabled)
            .map_err(|_| Error::Malformed),
        Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}
fn reject_non_regular(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => Err(Error::NotRegularFile),
        Ok(_) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
fn read_regular(path: &Path, limit: usize) -> Result<Vec<u8>, Error> {
    reject_non_regular(path)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(Error::NotRegularFile);
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Error::TooLarge);
    }
    Ok(bytes)
}
fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            Error::DestinationExists
        } else {
            Error::Io(e)
        }
    })?;
    if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(e.into());
    }
    Ok(())
}
fn cleanup_owned_directory(path: &Path, original: &fs::Metadata) {
    let Ok(current) = fs::symlink_metadata(path) else {
        return;
    };
    if !current.file_type().is_dir() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if original.dev() != current.dev() || original.ino() != current.ino() {
            return;
        }
    }
    #[cfg(not(unix))]
    let _ = original;
    // Only known files created by this operation; never recursively remove any
    // directory another actor might have added while restoration ran.
    for name in [
        "profiles.json",
        "profiles.json.lock",
        "app-settings.json",
        "app-settings.json.lock",
        "editor-preferences.json",
        "portable-complete.json",
    ] {
        let _ = fs::remove_file(path.join(name));
    }
    let _ = fs::remove_dir(path);
}

#[cfg(test)]
mod tests;
