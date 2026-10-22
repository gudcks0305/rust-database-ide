//! Private, bounded, independently readable historical catalog shards.
//! Call only from a worker. No credentials, SQL editor buffers, row data, or database I/O.
//! Schema definitions are metadata and are never executed by this store.
mod search;
pub use search::{SearchLimits, SearchMatch, SearchPage, SearchProfile, SearchQuery};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const VERSION: u32 = 1;
pub const MAX_SHARD_BYTES: usize = 1024 * 1024;
pub const MAX_DISK_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_SHARDS: usize = 2048;
const MARKER: &str = ".dbide-catalog-v1";
const MARKER_CONTENT: &[u8] = b"dbide historical catalog cache v1\n";
const INDEX: &str = "index-v1.json";
const WRITER_LOCK: &str = ".writer.lock";
const MAX_INDEX_BYTES: usize = 2 * 1024 * 1024;
const MAX_KEY_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    pub schema: String,
    pub name: String,
    pub kind: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    Databases,
    Schemas,
    Objects(String),
    Columns(Relation),
    Details(Relation),
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    pub profile_id: String,
    pub fingerprint: String,
    pub database: String,
    pub scope: Scope,
}
impl Key {
    pub fn inside(&self, parent: &Self) -> bool {
        if self.profile_id != parent.profile_id || self.fingerprint != parent.fingerprint {
            return false;
        }
        match &parent.scope {
            Scope::Databases => true,
            Scope::Schemas => {
                self.database == parent.database && !matches!(self.scope, Scope::Databases)
            }
            Scope::Objects(schema) => {
                self.database == parent.database
                    && (self.scope == parent.scope
                        || matches!(&self.scope, Scope::Columns(r) | Scope::Details(r) if &r.schema == schema))
            }
            Scope::Columns(relation) => {
                self.database == parent.database
                    && matches!(&self.scope, Scope::Columns(r) | Scope::Details(r) if r == relation)
            }
            Scope::Details(_) => self == parent,
        }
    }
    pub fn filename(&self) -> String {
        format!(
            "{:x}.json",
            Sha256::digest(serde_json::to_vec(self).expect("key serializes"))
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Database {
    pub name: String,
    pub accessible: bool,
    pub current: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Object {
    pub schema: String,
    pub name: String,
    pub kind: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key_ordinal: Option<u16>,
    pub foreign_key: bool,
    pub default: Option<String>,
    pub extra: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Data {
    Databases(Vec<Database>),
    Schemas(Vec<String>),
    Objects(Vec<Object>),
    Columns(Vec<Column>),
    Details {
        table: Option<Box<dbide_schema::Table>>,
        view: Option<Box<dbide_schema::View>>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub captured_unix_ms: u64,
    pub truncated: bool,
    /// True when the server supplied only a partial structural snapshot.
    pub fields_partial: bool,
    pub data: Data,
}
impl Snapshot {
    fn matches(&self, key: &Key) -> bool {
        match (&key.scope, &self.data) {
            (Scope::Databases, Data::Databases(_)) | (Scope::Schemas, Data::Schemas(_)) => true,
            (Scope::Objects(schema), Data::Objects(objects)) => {
                objects.iter().all(|o| &o.schema == schema)
            }
            (Scope::Columns(_), Data::Columns(_)) => true,
            (
                Scope::Details(relation),
                Data::Details {
                    table: Some(table),
                    view: None,
                },
            ) => relation.kind == "table" && table.name == relation.name,
            (
                Scope::Details(relation),
                Data::Details {
                    table: None,
                    view: Some(view),
                },
            ) => relation.kind != "table" && view.name == relation.name,
            _ => false,
        }
    }
}
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    key: Key,
    snapshot: Snapshot,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    version: u32,
    entries: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    key: Key,
    bytes: u64,
    captured_unix_ms: u64,
}

/// Construction performs no filesystem access. Lookup opens exactly one branch shard.
pub struct Store {
    root: PathBuf,
    sequence: u64,
}
impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root, sequence: 0 }
    }
    pub fn load(&mut self, key: &Key) -> Result<Option<Snapshot>, String> {
        if !self.ensure_root(false)? {
            return Ok(None);
        }
        if !self
            .read_index()?
            .entries
            .iter()
            .any(|entry| entry.key == *key)
        {
            return Ok(None);
        }
        self.load_shard(key)
    }
    fn load_shard(&self, key: &Key) -> Result<Option<Snapshot>, String> {
        let path = self.root.join(key.filename());
        let Some(bytes) = read_optional(&path, MAX_SHARD_BYTES)? else {
            return Ok(None);
        };
        let envelope: Envelope = serde_json::from_slice(&bytes)
            .map_err(|_| "Catalog shard corrupt or unsupported; ignored.".to_string())?;
        if envelope.version != VERSION || envelope.key != *key || !envelope.snapshot.matches(key) {
            return Err("Catalog shard version, identity or scope mismatch; ignored.".into());
        }
        Ok(Some(envelope.snapshot))
    }
    pub fn save(&mut self, key: Key, snapshot: Snapshot) -> Result<(), String> {
        if !snapshot.matches(&key) {
            return Err("Catalog snapshot scope mismatch.".into());
        }
        if serde_json::to_vec_pretty(&key)
            .map_err(|e| e.to_string())?
            .len()
            > MAX_KEY_BYTES
        {
            return Err("Catalog identity exceeds 16 KiB.".into());
        }
        let bytes = serde_json::to_vec_pretty(&Envelope {
            version: VERSION,
            key: key.clone(),
            snapshot: snapshot.clone(),
        })
        .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_SHARD_BYTES {
            return Err("Catalog shard exceeds 1 MiB; memory snapshot only.".into());
        }
        self.ensure_root(true)?;
        let _writer_lock = self.writer_lock()?;
        // Re-read the latest manifest only after acquiring the cross-instance lock.
        let mut index = self.read_index()?;
        // Existing unrecognized/corrupt/future-version files are never replaced.
        let existing = self.load_shard(&key)?;
        let existing_bytes = if existing.is_some() {
            fs::metadata(self.root.join(key.filename()))
                .map_err(|e| e.to_string())?
                .len()
        } else {
            0
        };
        // Reserve enough for either the prior or new payload. If payload publication
        // fails after the manifest rename, the old file remains fully budgeted.
        let reserved_bytes = (bytes.len() as u64).max(existing_bytes).max(
            index
                .entries
                .iter()
                .find(|entry| entry.key == key)
                .map_or(0, |entry| entry.bytes),
        );
        index.entries.retain(|e| e.key != key);
        index.entries.sort_by_key(|e| e.captured_unix_ms);
        let mut total = index.entries.iter().map(|e| e.bytes).sum::<u64>();
        while index.entries.len() >= MAX_SHARDS || total + reserved_bytes > MAX_DISK_BYTES {
            if index.entries.is_empty() {
                return Err("Catalog disk budget exceeded.".into());
            }
            let old = index.entries.remove(0);
            self.remove_known(&old.key)?;
            total = total.saturating_sub(old.bytes);
        }
        index.entries.push(Entry {
            key: key.clone(),
            bytes: reserved_bytes,
            captured_unix_ms: snapshot.captured_unix_ms,
        });
        // Check index size before mutating shard; no untracked oversized identity entry.
        let index_bytes = encode_index(&index)?;
        // Publish identity first: a crash may leave a harmless missing shard, never
        // an unindexed payload that clear/invalidation cannot find.
        self.atomic_write(INDEX, &index_bytes)?;
        self.atomic_write(&key.filename(), &bytes)
    }
    pub fn invalidate(&mut self, parent: Option<&Key>) -> Result<(), String> {
        if !self.ensure_root(false)? {
            return Ok(());
        }
        let _writer_lock = self.writer_lock()?;
        let mut index = self.read_index()?;
        let mut diagnostic = None;
        for entry in &index.entries {
            if parent.is_none_or(|p| entry.key.inside(p))
                && let Err(error) = self.remove_known(&entry.key)
            {
                diagnostic.get_or_insert(error);
            }
        }
        index
            .entries
            .retain(|e| parent.is_some_and(|p| !e.key.inside(p)));
        self.atomic_write(INDEX, &encode_index(&index)?)?;
        if let Some(error) = diagnostic {
            return Err(error);
        }
        Ok(())
    }
    fn writer_lock(&self) -> Result<File, String> {
        let mut options = private_options();
        let file = options
            .read(true)
            .write(true)
            .create(true)
            .open(self.root.join(WRITER_LOCK))
            .map_err(|e| format!("Catalog writer lock unavailable: {e}"))?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() != 0 {
            return Err("Unknown catalog writer-lock file preserved.".into());
        }
        #[cfg(unix)]
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("Catalog writer-lock file must be private (0600).".into());
        }
        file.lock()
            .map_err(|e| format!("Catalog writer lock failed: {e}"))?;
        Ok(file) // Closing this independently opened handle releases the advisory lock.
    }
    fn remove_known(&mut self, key: &Key) -> Result<(), String> {
        // Read only ownership/identity header, never every catalog payload.
        // Unknown headers and future-version files remain untouched.
        let path = self.root.join(key.filename());
        let Some(mut file) = open_optional(&path, MAX_SHARD_BYTES)? else {
            return Ok(());
        };
        let mut prefix = Vec::new();
        (&mut file)
            .take((MAX_KEY_BYTES + 256) as u64)
            .read_to_end(&mut prefix)
            .map_err(|e| e.to_string())?;
        let boundary = b"\n  \"snapshot\":";
        let end = prefix
            .windows(boundary.len())
            .position(|bytes| bytes == boundary)
            .ok_or("Catalog shard header unsupported; preserved.")?;
        let mut header_bytes = prefix[..end].to_vec();
        while header_bytes.last().is_some_and(u8::is_ascii_whitespace) {
            header_bytes.pop();
        }
        if header_bytes.pop() != Some(b',') {
            return Err("Catalog shard header corrupt; preserved.".into());
        }
        header_bytes.push(b'}');
        let header: Header = serde_json::from_slice(&header_bytes)
            .map_err(|_| "Catalog shard header corrupt; preserved.".to_string())?;
        if header.version != VERSION || header.key != *key {
            return Err("Catalog shard header unsupported; preserved.".into());
        }
        fs::remove_file(path).map_err(|e| e.to_string())
    }
    fn read_index(&self) -> Result<Index, String> {
        let Some(bytes) = read_optional(&self.root.join(INDEX), MAX_INDEX_BYTES)? else {
            return Ok(Index {
                version: VERSION,
                entries: Vec::new(),
            });
        };
        let index: Index = serde_json::from_slice(&bytes)
            .map_err(|_| "Catalog index corrupt; cache writes disabled.".to_string())?;
        if index.version != VERSION
            || index.entries.len() > MAX_SHARDS
            || index
                .entries
                .iter()
                .any(|e| e.bytes > MAX_SHARD_BYTES as u64)
            || index.entries.iter().map(|e| e.bytes).sum::<u64>() > MAX_DISK_BYTES
        {
            return Err("Catalog index version or limits invalid; cache writes disabled.".into());
        }
        Ok(index)
    }
    fn ensure_root(&self, create: bool) -> Result<bool, String> {
        // Reject symlinks in the configured directory path, including its ancestors.
        let mut component_path = PathBuf::new();
        for component in self.root.components() {
            component_path.push(component.as_os_str());
            match fs::symlink_metadata(&component_path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err("Catalog directory symlink refused.".into());
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        match fs::symlink_metadata(&self.root) {
            Ok(metadata) => {
                if !metadata.is_dir() {
                    return Err("Catalog cache path is not a directory.".into());
                }
                #[cfg(unix)]
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err("Catalog directory must be private (0700).".into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !create => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                builder.mode(0o700);
                builder.create(&self.root).map_err(|e| e.to_string())?;
                let mut options = private_options();
                let mut marker = options
                    .write(true)
                    .create_new(true)
                    .open(self.root.join(MARKER))
                    .map_err(|e| e.to_string())?;
                marker
                    .write_all(MARKER_CONTENT)
                    .map_err(|e| e.to_string())?;
                marker.sync_all().map_err(|e| e.to_string())?;
            }
            Err(e) => return Err(e.to_string()),
        }
        if read_optional(&self.root.join(MARKER), 128)?.as_deref() != Some(MARKER_CONTENT) {
            return Err("Unrecognized catalog directory; existing files preserved.".into());
        }
        Ok(true)
    }
    fn atomic_write(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.sequence = self.sequence.wrapping_add(1);
        let temporary = self
            .root
            .join(format!(".tmp-{}-{}", std::process::id(), self.sequence));
        let mut options = private_options();
        // Do not clean up a pre-existing temporary file when create_new fails.
        let mut file = options
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        let result = (|| {
            file.write_all(bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            fs::rename(&temporary, self.root.join(name)).map_err(|e| e.to_string())?;
            File::open(&self.root)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}
fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    options
}
fn open_optional(path: &Path, max: usize) -> Result<Option<File>, String> {
    let mut options = private_options();
    let file = match options.read(true).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Catalog file unavailable: {e}")),
    };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > max as u64 {
        return Err("Catalog file type or size refused.".into());
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err("Catalog file must be private (0600).".into());
    }
    Ok(Some(file))
}
fn read_optional(path: &Path, max: usize) -> Result<Option<Vec<u8>>, String> {
    let Some(mut file) = open_optional(path, max)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    (&mut file)
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > max {
        return Err("Catalog file exceeds read limit.".into());
    }
    Ok(Some(bytes))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    key: Key,
}
fn encode_index(index: &Index) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec_pretty(index).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_INDEX_BYTES {
        return Err("Catalog index exceeds bounded identity budget.".into());
    }
    Ok(bytes)
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub shard_id: String,
    pub key: Key,
    pub bytes: u64,
    pub captured_unix_ms: u64,
}
impl Store {
    /// Reads only the bounded identity manifest; never reads shard payloads or scans directories.
    pub fn list(&self) -> Result<Vec<Summary>, String> {
        if !self.ensure_root(false)? {
            return Ok(Vec::new());
        }
        Ok(self
            .read_index()?
            .entries
            .into_iter()
            .map(|e| Summary {
                shard_id: e.key.filename(),
                key: e.key,
                bytes: e.bytes,
                captured_unix_ms: e.captured_unix_ms,
            })
            .collect())
    }
    /// Inspects one manifest-listed shard. Arbitrary paths are not accepted.
    pub fn read_by_id(&mut self, id: &str) -> Result<Option<(Key, Snapshot)>, String> {
        let Some(summary) = self.list()?.into_iter().find(|e| e.shard_id == id) else {
            return Ok(None);
        };
        self.load(&summary.key)
            .map(|s| s.map(|snapshot| (summary.key, snapshot)))
    }
}

use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
};
const MAX_JOBS: usize = 32;
enum Job {
    Load(u64, Key),
    Save(Key, Snapshot),
    Invalidate(Option<Key>),
}
struct Queue {
    jobs: VecDeque<Job>,
    shutdown: bool,
}
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}
pub enum Reply {
    Loaded {
        ticket: u64,
        key: Key,
        snapshot: Result<Option<Snapshot>, String>,
    },
    Diagnostic(String),
}
/// One bounded serial disk worker. Invalidation is a barrier after older writes.
/// A full control queue coalesces into a full cache clear, preserving safety.
pub struct Worker {
    shared: Arc<Shared>,
    replies: Receiver<Reply>,
}
impl Worker {
    pub fn start(root: PathBuf) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                jobs: VecDeque::new(),
                shutdown: false,
            }),
            ready: Condvar::new(),
        });
        let worker_shared = shared.clone();
        let (sender, replies) = mpsc::sync_channel(64);
        std::thread::Builder::new()
            .name("catalog-disk".into())
            .spawn(move || {
                let mut store = Store::new(root);
                loop {
                    let job = {
                        let mut queue = worker_shared
                            .queue
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        while queue.jobs.is_empty() && !queue.shutdown {
                            queue = worker_shared
                                .ready
                                .wait(queue)
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                        }
                        if queue.shutdown && queue.jobs.is_empty() {
                            break;
                        }
                        queue.jobs.pop_front().expect("not empty")
                    };
                    match job {
                        Job::Load(ticket, key) => {
                            let snapshot = store.load(&key);
                            // Receiver may disappear during Navigator drop; still drain
                            // queued invalidation barriers before leaving this worker.
                            let _ = sender.send(Reply::Loaded {
                                ticket,
                                key,
                                snapshot,
                            });
                        }
                        Job::Save(key, snapshot) => {
                            if let Err(e) = store.save(key, snapshot) {
                                diagnostic(&sender, e);
                            }
                        }
                        Job::Invalidate(parent) => {
                            if let Err(e) = store.invalidate(parent.as_ref()) {
                                diagnostic(&sender, e);
                            }
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { shared, replies })
    }
    pub fn load(&self, ticket: u64, key: Key) -> bool {
        self.push(Job::Load(ticket, key))
    }
    pub fn save(&self, key: Key, snapshot: Snapshot) -> bool {
        self.push(Job::Save(key, snapshot))
    }
    pub fn invalidate(&self, parent: Option<Key>) {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if queue.jobs.len() >= MAX_JOBS {
            // Keep loads so each receives a terminal response; drop queued saves and
            // coalesce all invalidations into one all-cache barrier at the tail.
            queue.jobs.retain(|j| matches!(j, Job::Load(..)));
            queue.jobs.push_back(Job::Invalidate(None));
        } else {
            queue.jobs.push_back(Job::Invalidate(parent));
        }
        self.shared.ready.notify_one();
    }
    pub fn try_next(&self) -> Option<Reply> {
        self.replies.try_recv().ok()
    }
    fn push(&self, job: Job) -> bool {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Reserve one slot for an invalidation barrier.
        if queue.jobs.len() >= MAX_JOBS - 1 {
            return false;
        }
        queue.jobs.push_back(job);
        self.shared.ready.notify_one();
        true
    }
}
fn diagnostic(sender: &SyncSender<Reply>, message: String) {
    let _ = sender.try_send(Reply::Diagnostic(message));
}
impl Drop for Worker {
    fn drop(&mut self) {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.shutdown = true;
        self.shared.ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Store, Key, Snapshot) {
        let temporary = tempfile::tempdir().unwrap();
        let store = Store::new(
            temporary
                .path()
                .canonicalize()
                .unwrap()
                .join("catalog-cache"),
        );
        let key = Key {
            profile_id: "owned-fixture".into(),
            fingerprint: "exact-endpoint-fingerprint".into(),
            database: "owned".into(),
            scope: Scope::Objects("public".into()),
        };
        let snapshot = Snapshot {
            captured_unix_ms: 1234,
            truncated: true,
            fields_partial: false,
            data: Data::Objects(vec![Object {
                schema: "public".into(),
                name: "orders".into(),
                kind: "table".into(),
            }]),
        };
        (temporary, store, key, snapshot)
    }
    #[test]
    fn independent_shards_survive_reopen_and_read_only_one_branch() {
        let (_temp, mut store, key, snapshot) = fixture();
        assert!(
            !store.root.exists(),
            "constructor must not touch filesystem"
        );
        store.save(key.clone(), snapshot.clone()).unwrap();
        let mut other = key.clone();
        other.database = "another-database".into();
        store.save(other.clone(), snapshot.clone()).unwrap();
        let root = store.root.clone();
        drop(store);
        let mut reopened = Store::new(root);
        // A corrupt unrelated payload must not affect this lookup or manifest list.
        fs::write(reopened.root.join(other.filename()), b"truncated").unwrap();
        assert_eq!(reopened.load(&key).unwrap(), Some(snapshot));
        assert_eq!(reopened.list().unwrap().len(), 2);
        assert!(reopened.load(&other).is_err());
        assert!(reopened.read_by_id("../../outside").unwrap().is_none());
        assert!(reopened.read_by_id(&key.filename()).unwrap().is_some());
    }
    #[test]
    fn corruption_future_version_identity_and_unknown_files_are_preserved() {
        let (_temp, mut store, key, snapshot) = fixture();
        store.save(key.clone(), snapshot.clone()).unwrap();
        let path = store.root.join(key.filename());
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"{").unwrap();
        assert!(store.load(&key).is_err());
        assert!(store.save(key.clone(), snapshot.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{");
        let mut future: serde_json::Value = serde_json::from_slice(&original).unwrap();
        future["version"] = 999.into();
        fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
        assert!(store.load(&key).is_err());
        future["version"] = VERSION.into();
        future["key"]["database"] = "wrong".into();
        fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
        assert!(store.load(&key).is_err());
        let unknown = store.root.join("user-notes.txt");
        fs::write(&unknown, "keep me").unwrap();
        assert!(store.invalidate(None).is_err());
        assert!(store.list().unwrap().is_empty());
        assert!(
            store.load(&key).unwrap().is_none(),
            "invalidated unknown shard cannot be revived by direct lookup"
        );
        assert_eq!(fs::read_to_string(unknown).unwrap(), "keep me");
    }
    #[test]
    fn oversize_scope_and_unowned_directory_are_refused() {
        let (_temp, mut store, key, mut snapshot) = fixture();
        snapshot.data = Data::Schemas(vec!["wrong-scope".into()]);
        assert!(store.save(key.clone(), snapshot.clone()).is_err());
        snapshot.data = Data::Objects(vec![Object {
            schema: "public".into(),
            name: "x".repeat(MAX_SHARD_BYTES),
            kind: "table".into(),
        }]);
        assert!(store.save(key.clone(), snapshot).is_err());
        assert!(!store.root.exists());
        fs::create_dir(&store.root).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&store.root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(store.root.join("user-data"), "preserve").unwrap();
        assert!(store.load(&key).is_err());
        assert!(!store.root.join(MARKER).exists());
    }
    #[test]
    fn invalidation_is_scoped_and_worker_barrier_prevents_old_save_resurrection() {
        let (_temp, mut store, key, snapshot) = fixture();
        let mut other = key.clone();
        other.database = "another".into();
        store.save(key.clone(), snapshot.clone()).unwrap();
        store.save(other.clone(), snapshot.clone()).unwrap();
        store.invalidate(Some(&key)).unwrap();
        assert!(store.load(&key).unwrap().is_none());
        assert!(store.load(&other).unwrap().is_some());
        let worker = Worker::start(store.root.clone()).unwrap();
        assert!(worker.save(key.clone(), snapshot));
        worker.invalidate(Some(key.clone()));
        assert!(worker.load(77, key));
        let start = std::time::Instant::now();
        loop {
            if let Some(Reply::Loaded {
                ticket, snapshot, ..
            }) = worker.try_next()
            {
                assert_eq!(ticket, 77);
                assert!(snapshot.unwrap().is_none());
                break;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(3));
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(store.load(&other).unwrap().is_some());
    }

    #[test]
    fn independent_writers_preserve_manifest_budget_and_scoped_invalidations() {
        let (_temp, mut store, key, snapshot) = fixture();
        store.save(key.clone(), snapshot.clone()).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let mut threads = Vec::new();
        for side in ["left", "right"] {
            let root = store.root.clone();
            let template = key.clone();
            let value = snapshot.clone();
            let start = barrier.clone();
            threads.push(std::thread::spawn(move || {
                let mut writer = Store::new(root);
                start.wait();
                for n in 0..24 {
                    let current = Key {
                        database: format!("{side}-{n}"),
                        ..template.clone()
                    };
                    writer.save(current.clone(), value.clone()).unwrap();
                    if n % 3 == 0 {
                        writer.invalidate(Some(&current)).unwrap();
                    }
                }
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        let summaries = store.list().unwrap();
        assert_eq!(
            summaries.len(),
            33,
            "both independently locked writers must retain every noninvalidated key"
        );
        let filenames: std::collections::HashSet<_> = summaries
            .iter()
            .map(|entry| entry.shard_id.clone())
            .collect();
        let mut actual_bytes = 0;
        let mut actual_files = 0;
        for entry in fs::read_dir(&store.root).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".json") && name != INDEX {
                assert!(filenames.contains(&name), "unindexed payload {name}");
                actual_files += 1;
                actual_bytes += entry.metadata().unwrap().len();
            }
        }
        assert_eq!(actual_files, 33);
        assert!(actual_files <= MAX_SHARDS);
        assert!(actual_bytes <= summaries.iter().map(|entry| entry.bytes).sum::<u64>());
        assert!(actual_bytes <= MAX_DISK_BYTES);
        store.invalidate(None).unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(
            filenames
                .into_iter()
                .all(|name| !store.root.join(name).exists())
        );
    }
    #[test]
    fn failed_payload_publication_preserves_old_file_and_conservative_reservation() {
        let (_temp, mut store, key, replacement) = fixture();
        let mut original = replacement.clone();
        original.data = Data::Objects(vec![Object {
            schema: "public".into(),
            name: "x".repeat(200_000),
            kind: "table".into(),
        }]);
        store.save(key.clone(), original.clone()).unwrap();
        let actual_before = fs::metadata(store.root.join(key.filename())).unwrap().len();
        // Index publication is the next temp, payload publication the one after it.
        let collision = store.root.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            store.sequence + 2
        ));
        fs::write(&collision, "unknown user file preserved").unwrap();
        assert!(store.save(key.clone(), replacement).is_err());
        assert_eq!(store.load(&key).unwrap(), Some(original));
        let summary = store
            .list()
            .unwrap()
            .into_iter()
            .find(|entry| entry.key == key)
            .unwrap();
        assert!(summary.bytes >= actual_before);
        assert_eq!(
            fs::read_to_string(collision).unwrap(),
            "unknown user file preserved"
        );
        store.invalidate(None).unwrap();
        assert!(!store.root.join(key.filename()).exists());
    }
    #[test]
    fn typed_table_and_view_details_roundtrip_and_invalidate_with_columns() {
        let (_temp, mut store, mut key, mut snapshot) = fixture();
        let relation = Relation {
            schema: "public".into(),
            name: "orders".into(),
            kind: "table".into(),
        };
        key.scope = Scope::Details(relation.clone());
        let table: dbide_schema::Table = serde_json::from_value(serde_json::json!({
            "name": "orders",
            "columns": [{"name":"id","data_type":"bigint","nullable":false,"default":"nextval('orders_id')","generated":"identity"}],
            "primary_key": {"name":"orders_pkey","columns":["id"]},
            "foreign_keys": [{"name":"orders_parent_fk","columns":["id"],"references":{"schema":"public","name":"parents"},"referenced_columns":["id"],"on_delete":"cascade","on_update":"restrict","deferrable":true,"initially_deferred":true}],
            "indexes": [{"name":"orders_idx","columns":["id"],"unique":true,"method":"btree","predicate":"id > 0","expressions":["(id + 1)"],"include":["id"],"constraint_owned":true,"unsupported_features":["nulls_not_distinct"]}],
            "unsupported_features":["partitioned"]
        })).unwrap();
        snapshot.data = Data::Details {
            table: Some(Box::new(table)),
            view: None,
        };
        snapshot.truncated = false;
        store.save(key.clone(), snapshot.clone()).unwrap();
        assert_eq!(store.load(&key).unwrap(), Some(snapshot.clone()));
        let mut bad = key.clone();
        bad.scope = Scope::Details(Relation {
            name: "wrong".into(),
            ..relation.clone()
        });
        assert!(store.save(bad, snapshot.clone()).is_err());
        let mut view_key = key.clone();
        view_key.scope = Scope::Details(Relation {
            schema: "public".into(),
            name: "orders_view".into(),
            kind: "materialized view".into(),
        });
        let view_snapshot = Snapshot {
            data: Data::Details {
                table: None,
                view: Some(Box::new(dbide_schema::View {
                    name: "orders_view".into(),
                    definition: "SELECT id FROM public.orders".into(),
                    materialized: true,
                })),
            },
            ..snapshot
        };
        store.save(view_key.clone(), view_snapshot.clone()).unwrap();
        assert_eq!(store.load(&view_key).unwrap(), Some(view_snapshot));
        let columns_key = Key {
            scope: Scope::Columns(relation),
            ..key.clone()
        };
        store.invalidate(Some(&columns_key)).unwrap();
        assert!(store.load(&key).unwrap().is_none());
        assert!(store.load(&view_key).unwrap().is_some());
        let object_key = Key {
            scope: Scope::Objects("public".into()),
            ..key
        };
        store.invalidate(Some(&object_key)).unwrap();
        assert!(store.load(&view_key).unwrap().is_none());
    }
    #[test]
    fn disk_and_index_limits_are_enforced_without_loading_all_shards() {
        let (_temp, mut store, key, snapshot) = fixture();
        store.save(key.clone(), snapshot.clone()).unwrap();
        let mut index = Index {
            version: VERSION,
            entries: Vec::new(),
        };
        for n in 0..128 {
            let mut k = key.clone();
            k.database = format!("owned-{n}");
            index.entries.push(Entry {
                key: k,
                bytes: MAX_SHARD_BYTES as u64,
                captured_unix_ms: n,
            });
        }
        store
            .atomic_write(INDEX, &encode_index(&index).unwrap())
            .unwrap();
        store.save(key.clone(), snapshot).unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 128);
        assert!(list.iter().map(|e| e.bytes).sum::<u64>() <= MAX_DISK_BYTES);
        // A physically oversized shard is refused before reading its payload.
        let file = OpenOptions::new()
            .write(true)
            .open(store.root.join(key.filename()))
            .unwrap();
        file.set_len(MAX_SHARD_BYTES as u64 + 1).unwrap();
        assert!(store.load(&key).is_err());
        while index.entries.len() <= MAX_SHARDS {
            index.entries.push(Entry {
                key: key.clone(),
                bytes: 1,
                captured_unix_ms: 0,
            });
        }
        store
            .atomic_write(INDEX, &serde_json::to_vec(&index).unwrap())
            .unwrap();
        assert!(store.list().is_err());
    }
    #[cfg(unix)]
    #[test]
    fn private_permissions_and_symlinks_are_enforced() {
        use std::os::unix::fs::symlink;
        let (temp, mut store, key, snapshot) = fixture();
        store.save(key.clone(), snapshot.clone()).unwrap();
        assert_eq!(
            fs::metadata(&store.root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let path = store.root.join(key.filename());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let outside = temp.path().join("outside");
        fs::write(&outside, "untouched").unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&outside, &path).unwrap();
        assert!(store.load(&key).is_err());
        assert!(store.save(key, snapshot).is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "untouched");
    }
}
