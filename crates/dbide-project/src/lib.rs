//! Explicit folder projects shared by GUI and CLI. No database or profile discovery.
//!
//! Cooperative writers serialize on `.dbide/project.lock`; SQL saves additionally
//! compare the digest of the bytes last loaded. Manifest revisions cover manifest
//! changes, while SQL documents have independent content revisions. These checks
//! are not a sandbox against malicious concurrent renames of parent directories
//! or writers that ignore the lock.

use dbide_profiles::StoredExecutionTarget;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

/// Persisted/guarded targets must not resolve a SQLite file against an unrelated cwd.
pub fn validate_target_profile(
    profile: &dbide_profiles::ConnectionProfile,
) -> Result<(), ProjectError> {
    profile
        .validate()
        .map_err(|_| ProjectError::InvalidTarget)?;
    if profile.engine == dbide_profiles::DatabaseEngine::Sqlite
        && profile
            .sqlite_path
            .as_deref()
            .is_none_or(|p| p != ":memory:" && !Path::new(p).is_absolute())
    {
        return Err(ProjectError::InvalidTarget);
    }
    Ok(())
}

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_SQL_BYTES: usize = 64 * 1024;
pub const MAX_LIST_ENTRIES: usize = 512;
pub const MAX_SCANNED_ENTRIES: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub name: String,
    pub revision: u64,
    pub default_target: Option<StoredExecutionTarget>,
}

impl Manifest {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ProjectError::UnsupportedVersion(self.schema_version.into()));
        }
        validate_name(&self.name)?;
        if let Some(target) = &self.default_target {
            target.validate().map_err(|_| ProjectError::InvalidTarget)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Directory,
    SqlFile,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ProjectEntry {
    pub relative_path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct DirectoryListing {
    pub relative_path: PathBuf,
    pub entries: Vec<ProjectEntry>,
    /// A bounded scan or returned-entry limit was reached. Expand another folder
    /// or narrow the directory externally; this is never a complete-tree claim.
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SqlFile {
    pub relative_path: PathBuf,
    pub absolute_path: PathBuf,
    pub sql: String,
    pub sha256: String,
}

#[derive(Debug)]
pub enum ProjectError {
    Conflict,
    NotFound,
    AlreadyExists,
    InvalidPath,
    UnsupportedVersion(u64),
    TooLarge,
    Busy,
    InvalidManifest,
    InvalidTarget,
    InvalidUtf8,
    Io(io::Error),
}

impl ProjectError {
    /// Stable machine-readable classification. Display text is for people.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Conflict => "conflict",
            Self::NotFound => "not_found",
            Self::AlreadyExists => "already_exists",
            Self::InvalidPath => "invalid_path",
            Self::UnsupportedVersion(_) => "unsupported_version",
            Self::TooLarge => "too_large",
            Self::Busy => "busy",
            Self::InvalidManifest => "invalid_manifest",
            Self::InvalidTarget => "invalid_target",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::Io(_) => "io",
        }
    }
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict => f.write_str("File or manifest changed; reload before saving"),
            Self::NotFound => f.write_str("Project or requested entry does not exist"),
            Self::AlreadyExists => f.write_str("Entry already exists; original was preserved"),
            Self::InvalidPath => f.write_str("Use a project-relative regular file or directory without hidden components or symlinks"),
            Self::UnsupportedVersion(version) => write!(f, "Project schema version {version} is unsupported; original was preserved"),
            Self::TooLarge => f.write_str("Project metadata or SQL exceeds the 64 KiB limit"),
            Self::Busy => f.write_str("Project is busy; retry after the current writer finishes"),
            Self::InvalidManifest => f.write_str("Project manifest is invalid; original was preserved"),
            Self::InvalidTarget => f.write_str("Default target is invalid; explicitly select a connection and database"),
            Self::InvalidUtf8 => f.write_str("SQL file must contain UTF-8 text"),
            Self::Io(error) => write!(f, "Project I/O failed: {error}"),
        }
    }
}

impl std::error::Error for ProjectError {}

impl From<io::Error> for ProjectError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::AlreadyExists => Self::AlreadyExists,
            _ => Self::Io(error),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    root: PathBuf,
}

impl Project {
    /// Initializes an explicitly selected existing directory, preserving its
    /// contents. Existing manifests are never reinitialized or overwritten.
    pub fn init(root: &Path, name: &str) -> Result<Self, ProjectError> {
        validate_name(name)?;
        let project = Self::from_root(root)?;
        let metadata = project.root.join(".dbide");
        create_directory(&metadata, true)?;
        let _lock = project.lock_writer()?;
        let manifest_path = metadata.join("project.json");
        match fs::symlink_metadata(&manifest_path) {
            Ok(_) => return Err(ProjectError::AlreadyExists),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        create_directory(&project.root.join("queries"), true)?;
        let manifest = Manifest {
            schema_version: SCHEMA_VERSION,
            name: name.into(),
            revision: 0,
            default_target: None,
        };
        project.publish_manifest(&manifest, false)?;
        Ok(project)
    }

    /// Validates existing metadata. Never creates a directory, lock, or file.
    pub fn open(root: &Path) -> Result<Self, ProjectError> {
        let project = Self::from_root(root)?;
        project.manifest()?;
        Ok(project)
    }

    fn from_root(root: &Path) -> Result<Self, ProjectError> {
        let root = fs::canonicalize(root)?;
        require_directory(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> Result<Manifest, ProjectError> {
        let metadata = self.metadata_directory()?;
        let bytes = read_regular(&metadata.join("project.json"), MAX_MANIFEST_BYTES)?;
        // Inspect version before the strict current-schema decode so newer
        // schemas remain distinguishable from malformed current metadata.
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| ProjectError::InvalidManifest)?;
        let version = value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or(ProjectError::InvalidManifest)?;
        if version != u64::from(SCHEMA_VERSION) {
            return Err(ProjectError::UnsupportedVersion(version));
        }
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|_| ProjectError::InvalidManifest)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Stores only password-free target identity. Missing profiles are allowed:
    /// resolving and explicitly connecting belong to the execution layer.
    pub fn set_default_target(
        &self,
        target: Option<StoredExecutionTarget>,
        expected_revision: u64,
    ) -> Result<Manifest, ProjectError> {
        if let Some(target) = &target {
            target.validate().map_err(|_| ProjectError::InvalidTarget)?;
        }
        let _lock = self.lock_writer()?;
        let mut manifest = self.manifest()?;
        if manifest.revision != expected_revision {
            return Err(ProjectError::Conflict);
        }
        manifest.revision = manifest
            .revision
            .checked_add(1)
            .ok_or(ProjectError::Conflict)?;
        manifest.default_target = target;
        self.publish_manifest(&manifest, true)?;
        Ok(manifest)
    }

    /// One directory only. Hidden entries, links, special files, and non-SQL
    /// files are omitted; neither nested folders nor SQL contents are loaded.
    pub fn list(&self, relative: &Path) -> Result<DirectoryListing, ProjectError> {
        let relative = validate_relative(relative, true)?;
        let absolute = self.checked_path(&relative, true)?;
        require_directory(&absolute)?;
        let mut entries = Vec::new();
        let mut truncated = false;
        let mut iterator = fs::read_dir(&absolute)?;
        for scanned in 0..MAX_SCANNED_ENTRIES {
            let Some(entry) = iterator.next() else {
                break;
            };
            let entry = entry?;
            if scanned + 1 == MAX_SCANNED_ENTRIES {
                truncated = true;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with('.') || name.chars().any(char::is_control) || name.contains('\\') {
                continue;
            }
            let file_type = entry.file_type()?;
            let kind = if file_type.is_dir() {
                EntryKind::Directory
            } else if file_type.is_file() && is_sql(Path::new(name)) {
                EntryKind::SqlFile
            } else {
                continue;
            };
            entries.push(ProjectEntry {
                relative_path: relative.join(name),
                name: name.into(),
                kind,
            });
        }
        entries.sort_by(|a, b| {
            let rank = |entry: &ProjectEntry| usize::from(entry.kind == EntryKind::SqlFile);
            rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name))
        });
        if entries.len() > MAX_LIST_ENTRIES {
            entries.truncate(MAX_LIST_ENTRIES);
            truncated = true;
        }
        Ok(DirectoryListing {
            relative_path: relative,
            entries,
            truncated,
        })
    }

    pub fn read_sql(&self, relative: &Path) -> Result<SqlFile, ProjectError> {
        let relative = validate_sql_path(relative)?;
        let absolute = self.checked_path(&relative, false)?;
        let bytes = read_regular(&absolute, MAX_SQL_BYTES)?;
        let sql = String::from_utf8(bytes).map_err(|_| ProjectError::InvalidUtf8)?;
        Ok(SqlFile {
            relative_path: relative,
            absolute_path: absolute,
            sha256: digest_sql(&sql),
            sql,
        })
    }

    /// `None` creates a new entry exclusively; `Some(hash)` replaces only the
    /// exact UTF-8 bytes previously loaded. Empty SQL is a valid unsent draft.
    pub fn write_sql(
        &self,
        relative: &Path,
        sql: &str,
        expected: Option<&str>,
    ) -> Result<SqlFile, ProjectError> {
        if sql.len() > MAX_SQL_BYTES {
            return Err(ProjectError::TooLarge);
        }
        let relative = validate_sql_path(relative)?;
        let _lock = self.lock_writer()?;
        // Also preserve a manifest that became corrupt/newer after opening.
        self.manifest()?;
        let absolute = self.checked_path(&relative, false)?;
        if let Some(expected) = expected {
            let current = self.read_sql(&relative).map_err(|error| {
                if matches!(error, ProjectError::NotFound) {
                    ProjectError::Conflict
                } else {
                    error
                }
            })?;
            if current.sha256 != expected {
                return Err(ProjectError::Conflict);
            }
        } else {
            match fs::symlink_metadata(&absolute) {
                Ok(_) => return Err(ProjectError::AlreadyExists),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        publish(&absolute, sql.as_bytes(), expected.is_some())?;
        Ok(SqlFile {
            relative_path: relative,
            absolute_path: absolute,
            sha256: digest_sql(sql),
            sql: sql.into(),
        })
    }

    /// Creates exactly one directory. Parents must exist; existing entries fail.
    pub fn mkdir(&self, relative: &Path) -> Result<PathBuf, ProjectError> {
        let relative = validate_relative(relative, false)?;
        let _lock = self.lock_writer()?;
        self.manifest()?;
        let absolute = self.checked_path(&relative, false)?;
        create_directory(&absolute, false)?;
        Ok(relative)
    }

    fn metadata_directory(&self) -> Result<PathBuf, ProjectError> {
        require_directory(&self.root)?;
        let path = self.root.join(".dbide");
        require_directory(&path)?;
        Ok(path)
    }

    fn checked_path(&self, relative: &Path, include_leaf: bool) -> Result<PathBuf, ProjectError> {
        require_directory(&self.root)?;
        let mut path = self.root.clone();
        let components: Vec<_> = relative.components().collect();
        for (index, component) in components.iter().enumerate() {
            path.push(component.as_os_str());
            if include_leaf || index + 1 < components.len() {
                require_directory(&path)?;
            }
        }
        Ok(path)
    }

    fn lock_writer(&self) -> Result<fs::File, ProjectError> {
        let path = self.metadata_directory()?.join("project.lock");
        reject_existing_nonregular(&path)?;
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        secure_open(&mut options);
        let lock = options.open(&path)?;
        require_regular(&lock)?;
        lock.try_lock().map_err(|error| match error {
            fs::TryLockError::WouldBlock => ProjectError::Busy,
            fs::TryLockError::Error(error) => ProjectError::from(error),
        })?;
        Ok(lock)
    }

    fn publish_manifest(&self, manifest: &Manifest, replace: bool) -> Result<(), ProjectError> {
        manifest.validate()?;
        let bytes =
            serde_json::to_vec_pretty(manifest).map_err(|_| ProjectError::InvalidManifest)?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ProjectError::TooLarge);
        }
        publish(
            &self.metadata_directory()?.join("project.json"),
            &bytes,
            replace,
        )
    }
}

pub fn digest_sql(sql: &str) -> String {
    format!("{:x}", Sha256::digest(sql.as_bytes()))
}

fn validate_name(name: &str) -> Result<(), ProjectError> {
    if name.trim().is_empty()
        || name != name.trim()
        || name.len() > 128
        || name.chars().any(char::is_control)
    {
        return Err(ProjectError::InvalidManifest);
    }
    Ok(())
}

fn validate_relative(relative: &Path, allow_root: bool) -> Result<PathBuf, ProjectError> {
    if relative.as_os_str().is_empty() && allow_root {
        return Ok(PathBuf::new());
    }
    let mut normalized = PathBuf::new();
    // A single dot is accepted to list the root. Parent components must never
    // be normalized away before validation.
    if relative == Path::new(".") && allow_root {
        return Ok(normalized);
    }
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(ProjectError::InvalidPath);
        };
        let name = name.to_str().ok_or(ProjectError::InvalidPath)?;
        if name.starts_with('.') || name.chars().any(char::is_control) || name.contains('\\') {
            return Err(ProjectError::InvalidPath);
        }
        normalized.push(name);
    }
    if normalized.as_os_str().is_empty() && !allow_root {
        return Err(ProjectError::InvalidPath);
    }
    Ok(normalized)
}

fn is_sql(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sql"))
}

fn validate_sql_path(relative: &Path) -> Result<PathBuf, ProjectError> {
    let relative = validate_relative(relative, false)?;
    if !is_sql(&relative) {
        return Err(ProjectError::InvalidPath);
    }
    Ok(relative)
}

fn require_directory(path: &Path) -> Result<(), ProjectError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(ProjectError::InvalidPath);
    }
    Ok(())
}

fn create_directory(path: &Path, allow_existing: bool) -> Result<(), ProjectError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if allow_existing && error.kind() == io::ErrorKind::AlreadyExists => {
            require_directory(path)
        }
        Err(error) => Err(error.into()),
    }
}

fn reject_existing_nonregular(path: &Path) -> Result<(), ProjectError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(ProjectError::InvalidPath),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn require_regular(file: &fs::File) -> Result<(), ProjectError> {
    if !file.metadata()?.file_type().is_file() {
        return Err(ProjectError::InvalidPath);
    }
    Ok(())
}

fn secure_open(options: &mut fs::OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    let _ = options;
}

fn read_regular(path: &Path, limit: usize) -> Result<Vec<u8>, ProjectError> {
    reject_existing_nonregular(path)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    secure_open(&mut options);
    let file = options.open(path)?;
    require_regular(&file)?;
    if file.metadata()?.len() > limit as u64 {
        return Err(ProjectError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(ProjectError::TooLarge);
    }
    Ok(bytes)
}

fn publish(path: &Path, bytes: &[u8], replace: bool) -> Result<(), ProjectError> {
    let parent = path.parent().ok_or(ProjectError::InvalidPath)?;
    require_directory(parent)?;
    reject_existing_nonregular(path)?;
    // Same-directory temporary file is private (0600 on Unix). Publish only a
    // complete flushed document; drop cleans failed temporary writes.
    let mut temporary = tempfile::Builder::new()
        .prefix(".dbide-")
        .tempfile_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    if replace {
        temporary
            .persist(path)
            .map_err(|error| ProjectError::from(error.error))?;
    } else {
        temporary
            .persist_noclobber(path)
            .map_err(|error| ProjectError::from(error.error))?;
    }
    Ok(())
}
