//! Password-free connection profiles and explicitly opted-in local query history.
pub use dbide_driver_api::DatabaseEngine;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_PROFILES: usize = 100;
pub const MAX_HISTORY: usize = 100;
pub const MAX_SQL_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TlsMode {
    Disable,
    Require,
    #[default]
    VerifyFull,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub engine: DatabaseEngine,
    #[serde(default)]
    pub sqlite_path: Option<String>,
    #[serde(default)]
    pub sqlite_create: bool,
    #[serde(default)]
    pub read_only: bool,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub tls_mode: TlsMode,
    #[serde(default)]
    pub root_cert_path: Option<String>,
    pub connect_timeout_secs: u64,
}

impl Default for ConnectionProfile {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: "Local PostgreSQL".into(),
            engine: DatabaseEngine::PostgreSql,
            sqlite_path: None,
            sqlite_create: false,
            read_only: false,
            host: "localhost".into(),
            port: 5432,
            database: "postgres".into(),
            username: "postgres".into(),
            tls_mode: TlsMode::VerifyFull,
            root_cert_path: None,
            connect_timeout_secs: 10,
        }
    }
}

impl ConnectionProfile {
    pub fn new() -> Self {
        Self::default()
    }
    /// Only the project-owned loopback fixture. No production credentials.
    pub fn fixture() -> Self {
        Self {
            id: "dbide-local-fixture".into(),
            name: "Local test fixture".into(),
            host: "127.0.0.1".into(),
            port: 55439,
            database: "dbide_fixture".into(),
            tls_mode: TlsMode::Disable,
            ..Self::default()
        }
    }
    pub fn validate(&self) -> Result<(), ProfileValidationError> {
        validate_text("id", &self.id, 128)?;
        validate_text("name", &self.name, 128)?;
        if !(1..=300).contains(&self.connect_timeout_secs) {
            return Err(ProfileValidationError::new(
                "connect_timeout_secs",
                "must be between 1 and 300",
            ));
        }
        if self.engine == DatabaseEngine::Sqlite {
            let path = self.sqlite_path.as_deref().ok_or_else(|| {
                ProfileValidationError::new("sqlite_path", "must be provided for SQLite")
            })?;
            if path.trim().is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
                return Err(ProfileValidationError::new(
                    "sqlite_path",
                    "must be a nonempty path without control characters (max 4096 bytes)",
                ));
            }
            return Ok(());
        }
        for (field, value, limit) in [
            ("host", self.host.as_str(), 253),
            ("database", &self.database, 256),
            ("username", &self.username, 256),
        ] {
            validate_text(field, value, limit)?;
        }
        if self.host.chars().any(char::is_whitespace) || self.host.contains(['/', '?', '#', '@']) {
            return Err(ProfileValidationError::new(
                "host",
                "must be a hostname or IP address, without a URL or credentials",
            ));
        }
        if let Some(path) = &self.root_cert_path
            && (path.trim().is_empty()
                || path != path.trim()
                || path.len() > 4096
                || path.chars().any(char::is_control))
        {
            return Err(ProfileValidationError::new(
                "root_cert_path",
                "must be a nonempty certificate path without surrounding whitespace or control characters (max 4096 bytes)",
            ));
        }
        if self.port == 0 {
            return Err(ProfileValidationError::new(
                "port",
                "must be between 1 and 65535",
            ));
        }
        Ok(())
    }
}

fn validate_text(
    field: &'static str,
    value: &str,
    limit: usize,
) -> Result<(), ProfileValidationError> {
    if value.trim().is_empty() {
        return Err(ProfileValidationError::new(field, "must not be empty"));
    }
    if value != value.trim() || value.chars().any(char::is_control) {
        return Err(ProfileValidationError::new(
            field,
            "must not contain surrounding whitespace or control characters",
        ));
    }
    if value.len() > limit {
        return Err(ProfileValidationError::new(field, "is too long"));
    }
    Ok(())
}

#[derive(Clone)]
pub struct ConnectionRequest {
    pub profile: ConnectionProfile,
    pub password: String,
}
impl fmt::Debug for ConnectionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionRequest")
            .field("profile", &self.profile)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileValidationError {
    pub field: &'static str,
    pub message: &'static str,
}
impl ProfileValidationError {
    fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }
}
impl fmt::Display for ProfileValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}
impl std::error::Error for ProfileValidationError {}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoryEntry {
    pub profile_id: String,
    pub sql: String,
    pub executed_at_unix_ms: u64,
    pub outcome: String,
    pub row_count: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceState {
    pub schema_version: u32,
    pub profiles: Vec<ConnectionProfile>,
    pub selected_profile_id: Option<String>,
    pub history: Vec<HistoryEntry>,
    pub persist_history: bool,
}
impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            profiles: vec![],
            selected_profile_id: None,
            history: vec![],
            persist_history: false,
        }
    }
}
impl WorkspaceState {
    pub fn validate(&self) -> Result<(), StoreError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(StoreError::InvalidState(
                "unsupported profile schema version".into(),
            ));
        }
        if self.profiles.len() > MAX_PROFILES || self.history.len() > MAX_HISTORY {
            return Err(StoreError::InvalidState(
                "profile or history count exceeds limit".into(),
            ));
        }
        let mut ids = HashSet::new();
        for profile in &self.profiles {
            profile.validate().map_err(StoreError::Validation)?;
            if !ids.insert(profile.id.as_str()) {
                return Err(StoreError::InvalidState("duplicate profile id".into()));
            }
        }
        if self
            .selected_profile_id
            .as_deref()
            .is_some_and(|id| !ids.contains(id))
        {
            return Err(StoreError::InvalidState(
                "selected profile does not exist".into(),
            ));
        }
        for entry in &self.history {
            validate_entry(entry)?;
        }
        Ok(())
    }
    pub fn upsert(&mut self, profile: ConnectionProfile) -> Result<(), StoreError> {
        profile.validate().map_err(StoreError::Validation)?;
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.id == profile.id) {
            *existing = profile;
        } else {
            if self.profiles.len() >= MAX_PROFILES {
                return Err(StoreError::InvalidState(
                    "profile count exceeds limit".into(),
                ));
            }
            self.profiles.push(profile);
        }
        Ok(())
    }
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.id != id);
        if self.selected_profile_id.as_deref() == Some(id) {
            self.selected_profile_id = None;
        }
        self.history.retain(|e| e.profile_id != id);
        self.profiles.len() != before
    }
    pub fn select(&mut self, id: Option<&str>) -> Result<(), StoreError> {
        if id.is_some_and(|id| !self.profiles.iter().any(|p| p.id == id)) {
            return Err(StoreError::InvalidState(
                "selected profile does not exist".into(),
            ));
        }
        self.selected_profile_id = id.map(str::to_owned);
        Ok(())
    }
    /// Keeps bounded session history even while disk persistence is disabled.
    pub fn record_history(&mut self, entry: HistoryEntry) -> Result<(), StoreError> {
        validate_entry(&entry)?;
        self.history.push(entry);
        if self.history.len() > MAX_HISTORY {
            self.history.drain(..self.history.len() - MAX_HISTORY);
        }
        Ok(())
    }
}
fn validate_entry(entry: &HistoryEntry) -> Result<(), StoreError> {
    if entry.sql.len() > MAX_SQL_BYTES
        || entry.profile_id.is_empty()
        || entry.profile_id.len() > 128
        || entry.outcome.len() > 256
    {
        return Err(StoreError::InvalidState(
            "history entry exceeds limits or has no profile id".into(),
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Json(serde_json::Error),
    Validation(ProfileValidationError),
    InvalidState(String),
    MissingHome,
    MissingAppData,
    TooLarge,
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "Profile file I/O failed: {e}"),
            Self::Json(_) => write!(f, "Profile file is malformed; original file was preserved"),
            Self::Validation(e) => e.fmt(f),
            Self::InvalidState(e) => e.fmt(f),
            Self::MissingAppData => {
                write!(f, "APPDATA is unavailable; choose an explicit profile path")
            }
            Self::MissingHome => write!(f, "HOME is unavailable; choose an explicit profile path"),
            Self::TooLarge => write!(f, "Profile file exceeds the 1 MiB limit"),
        }
    }
}
impl std::error::Error for StoreError {}
impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

#[derive(Clone, Copy)]
enum StoragePlatform {
    MacOs,
    Windows,
    Unix,
}

// Pure path calculation; tests never mutate process environment or touch real settings.
fn resolve_default_path(
    platform: StoragePlatform,
    home: Option<&Path>,
    appdata: Option<&Path>,
    xdg: Option<&Path>,
) -> Result<PathBuf, StoreError> {
    let nonempty = |path: &&Path| !path.as_os_str().is_empty();
    let folder = match platform {
        StoragePlatform::MacOs => home
            .filter(nonempty)
            .ok_or(StoreError::MissingHome)?
            .join("Library/Application Support/RustDatabaseIDE"),
        StoragePlatform::Windows => appdata
            .filter(nonempty)
            .ok_or(StoreError::MissingAppData)?
            .join("RustDatabaseIDE"),
        StoragePlatform::Unix => {
            if let Some(base) = xdg.filter(nonempty).filter(|path| path.is_absolute()) {
                base.join("RustDatabaseIDE")
            } else {
                home.filter(nonempty)
                    .ok_or(StoreError::MissingHome)?
                    .join(".config/RustDatabaseIDE")
            }
        }
    };
    Ok(folder.join("profiles.json"))
}

#[derive(Clone, Debug)]
pub struct ProfileStore {
    path: PathBuf,
}
impl ProfileStore {
    /// Does not create files or directories. Existing malformed files return an error.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let store = Self { path: path.into() };
        store.load()?;
        Ok(store)
    }
    pub fn default_path() -> Result<PathBuf, StoreError> {
        let platform = if cfg!(target_os = "macos") {
            StoragePlatform::MacOs
        } else if cfg!(target_os = "windows") {
            StoragePlatform::Windows
        } else {
            StoragePlatform::Unix
        };
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        resolve_default_path(
            platform,
            home.as_deref(),
            appdata.as_deref(),
            xdg.as_deref(),
        )
    }
    pub fn open_default() -> Result<Self, StoreError> {
        Self::open(Self::default_path()?)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Reads regular files only, including symlinks whose opened target is regular.
    /// Saving atomically replaces the named directory entry rather than following
    /// a snapshot symlink when writing. Special files are rejected before reading.
    pub fn load(&self) -> Result<WorkspaceState, StoreError> {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        // Opening a FIFO itself blocks unless O_NONBLOCK is set; checking only
        // the path before open would leave a replacement race.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = match options.open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(WorkspaceState::default()),
            Err(e) => return Err(e.into()),
        };
        require_regular_file(&file)?;
        let mut bytes = Vec::new();
        file.take((MAX_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(StoreError::TooLarge);
        }
        let mut state: WorkspaceState = serde_json::from_slice(&bytes)?;
        state.validate()?;
        if !state.persist_history {
            state.history.clear();
        }
        Ok(state)
    }
    /// Replaces the entire state under the shared writer lock.
    /// For changes to an already loaded workspace, use `update` to avoid stale overwrites.
    pub fn save(&self, state: &WorkspaceState) -> Result<(), StoreError> {
        let _lock = self.lock_writer()?;
        // Preserve corrupt/unsupported originals even if they changed after open.
        self.load()?;
        self.save_unlocked(state)
    }
    /// Atomically reads the latest state, applies a bounded change, and persists it.
    /// The callback must not call this store's save/update or perform external I/O.
    /// Callback/validation/write errors leave the existing state untouched.
    pub fn update(
        &self,
        action: impl FnOnce(&mut WorkspaceState) -> Result<(), StoreError>,
    ) -> Result<WorkspaceState, StoreError> {
        let _lock = self.lock_writer()?;
        let mut state = self.load()?;
        action(&mut state)?;
        self.save_unlocked(&state)?;
        Ok(state)
    }
    fn lock_writer(&self) -> Result<fs::File, StoreError> {
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let lock = options.open(self.path.with_extension("json.lock"))?;
        require_regular_file(&lock)?;
        lock.lock()?;
        Ok(lock)
    }
    fn save_unlocked(&self, state: &WorkspaceState) -> Result<(), StoreError> {
        state.validate()?;
        let mut persisted = state.clone();
        if !persisted.persist_history {
            persisted.history.clear();
        }
        let bytes = serde_json::to_vec_pretty(&persisted)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(StoreError::TooLarge);
        }
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
        let temporary = parent.join(format!(".profiles-{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<(), StoreError> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn require_regular_file(file: &fs::File) -> io::Result<()> {
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Settings and lock paths must identify regular files",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(sql: &str) -> HistoryEntry {
        HistoryEntry {
            profile_id: "dbide-local-fixture".into(),
            sql: sql.into(),
            executed_at_unix_ms: 1,
            outcome: "completed".into(),
            row_count: Some(1),
        }
    }
    fn state() -> WorkspaceState {
        let mut state = WorkspaceState::default();
        state.upsert(ConnectionProfile::fixture()).unwrap();
        state.select(Some("dbide-local-fixture")).unwrap();
        state
    }
    #[test]
    fn defaults_are_general_secure_and_ids_are_unique() {
        let a = ConnectionProfile::default();
        let b = ConnectionProfile::new();
        assert_eq!(a.host, "localhost");
        assert_eq!(a.port, 5432);
        assert_eq!(a.tls_mode, TlsMode::VerifyFull);
        assert_ne!(a.id, b.id);
        assert_eq!(ConnectionProfile::fixture().tls_mode, TlsMode::Disable);
        a.validate().unwrap();
    }
    #[test]
    fn request_debug_redacts_password() {
        let request = ConnectionRequest {
            profile: ConnectionProfile::default(),
            password: "secret-example-password".into(),
        };
        let debug = format!("{request:?}");
        assert!(!debug.contains(&request.password));
        assert!(debug.contains("[REDACTED]"));
    }
    #[test]
    fn rejects_invalid_fields_without_echoing_them() {
        let mut profile = ConnectionProfile::default();
        for host in [
            "",
            " localhost",
            "some host",
            "postgres://secret@server",
            "server\n",
        ] {
            profile.host = host.into();
            assert!(profile.validate().is_err());
        }
        profile.host = "::1".into();
        profile.validate().unwrap();
        profile.username = " ".into();
        assert_eq!(profile.validate().unwrap_err().field, "username");
        profile.username = "postgres".into();
        profile.port = 0;
        assert_eq!(profile.validate().unwrap_err().field, "port");
        profile.port = 5432;
        profile.connect_timeout_secs = 301;
        assert!(profile.validate().is_err());
    }
    #[test]
    fn preserves_profile_identity_selection_and_removal() {
        let mut state = state();
        let mut profile = state.profiles[0].clone();
        profile.name = "Changed".into();
        state.upsert(profile).unwrap();
        assert_eq!(state.profiles.len(), 1);
        assert!(state.select(Some("missing")).is_err());
        state.record_history(entry("SELECT 1")).unwrap();
        assert!(state.remove("dbide-local-fixture"));
        assert!(state.selected_profile_id.is_none());
        assert!(state.history.is_empty());
    }
    #[test]
    fn round_trip_history_is_explicitly_opted_in_and_password_fields_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app/profiles.json");
        let store = ProfileStore::open(&path).unwrap();
        assert!(!path.exists());
        let mut state = state();
        state.record_history(entry("SELECT 'private sql'")).unwrap();
        store.save(&state).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("private sql"));
        assert!(store.load().unwrap().history.is_empty());
        state.persist_history = true;
        store.save(&state).unwrap();
        assert_eq!(store.load().unwrap(), state);
        state.persist_history = false;
        store.save(&state).unwrap();
        assert!(store.load().unwrap().history.is_empty());
        let mut json = serde_json::to_value(&state).unwrap();
        json["profiles"][0]["password"] = "never-store".into();
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(matches!(store.load(), Err(StoreError::Json(_))));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn corrupt_and_future_files_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        let store = ProfileStore::open(&path).unwrap();
        for bytes in [b"not json".to_vec(), {
            let mut s = state();
            s.schema_version = 2;
            serde_json::to_vec(&s).unwrap()
        }] {
            fs::write(&path, &bytes).unwrap();
            assert!(ProfileStore::open(&path).is_err());
            assert!(store.save(&state()).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }
    #[test]
    fn rejects_duplicates_and_missing_selection() {
        let mut s = state();
        s.profiles.push(s.profiles[0].clone());
        assert!(s.validate().is_err());
        s.profiles.pop();
        s.selected_profile_id = Some("missing".into());
        assert!(s.validate().is_err());
    }
    #[test]
    fn bounds_history_and_rejects_oversized_input_without_replacing_original() {
        let mut s = state();
        for n in 0..MAX_HISTORY + 5 {
            s.record_history(entry(&n.to_string())).unwrap();
        }
        assert_eq!(s.history.len(), MAX_HISTORY);
        assert_eq!(s.history[0].sql, "5");
        assert!(
            s.record_history(entry(&"x".repeat(MAX_SQL_BYTES + 1)))
                .is_err()
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        let store = ProfileStore::open(&path).unwrap();
        store.save(&s).unwrap();
        let original = fs::read(&path).unwrap();
        s.persist_history = true;
        s.history = (0..MAX_HISTORY)
            .map(|_| entry(&"x".repeat(MAX_SQL_BYTES)))
            .collect();
        assert!(matches!(store.save(&s), Err(StoreError::TooLarge)));
        assert_eq!(fs::read(&path).unwrap(), original);
        let huge = vec![b' '; MAX_FILE_BYTES + 1];
        fs::write(&path, &huge).unwrap();
        assert!(matches!(store.load(), Err(StoreError::TooLarge)));
        assert_eq!(fs::read(&path).unwrap(), huge);
    }
    #[test]
    fn atomic_rename_failure_cleans_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        let store = ProfileStore::open(&path).unwrap();
        // A directory replacing the target makes load fail before any write.
        fs::create_dir(&path).unwrap();
        assert!(store.save(&state()).is_err());
        assert!(path.is_dir());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}

#[cfg(test)]
mod engine_tests {
    use super::*;

    #[test]
    fn profile_id_and_engine_round_trip_without_identity_changes() {
        for engine in [
            DatabaseEngine::PostgreSql,
            DatabaseEngine::Sqlite,
            DatabaseEngine::MySql,
        ] {
            let profile = ConnectionProfile {
                engine,
                sqlite_path: Some("/tmp/fixture.sqlite".into()),
                ..ConnectionProfile::default()
            };
            let bytes = serde_json::to_vec(&profile).unwrap();
            let restored: ConnectionProfile = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(restored, profile);
            restored.validate().unwrap();
        }
    }

    #[test]
    fn legacy_profiles_default_to_postgres_without_creating_files() {
        let original = ConnectionProfile::fixture();
        let mut json = serde_json::to_value(&original).unwrap();
        for field in ["engine", "sqlite_path", "sqlite_create", "read_only"] {
            json.as_object_mut().unwrap().remove(field);
        }
        let restored: ConnectionProfile = serde_json::from_value(json).unwrap();
        assert_eq!(restored, original);
        assert_eq!(restored.engine, DatabaseEngine::PostgreSql);
        assert!(restored.sqlite_path.is_none());
        assert!(!restored.sqlite_create);
        assert!(!restored.read_only);
        // A SQLite path never implicitly selects the SQLite engine.
        let mut postgres = restored;
        postgres.sqlite_path = Some("/tmp/example.sqlite".into());
        postgres.host.clear();
        assert_eq!(postgres.validate().unwrap_err().field, "host");
    }

    #[test]
    fn sqlite_requires_its_path_and_ignores_network_fields() {
        let mut profile = ConnectionProfile {
            engine: DatabaseEngine::Sqlite,
            host: String::new(),
            port: 0,
            database: String::new(),
            username: String::new(),
            root_cert_path: Some("\n".into()),
            ..ConnectionProfile::default()
        };
        assert_eq!(profile.validate().unwrap_err().field, "sqlite_path");
        for invalid in [
            "".into(),
            "   ".into(),
            "bad\0path".into(),
            "bad\npath".into(),
            "x".repeat(4097),
        ] {
            profile.sqlite_path = Some(invalid);
            assert_eq!(profile.validate().unwrap_err().field, "sqlite_path");
        }
        for valid in [
            "/tmp/a database.sqlite",
            " leading-space.sqlite",
            ":memory:",
        ] {
            profile.sqlite_path = Some(valid.into());
            profile.validate().unwrap();
        }
        profile.sqlite_path = Some("x".repeat(4096));
        profile.validate().unwrap();
        profile.name.clear();
        assert_eq!(profile.validate().unwrap_err().field, "name");
    }

    #[test]
    fn mysql_retains_network_validation() {
        let mut profile = ConnectionProfile {
            engine: DatabaseEngine::MySql,
            port: 3306,
            ..ConnectionProfile::default()
        };
        profile.validate().unwrap();
        profile.username.clear();
        assert_eq!(profile.validate().unwrap_err().field, "username");
        profile.username = "user".into();
        profile.port = 0;
        assert_eq!(profile.validate().unwrap_err().field, "port");
    }

    #[test]
    fn default_paths_are_platform_specific_and_environment_free() {
        let home = Path::new("/test/home");
        let roaming = Path::new("/test/roaming");
        let config = Path::new("/test/xdg");
        assert_eq!(
            resolve_default_path(
                StoragePlatform::MacOs,
                Some(home),
                Some(roaming),
                Some(config)
            )
            .unwrap(),
            PathBuf::from("/test/home/Library/Application Support/RustDatabaseIDE/profiles.json")
        );
        assert_eq!(
            resolve_default_path(
                StoragePlatform::Windows,
                Some(home),
                Some(roaming),
                Some(config)
            )
            .unwrap(),
            PathBuf::from("/test/roaming/RustDatabaseIDE/profiles.json")
        );
        assert_eq!(
            resolve_default_path(
                StoragePlatform::Unix,
                Some(home),
                Some(roaming),
                Some(config)
            )
            .unwrap(),
            PathBuf::from("/test/xdg/RustDatabaseIDE/profiles.json")
        );
        assert_eq!(
            resolve_default_path(StoragePlatform::Unix, Some(home), None, None).unwrap(),
            PathBuf::from("/test/home/.config/RustDatabaseIDE/profiles.json")
        );
        assert_eq!(
            resolve_default_path(
                StoragePlatform::Unix,
                Some(home),
                None,
                Some(Path::new("relative"))
            )
            .unwrap(),
            PathBuf::from("/test/home/.config/RustDatabaseIDE/profiles.json")
        );
        assert_eq!(
            resolve_default_path(StoragePlatform::Unix, Some(home), None, Some(Path::new("")))
                .unwrap(),
            PathBuf::from("/test/home/.config/RustDatabaseIDE/profiles.json")
        );
        assert!(matches!(
            resolve_default_path(StoragePlatform::Windows, Some(home), None, None),
            Err(StoreError::MissingAppData)
        ));
        assert!(matches!(
            resolve_default_path(StoragePlatform::MacOs, None, Some(roaming), Some(config)),
            Err(StoreError::MissingHome)
        ));
        assert!(matches!(
            resolve_default_path(StoragePlatform::Unix, None, None, None),
            Err(StoreError::MissingHome)
        ));
    }
}
