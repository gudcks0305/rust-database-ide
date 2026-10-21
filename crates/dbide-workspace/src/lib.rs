//! Bounded local editor recovery; loading never runs SQL or follows document paths.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_DOCUMENTS: usize = 32;
pub const MAX_SQL_BYTES: usize = 64 * 1024;
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DocumentSnapshot {
    pub id: u64,
    pub title: String,
    pub sql: String,
    /// The last known saved buffer, retained so dirty status survives restart.
    pub saved_sql: String,
    pub revision: u64,
    /// Metadata only. The store never opens or overwrites this path.
    pub path: Option<PathBuf>,
    /// Context only. Restoration never configures or opens a connection.
    pub profile_id: Option<String>,
}
impl DocumentSnapshot {
    pub fn dirty(&self) -> bool {
        self.sql != self.saved_sql
    }
    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if self.id == 0 {
            return Err(WorkspaceError::Invalid("document ID must be nonzero"));
        }
        if self.title.trim().is_empty()
            || self.title.len() > 256
            || self.title.chars().any(char::is_control)
        {
            return Err(WorkspaceError::Invalid(
                "document title must be nonempty, at most 256 bytes, without control characters",
            ));
        }
        if self.sql.len() > MAX_SQL_BYTES || self.saved_sql.len() > MAX_SQL_BYTES {
            return Err(WorkspaceError::Invalid(
                "SQL recovery buffers exceed the 64 KiB per-buffer limit",
            ));
        }
        if let Some(path) = &self.path {
            let text = path.to_str().ok_or(WorkspaceError::Invalid(
                "document paths must be valid UTF-8",
            ))?;
            if text.trim().is_empty() || text.len() > 4096 || text.chars().any(char::is_control) {
                return Err(WorkspaceError::Invalid(
                    "document path must be nonempty, at most 4096 bytes, without control characters",
                ));
            }
        }
        if let Some(id) = &self.profile_id
            && (id.trim().is_empty()
                || id != id.trim()
                || id.len() > 128
                || id.chars().any(char::is_control))
        {
            return Err(WorkspaceError::Invalid(
                "profile context ID must be 1–128 bytes without surrounding whitespace or control characters",
            ));
        }
        Ok(())
    }
}
impl fmt::Debug for DocumentSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocumentSnapshot")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("sql", &"[REDACTED]")
            .field("saved_sql", &"[REDACTED]")
            .field("revision", &self.revision)
            .field("path", &self.path)
            .field("profile_id", &self.profile_id)
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSnapshot {
    pub schema_version: u32,
    pub documents: Vec<DocumentSnapshot>,
    pub active_document_id: Option<u64>,
}
impl Default for WorkspaceSnapshot {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            documents: vec![],
            active_document_id: None,
        }
    }
}
impl WorkspaceSnapshot {
    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(WorkspaceError::UnsupportedVersion(self.schema_version));
        }
        if self.documents.len() > MAX_DOCUMENTS {
            return Err(WorkspaceError::Invalid(
                "workspace exceeds the 32 document limit; no tabs were discarded",
            ));
        }
        let mut ids = HashSet::new();
        for document in &self.documents {
            document.validate()?;
            if !ids.insert(document.id) {
                return Err(WorkspaceError::Invalid("duplicate document ID"));
            }
        }
        match self.active_document_id {
            Some(id) if !ids.contains(&id) => {
                Err(WorkspaceError::Invalid("active document ID does not exist"))
            }
            None if !self.documents.is_empty() => Err(WorkspaceError::Invalid(
                "nonempty workspace requires an active document",
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Debug)]
pub enum WorkspaceError {
    Io(io::Error),
    /// No parser source text is exposed in Display.
    Malformed,
    UnsupportedVersion(u32),
    Invalid(&'static str),
    TooLarge,
    /// Rename committed; syncing its parent directory failed. Do not report no save occurred.
    Durability(io::Error),
}
impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "Workspace recovery file I/O failed: {error}"),
            Self::Malformed => write!(
                f,
                "Workspace recovery file is malformed; original file was preserved"
            ),
            Self::UnsupportedVersion(version) => write!(
                f,
                "Workspace recovery schema version {version} is unsupported; original file was preserved"
            ),
            Self::Invalid(reason) => reason.fmt(f),
            Self::TooLarge => write!(
                f,
                "Workspace recovery file exceeds 2 MiB; no tabs were discarded"
            ),
            Self::Durability(error) => write!(
                f,
                "Workspace recovery file was replaced, but directory sync failed: {error}"
            ),
        }
    }
}
impl std::error::Error for WorkspaceError {}
impl From<io::Error> for WorkspaceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceStore {
    path: PathBuf,
}
impl WorkspaceStore {
    /// Pure construction; the caller owns path selection. No global discovery.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    /// Validates existing content without creating files or directories.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let store = Self::new(path);
        store.load()?;
        Ok(store)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Reads regular files only, including symlinks whose opened target is regular.
    /// Saving atomically replaces the named directory entry rather than following
    /// a snapshot symlink when writing. Special files are rejected before reading.
    pub fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError> {
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
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        require_regular_file(&file)?;
        let mut bytes = Vec::new();
        file.take((MAX_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(WorkspaceError::TooLarge);
        }
        let snapshot: WorkspaceSnapshot =
            serde_json::from_slice(&bytes).map_err(|_| WorkspaceError::Malformed)?;
        snapshot.validate()?;
        Ok(Some(snapshot))
    }
    /// Full ordered recovery snapshot replacement. Concurrent instances serialize;
    /// the last successful complete snapshot wins. No implicit document merging.
    pub fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<(), WorkspaceError> {
        snapshot.validate()?;
        let bytes = serde_json::to_vec(snapshot)
            .map_err(|_| WorkspaceError::Invalid("workspace metadata cannot be serialized"))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(WorkspaceError::TooLarge);
        }
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut directories = fs::DirBuilder::new();
        directories.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directories.mode(0o700);
        }
        directories.create(parent)?;
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
        // Corruption or a newer application format must never be silently reset.
        self.load()?;
        let temporary = parent.join(format!(".workspace-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<(), WorkspaceError> {
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
            #[cfg(unix)]
            fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(WorkspaceError::Durability)?;
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
    fn document(id: u64) -> DocumentSnapshot {
        DocumentSnapshot {
            id,
            title: format!("Query {id}"),
            sql: "SELECT '한글 😊'; -- edited".into(),
            saved_sql: "SELECT 1;".into(),
            revision: 9,
            path: Some(PathBuf::from("/must-not-open-or-write/statement.sql")),
            profile_id: Some("profile-may-no-longer-exist".into()),
        }
    }
    fn snapshot() -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            schema_version: SCHEMA_VERSION,
            documents: vec![document(3), document(1), document(2)],
            active_document_id: Some(1),
        }
    }
    #[test]
    fn open_missing_does_not_create_anything() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing/workspace.json");
        let store = WorkspaceStore::open(&path).unwrap();
        assert!(store.load().unwrap().is_none());
        assert!(!path.parent().unwrap().exists());
    }
    #[test]
    fn round_trip_preserves_order_revision_dirty_buffers_and_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let store = WorkspaceStore::open(&path).unwrap();
        let original = snapshot();
        store.save(&original).unwrap();
        let restored = store.load().unwrap().unwrap();
        assert_eq!(restored, original);
        assert_eq!(
            restored.documents.iter().map(|d| d.id).collect::<Vec<_>>(),
            vec![3, 1, 2]
        );
        assert!(restored.documents.iter().all(DocumentSnapshot::dirty));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for file in [&path, &path.with_extension("json.lock")] {
                assert_eq!(
                    fs::metadata(file).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }
    #[test]
    fn sql_debug_is_redacted() {
        let snapshot = snapshot();
        let debug = format!("{snapshot:?}");
        assert!(!debug.contains(&snapshot.documents[0].sql));
        assert!(!debug.contains(&snapshot.documents[0].saved_sql));
        assert!(debug.contains("[REDACTED]"));
    }
    #[test]
    fn rejects_duplicate_missing_active_and_invalid_identifiers() {
        let mut s = snapshot();
        s.documents[1].id = 3;
        assert!(s.validate().is_err());
        s = snapshot();
        s.documents[0].id = 0;
        assert!(s.validate().is_err());
        s = snapshot();
        s.active_document_id = Some(9);
        assert!(s.validate().is_err());
        s.active_document_id = None;
        assert!(s.validate().is_err());
        WorkspaceSnapshot::default().validate().unwrap();
        let mut d = document(1);
        d.profile_id = Some("bad\ncontext".into());
        assert!(d.validate().is_err());
    }
    #[test]
    fn utf8_limits_are_bytes_not_characters() {
        let mut d = document(1);
        d.sql = "한".repeat(MAX_SQL_BYTES / 3);
        d.validate().unwrap();
        d.sql.push('한');
        assert!(d.validate().is_err());
        d.sql.clear();
        d.saved_sql = "x".repeat(MAX_SQL_BYTES + 1);
        assert!(d.validate().is_err());
        d.saved_sql.clear();
        d.path = Some(PathBuf::from("x".repeat(4097)));
        assert!(d.validate().is_err());
    }
    #[test]
    fn invalid_snapshots_never_drop_tabs_or_replace_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let store = WorkspaceStore::new(&path);
        store.save(&snapshot()).unwrap();
        let bytes = fs::read(&path).unwrap();
        let mut invalid = snapshot();
        invalid.documents = (1..=33).map(document).collect();
        assert!(store.save(&invalid).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        invalid = snapshot();
        invalid.documents[0].saved_sql = "x".repeat(MAX_SQL_BYTES + 1);
        assert!(store.save(&invalid).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    #[test]
    fn bounded_total_file_size_preserves_previous_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let store = WorkspaceStore::new(&path);
        store.save(&snapshot()).unwrap();
        let bytes = fs::read(&path).unwrap();
        let documents = (1..=32)
            .map(|id| DocumentSnapshot {
                sql: "x".repeat(MAX_SQL_BYTES),
                saved_sql: "y".repeat(MAX_SQL_BYTES),
                ..document(id)
            })
            .collect();
        let large = WorkspaceSnapshot {
            documents,
            active_document_id: Some(1),
            ..WorkspaceSnapshot::default()
        };
        assert!(matches!(store.save(&large), Err(WorkspaceError::TooLarge)));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::write(&path, vec![b' '; MAX_FILE_BYTES + 1]).unwrap();
        assert!(matches!(store.load(), Err(WorkspaceError::TooLarge)));
    }
    #[test]
    fn partial_corrupt_newer_and_unknown_fields_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let store = WorkspaceStore::new(&path);
        let mut future = snapshot();
        future.schema_version = SCHEMA_VERSION + 1;
        let mut unexpected = serde_json::to_value(snapshot()).unwrap();
        unexpected["password"] = "forbidden-fixture-field".into();
        for bytes in [
            b"{\"schema_version\":1,\"documents\":[".to_vec(),
            serde_json::to_vec(&future).unwrap(),
            serde_json::to_vec(&unexpected).unwrap(),
            vec![0xff],
        ] {
            fs::write(&path, &bytes).unwrap();
            assert!(WorkspaceStore::open(&path).is_err());
            assert!(store.save(&snapshot()).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }
    #[test]
    fn abandoned_partial_temp_is_ignored_and_clean_replacement_stays_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let store = WorkspaceStore::new(&path);
        let before = snapshot();
        store.save(&before).unwrap();
        let abandoned = dir.path().join(".workspace-crashed-fixture.tmp");
        fs::write(&abandoned, b"{partial-write").unwrap();
        assert_eq!(store.load().unwrap().unwrap(), before);
        let mut after = before;
        after.documents[0].revision += 1;
        after.documents[0].sql.push_str("\n-- new");
        store.save(&after).unwrap();
        assert_eq!(store.load().unwrap().unwrap(), after);
        assert_eq!(fs::read(abandoned).unwrap(), b"{partial-write");
    }
    #[test]
    fn recovery_never_reads_or_replaces_referenced_sql_file() {
        let dir = tempfile::tempdir().unwrap();
        let sql_path = dir.path().join("source.sql");
        fs::write(&sql_path, b"separately changed on disk").unwrap();
        let mut s = snapshot();
        s.documents[0].path = Some(sql_path.clone());
        let store = WorkspaceStore::new(dir.path().join("workspace.json"));
        store.save(&s).unwrap();
        assert_eq!(store.load().unwrap().unwrap(), s);
        assert_eq!(fs::read(sql_path).unwrap(), b"separately changed on disk");
    }
}
