//! Lazy, bounded browsing runtime. Never owns or reconfigures a SQL console.
use dbide_bridge::{
    BridgeEvent, DatabaseBridge, DatabaseInfo, OperationId, RelationRef, SchemaColumn, SchemaInfo,
    SchemaObjectInfo,
};
use dbide_catalog_cache::{CacheConfig, CacheStats, MetadataCache, MetadataLease};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine, StoredExecutionTarget};
use std::{
    collections::{HashMap, VecDeque},
    mem::size_of,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub const MAX_BRANCHES: usize = 256;
pub const MAX_PENDING: usize = 16;
pub const WORKERS: usize = 2;
pub const TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Databases,
    Schemas,
    Objects(String),
    Columns(Relation),
    Details(Relation),
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Relation {
    pub schema: String,
    pub name: String,
    pub kind: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub profile_id: String,
    pub fingerprint: String,
    pub generation: u64,
    pub database: String,
    pub scope: Scope,
}
impl Key {
    pub fn child(&self, database: &str, scope: Scope) -> Self {
        Self {
            database: database.into(),
            scope,
            ..self.clone()
        }
    }
    fn inside(&self, parent: &Self) -> bool {
        if self.profile_id != parent.profile_id || self.generation != parent.generation {
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
                    && matches!(&self.scope,Scope::Columns(r)|Scope::Details(r) if r==relation)
            }
            Scope::Details(_) => self == parent,
        }
    }
}
#[derive(Debug)]
pub enum Data {
    Databases(Vec<DatabaseInfo>),
    Schemas(Vec<SchemaInfo>),
    Objects(Vec<SchemaObjectInfo>),
    Columns(Vec<SchemaColumn>),
    Details {
        table: Option<Box<dbide_schema::Table>>,
        view: Option<Box<dbide_schema::View>>,
    },
}
#[derive(Debug)]
pub struct Snapshot {
    pub data: Data,
    pub truncated: bool,
    pub fetched_at: Instant,
    /// Historical disk snapshots never establish current metadata identity.
    pub historical: bool,
    pub captured_unix_ms: u64,
}
impl Snapshot {
    pub fn age(&self) -> Duration {
        if self.historical {
            Duration::from_millis(
                dbide_catalog_store::now_unix_ms().saturating_sub(self.captured_unix_ms),
            )
        } else {
            self.fetched_at.elapsed()
        }
    }
    pub fn stale(&self) -> bool {
        self.historical || self.age() >= TTL
    }
    pub fn len(&self) -> usize {
        match &self.data {
            Data::Databases(v) => v.len(),
            Data::Schemas(v) => v.len(),
            Data::Objects(v) => v.len(),
            Data::Columns(v) => v.len(),
            Data::Details { table, view } => {
                usize::from(table.is_some()) + usize::from(view.is_some())
            }
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
pub type Lease = MetadataLease<Snapshot>;
pub struct Branch {
    pub snapshot: Option<Lease>,
    pub error: Option<String>,
    pub loading: bool,
    ticket: u64,
}
struct Target {
    profile: ConnectionProfile,
    fingerprint: String,
    generation: u64,
}
struct Task {
    key: Key,
    ticket: u64,
    request: Arc<ConnectionRequest>,
}
struct Worker {
    bridge: DatabaseBridge,
    task: Option<Task>,
    configuring: Option<OperationId>,
    operation: Option<OperationId>,
    details: Option<std::sync::mpsc::Receiver<MetadataResult>>,
}

type MetadataResult = Result<(Data, bool), String>;

pub struct Navigator {
    targets: HashMap<String, Target>,
    branches: HashMap<Key, Branch>,
    queue: VecDeque<Task>,
    disk_waiting: HashMap<u64, Task>,
    disk: Option<dbide_catalog_store::Worker>,
    workers: [Option<Worker>; WORKERS],
    cache: MetadataCache<Key, Snapshot>,
    sequence: u64,
    last_tick: Instant,
    pub notice: Option<String>,
}
impl Default for Navigator {
    fn default() -> Self {
        Self::new(CacheConfig::default())
    }
}
impl Navigator {
    pub fn new(config: CacheConfig) -> Self {
        Self {
            targets: HashMap::new(),
            branches: HashMap::new(),
            queue: VecDeque::new(),
            disk_waiting: HashMap::new(),
            disk: None,
            workers: std::array::from_fn(|_| None),
            cache: MetadataCache::new(config).expect("positive metadata budget"),
            sequence: 0,
            last_tick: Instant::now(),
            notice: None,
        }
    }
    /// Starts one bounded disk worker; filesystem access stays off the UI thread.
    pub fn with_store(config: CacheConfig, root: PathBuf) -> Self {
        let mut navigator = Self::new(config);
        match dbide_catalog_store::Worker::start(root) {
            Ok(worker) => navigator.disk = Some(worker),
            Err(error) => navigator.notice = Some(error),
        }
        navigator
    }
    fn invalidate_disk(&self, key: Option<&Key>) {
        if let Some(disk) = &self.disk {
            let parent = match key {
                Some(key) => {
                    let Some(key) = stored_key(key) else {
                        return;
                    };
                    Some(key)
                }
                None => None,
            };
            disk.invalidate(parent);
        }
    }
    fn next_id(&mut self) -> Result<u64, String> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or("Navigator generation exhausted; reopen the workspace.")?;
        Ok(self.sequence)
    }
    pub fn sync_profiles(&mut self, profiles: &[ConnectionProfile]) {
        // The UI timer calls this frequently. An unchanged catalog allocates
        // nothing and avoids rebuilding profile signatures or scanning cache keys.
        if self.targets.len() == profiles.len()
            && profiles
                .iter()
                .all(|p| self.targets.get(&p.id).is_some_and(|t| t.profile == *p))
        {
            return;
        }
        let removed: Vec<_> = self
            .targets
            .keys()
            .filter(|id| !profiles.iter().any(|p| &p.id == *id))
            .filter_map(|id| self.root(id))
            .collect();
        for key in &removed {
            self.invalidate_disk(Some(key));
        }
        self.targets
            .retain(|id, _| profiles.iter().any(|p| &p.id == id));
        for profile in profiles {
            if self
                .targets
                .get(&profile.id)
                .is_some_and(|t| t.profile == *profile)
            {
                continue;
            }
            let mut normalized = profile.clone();
            normalized.name.clear();
            if self.targets.get(&profile.id).is_some_and(|t| {
                let mut old = t.profile.clone();
                old.name.clear();
                old == normalized
            }) {
                if let Some(target) = self.targets.get_mut(&profile.id) {
                    target.profile.name = profile.name.clone();
                }
                continue;
            }
            if let Some(old) = self.root(&profile.id) {
                self.invalidate_disk(Some(&old));
            }
            let database = if profile.engine == DatabaseEngine::Sqlite {
                "main"
            } else {
                &profile.database
            };
            let Ok(identity) = StoredExecutionTarget::capture(profile, database) else {
                self.targets.remove(&profile.id);
                continue;
            };
            let Ok(generation) = self.next_id() else {
                self.notice = Some("Navigator generation exhausted.".into());
                continue;
            };
            self.targets.insert(
                profile.id.clone(),
                Target {
                    profile: profile.clone(),
                    fingerprint: identity.endpoint_fingerprint,
                    generation,
                },
            );
        }
        let targets = &self.targets;
        self.branches.retain(|key, _| authorized(targets, key));
        self.queue.retain(|task| authorized(targets, &task.key));
        self.disk_waiting
            .retain(|_, task| authorized(targets, &task.key));
        self.cache.invalidate_where(|key| !authorized(targets, key));
        // In-flight obsolete probes still occupy their slot until terminal, so rapid
        // profile edits cannot exceed the two concurrent metadata requests.
    }
    pub fn refresh_connection(&mut self, profile_id: &str) -> Result<Key, String> {
        if let Some(key) = self.root(profile_id) {
            self.invalidate_disk(Some(&key));
        }
        let generation = self.next_id()?;
        let target = self
            .targets
            .get_mut(profile_id)
            .ok_or("Connection no longer exists.")?;
        target.generation = generation;
        self.branches.retain(|key, _| key.profile_id != profile_id);
        self.queue.retain(|task| task.key.profile_id != profile_id);
        self.disk_waiting
            .retain(|_, task| task.key.profile_id != profile_id);
        self.cache
            .invalidate_where(|key| key.profile_id == profile_id);
        self.root(profile_id)
            .ok_or_else(|| "Connection no longer exists.".into())
    }
    pub fn root(&self, profile_id: &str) -> Option<Key> {
        let t = self.targets.get(profile_id)?;
        Some(Key {
            profile_id: profile_id.into(),
            fingerprint: t.fingerprint.clone(),
            generation: t.generation,
            database: if t.profile.engine == DatabaseEngine::Sqlite {
                "main".into()
            } else {
                t.profile.database.clone()
            },
            scope: Scope::Databases,
        })
    }
    pub fn engine(&self, key: &Key) -> Option<DatabaseEngine> {
        self.authorized(key)
            .then(|| self.targets.get(&key.profile_id).unwrap().profile.engine)
    }
    pub fn profile(&self, key: &Key) -> Option<ConnectionProfile> {
        if !self.authorized(key) {
            return None;
        }
        let mut p = self.targets.get(&key.profile_id)?.profile.clone();
        p.database = key.database.clone();
        Some(p)
    }
    pub fn authorized(&self, key: &Key) -> bool {
        authorized(&self.targets, key)
    }
    pub fn branch(&self, key: &Key) -> Option<&Branch> {
        self.branches.get(key)
    }
    pub fn stats(&self) -> CacheStats {
        self.cache.stats()
    }
    pub fn pending(&self) -> usize {
        self.queue.len() + self.disk_waiting.len()
    }
    pub fn running(&self) -> usize {
        self.workers
            .iter()
            .flatten()
            .filter(|w| w.task.is_some())
            .count()
    }
    pub fn snapshot(&self, key: &Key) -> Option<Lease> {
        self.branch(key)?.snapshot.clone()
    }
    pub fn collapse(&mut self, key: &Key) {
        self.branches.retain(|k, _| !k.inside(key));
        self.queue.retain(|t| !t.key.inside(key));
        self.disk_waiting.retain(|_, t| !t.key.inside(key));
    }
    pub fn clear(&mut self) {
        self.invalidate_disk(None);
        self.disk_waiting.clear();
        self.branches.clear();
        self.queue.clear();
        self.cache.clear();
        self.notice = None;
    }
    pub fn expand(
        &mut self,
        key: Key,
        request: ConnectionRequest,
        refresh: bool,
    ) -> Result<(), String> {
        if !self.authorized(&key) {
            return Err("This connection changed. Reopen its branch.".into());
        }
        let expected = self.profile(&key).ok_or("Connection no longer exists.")?;
        let identity = StoredExecutionTarget::capture(&request.profile, &key.database)
            .map_err(|e| e.to_string())?;
        if identity.endpoint_fingerprint != key.fingerprint
            || request.profile.id != key.profile_id
            || request.profile.engine != expected.engine
            || request.profile.database != key.database
        {
            return Err("Metadata request target does not match the captured branch.".into());
        }
        if !refresh && self.branches.contains_key(&key) {
            return Ok(());
        }
        if !self.branches.contains_key(&key) && self.branches.len() >= MAX_BRANCHES {
            return Err("256 expanded metadata branches reached. Collapse a branch first.".into());
        }
        if !refresh && let Some(snapshot) = self.cache.get(&key) {
            self.branches.insert(
                key,
                Branch {
                    snapshot: Some(snapshot),
                    error: None,
                    loading: false,
                    ticket: 0,
                },
            );
            return Ok(());
        }
        if self.pending() >= MAX_PENDING {
            return Err("Metadata queue full (16). Wait for a probe to finish, then retry.".into());
        }
        let ticket = self.next_id()?;
        if refresh {
            self.invalidate_disk(Some(&key));
            self.collapse(&key);
            self.cache.invalidate_where(|cached| cached.inside(&key));
        }
        self.queue.retain(|t| t.key != key);
        // Release the displayed lease before invalidation. External inspectors may
        // still pin it; admission will truthfully reject if the budget stays full.
        self.branches.insert(
            key.clone(),
            Branch {
                snapshot: None,
                error: None,
                loading: true,
                ticket,
            },
        );
        if refresh {
            self.cache.invalidate(&key);
        }
        let task = Task {
            key,
            ticket,
            request: Arc::new(request),
        };
        if !refresh
            && let Some(disk) = &self.disk
            && let Some(key) = stored_key(&task.key)
            && disk.load(ticket, key)
        {
            self.disk_waiting.insert(ticket, task);
        } else {
            self.queue.push_back(task);
        }
        Ok(())
    }
    fn current(&self, task: &Task) -> bool {
        self.authorized(&task.key)
            && self
                .branches
                .get(&task.key)
                .is_some_and(|b| b.ticket == task.ticket && b.loading)
    }
    fn finish(&mut self, task: Task, result: Result<(Data, bool), String>) {
        if !self.current(&task) {
            return;
        }
        let result = result.and_then(|(data, truncated)| {
            if !matches!(
                (&task.key.scope, &data),
                (Scope::Databases, Data::Databases(_))
                    | (Scope::Schemas, Data::Schemas(_))
                    | (Scope::Objects(_), Data::Objects(_))
                    | (Scope::Columns(_), Data::Columns(_))
                    | (Scope::Details(_), Data::Details { .. })
            ) {
                return Err("Metadata response scope mismatch.".into());
            }
            let weight = owned_bytes(&task.key, &data);
            self.cache
                .insert(
                    task.key.clone(),
                    Snapshot {
                        data,
                        truncated,
                        fetched_at: Instant::now(),
                        historical: false,
                        captured_unix_ms: dbide_catalog_store::now_unix_ms(),
                    },
                    weight,
                    TTL,
                )
                .map_err(|e| e.to_string())
        });
        if let Some(branch) = self.branches.get_mut(&task.key) {
            branch.loading = false;
            match result {
                Ok(snapshot) => {
                    if self.disk.is_some()
                        && snapshot.accounted_bytes() > dbide_catalog_store::MAX_SHARD_BYTES
                    {
                        self.notice = Some(
                            "Catalog snapshot exceeds disk admission budget; memory only.".into(),
                        );
                    } else if let Some(disk) = &self.disk
                        && let Some(key) = stored_key(&task.key)
                        && let Some(value) = stored_snapshot(&snapshot)
                        && !disk.save(key, value)
                    {
                        self.notice =
                            Some("Catalog disk queue full; snapshot retained in memory.".into());
                    }
                    branch.snapshot = Some(snapshot);
                    branch.error = None;
                }
                Err(error) => {
                    branch.snapshot = None;
                    branch.error = Some(error);
                }
            }
        }
    }
    /// Called by one UI timer. Only two slots and 32 events total are serviced.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for _ in 0..64 {
            let Some(reply) = self.disk.as_ref().and_then(|disk| disk.try_next()) else {
                break;
            };
            match reply {
                dbide_catalog_store::Reply::Diagnostic(error) => {
                    self.notice = Some(error);
                    changed = true;
                }
                dbide_catalog_store::Reply::Loaded {
                    ticket,
                    key,
                    snapshot,
                } => {
                    let Some(task) = self.disk_waiting.remove(&ticket) else {
                        continue;
                    };
                    if !self.current(&task) || stored_key(&task.key).as_ref() != Some(&key) {
                        continue;
                    }
                    match snapshot {
                        Ok(Some(stored)) => {
                            let captured_unix_ms = stored.captured_unix_ms;
                            let truncated = stored.truncated || stored.fields_partial;
                            let data = restored_data(stored.data);
                            let weight = owned_bytes(&task.key, &data);
                            let result = self.cache.insert(
                                task.key.clone(),
                                Snapshot {
                                    data,
                                    truncated,
                                    fetched_at: Instant::now(),
                                    historical: true,
                                    captured_unix_ms,
                                },
                                weight,
                                TTL,
                            );
                            match result {
                                Ok(snapshot) => {
                                    if let Some(branch) = self.branches.get_mut(&task.key) {
                                        branch.snapshot = Some(snapshot);
                                        branch.loading = false;
                                        branch.error = None;
                                    }
                                }
                                Err(error) => {
                                    self.notice = Some(error.to_string());
                                    self.queue.push_back(task);
                                }
                            }
                        }
                        Ok(None) => self.queue.push_back(task),
                        Err(error) => {
                            self.notice = Some(error);
                            self.queue.push_back(task);
                        }
                    }
                    changed = true;
                }
            }
        }
        for ix in 0..WORKERS {
            if self.workers[ix].as_ref().is_none_or(|w| w.task.is_none())
                && let Some(task) = self.queue.pop_front()
            {
                if !self.current(&task) {
                    continue;
                }
                if self.workers[ix].is_none() {
                    match DatabaseBridge::new() {
                        Ok(bridge) => {
                            self.workers[ix] = Some(Worker {
                                bridge,
                                task: None,
                                configuring: None,
                                operation: None,
                                details: None,
                            })
                        }
                        Err(e) => {
                            self.finish(task, Err(e.to_string()));
                            changed = true;
                            continue;
                        }
                    }
                }
                let worker = self.workers[ix].as_mut().unwrap();
                if let Scope::Details(relation) = &task.key.scope {
                    let request = task.request.clone();
                    let relation = relation.clone();
                    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
                    match std::thread::Builder::new()
                        .name("navigator-detail".into())
                        .spawn(move || {
                            let _ = sender.send(capture_details(&request, &relation));
                        }) {
                        Ok(_) => {
                            worker.details = Some(receiver);
                            worker.task = Some(task);
                        }
                        Err(error) => {
                            self.finish(task, Err(error.to_string()));
                            changed = true;
                        }
                    }
                    continue;
                }
                match worker.bridge.configure((*task.request).clone()) {
                    Ok(op) => {
                        worker.configuring = Some(op);
                        worker.operation = None;
                        worker.task = Some(task);
                    }
                    Err(e) => {
                        self.finish(task, Err(format!("Metadata configuration rejected: {e:?}")));
                        changed = true;
                    }
                }
            }
        }
        for ix in 0..WORKERS {
            let Some(worker) = self.workers[ix].as_mut() else {
                continue;
            };
            if let Some(receiver) = worker.details.as_ref() {
                let result = match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Detailed metadata worker stopped.".into()))
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                };
                if let Some(result) = result {
                    worker.details = None;
                    if let Some(task) = worker.task.take() {
                        self.finish(task, result);
                        changed = true;
                    }
                }
            }
        }
        for step in 0..32 {
            let ix = step % WORKERS;
            let Some(worker) = self.workers[ix].as_mut() else {
                continue;
            };
            if worker.details.is_some() {
                continue;
            }
            let Some(event) = worker.bridge.try_next() else {
                continue;
            };
            let Some(task) = worker.task.as_ref() else {
                continue;
            };
            let mut terminal = None;
            match event {
                BridgeEvent::Configured { operation } if worker.configuring == Some(operation) => {
                    worker.configuring = None;
                    if !authorized(&self.targets, &task.key)
                        || !self
                            .branches
                            .get(&task.key)
                            .is_some_and(|b| b.loading && b.ticket == task.ticket)
                    {
                        let task = worker.task.take().unwrap();
                        self.finish(task, Err("Obsolete metadata request.".into()));
                        changed = true;
                        continue;
                    }
                    let submitted = match &task.key.scope {
                        Scope::Databases => worker.bridge.list_databases(),
                        Scope::Schemas => worker.bridge.list_schemas(),
                        Scope::Objects(schema) => worker.bridge.list_objects(schema.clone()),
                        Scope::Details(_) => unreachable!("detail slots do not configure a bridge"),
                        Scope::Columns(r) => worker.bridge.list_columns(RelationRef {
                            schema: r.schema.clone(),
                            name: r.name.clone(),
                        }),
                    };
                    match submitted {
                        Ok(op) => worker.operation = Some(op),
                        Err(e) => terminal = Some(Err(format!("Metadata request rejected: {e:?}"))),
                    }
                }
                BridgeEvent::DatabasesLoaded {
                    operation,
                    databases,
                    truncated,
                } if worker.operation == Some(operation) => {
                    terminal = Some(Ok((Data::Databases(databases), truncated)))
                }
                BridgeEvent::SchemasLoaded {
                    operation,
                    schemas,
                    truncated,
                } if worker.operation == Some(operation) => {
                    terminal = Some(Ok((Data::Schemas(schemas), truncated)))
                }
                BridgeEvent::ObjectsLoaded {
                    operation,
                    schema,
                    objects,
                    truncated,
                } if worker.operation == Some(operation) => {
                    terminal = Some(validate_scope(
                        &task.key.scope,
                        Some(&schema),
                        None,
                        Data::Objects(objects),
                        truncated,
                    ));
                }
                BridgeEvent::ColumnsLoaded {
                    operation,
                    relation,
                    columns,
                    truncated,
                } if worker.operation == Some(operation) => {
                    terminal = Some(validate_scope(
                        &task.key.scope,
                        None,
                        Some(&relation),
                        Data::Columns(columns),
                        truncated,
                    ));
                }
                BridgeEvent::ConnectionFailed {
                    operation,
                    diagnostic,
                } if worker.operation == Some(operation)
                    || worker.configuring == Some(operation) =>
                {
                    terminal = Some(Err(format!("{} · {}", diagnostic.message, diagnostic.hint)))
                }
                BridgeEvent::Error { operation, message }
                    if operation.is_none()
                        || operation == worker.operation
                        || operation == worker.configuring =>
                {
                    terminal = Some(Err(message))
                }
                _ => {}
            }
            if let Some(result) = terminal {
                let task = worker.task.take().unwrap();
                worker.operation = None;
                worker.configuring = None;
                self.finish(task, result);
                changed = true;
            }
        }
        if self.last_tick.elapsed() >= Duration::from_secs(1) {
            self.last_tick = Instant::now();
            self.cache.prune_expired();
            changed |= !self.branches.is_empty();
        }
        changed
    }
}
fn stored_key(key: &Key) -> Option<dbide_catalog_store::Key> {
    use dbide_catalog_store as store;
    let scope = match &key.scope {
        Scope::Databases => store::Scope::Databases,
        Scope::Schemas => store::Scope::Schemas,
        Scope::Objects(schema) => store::Scope::Objects(schema.clone()),
        Scope::Columns(r) => store::Scope::Columns(store::Relation {
            schema: r.schema.clone(),
            name: r.name.clone(),
            kind: r.kind.clone(),
        }),
        Scope::Details(r) => store::Scope::Details(store::Relation {
            schema: r.schema.clone(),
            name: r.name.clone(),
            kind: r.kind.clone(),
        }),
    };
    Some(store::Key {
        profile_id: key.profile_id.clone(),
        fingerprint: key.fingerprint.clone(),
        database: key.database.clone(),
        scope,
    })
}
fn stored_snapshot(snapshot: &Snapshot) -> Option<dbide_catalog_store::Snapshot> {
    use dbide_catalog_store as s;
    let data = match &snapshot.data {
        Data::Databases(v) => s::Data::Databases(
            v.iter()
                .map(|d| s::Database {
                    name: d.name.clone(),
                    accessible: d.accessible,
                    current: d.current,
                })
                .collect(),
        ),
        Data::Schemas(v) => s::Data::Schemas(v.iter().map(|d| d.name.clone()).collect()),
        Data::Objects(v) => s::Data::Objects(
            v.iter()
                .map(|d| s::Object {
                    schema: d.schema.clone(),
                    name: d.name.clone(),
                    kind: d.kind.clone(),
                })
                .collect(),
        ),
        Data::Columns(v) => s::Data::Columns(
            v.iter()
                .map(|d| s::Column {
                    name: d.name.clone(),
                    data_type: d.data_type.clone(),
                    nullable: d.nullable,
                    primary_key_ordinal: d.primary_key_ordinal,
                    foreign_key: d.foreign_key,
                    default: d.default.clone(),
                    extra: d.extra.clone(),
                })
                .collect(),
        ),
        Data::Details { table, view } => s::Data::Details {
            table: table.clone(),
            view: view.clone(),
        },
    };
    Some(s::Snapshot {
        captured_unix_ms: snapshot.captured_unix_ms,
        truncated: snapshot.truncated,
        fields_partial: false,
        data,
    })
}
fn restored_data(data: dbide_catalog_store::Data) -> Data {
    use dbide_catalog_store::Data as S;
    match data {
        S::Details { table, view } => Data::Details { table, view },
        S::Databases(v) => Data::Databases(
            v.into_iter()
                .map(|d| DatabaseInfo {
                    name: d.name,
                    accessible: d.accessible,
                    current: d.current,
                })
                .collect(),
        ),
        S::Schemas(v) => Data::Schemas(v.into_iter().map(|name| SchemaInfo { name }).collect()),
        S::Objects(v) => Data::Objects(
            v.into_iter()
                .map(|d| SchemaObjectInfo {
                    schema: d.schema,
                    name: d.name,
                    kind: d.kind,
                })
                .collect(),
        ),
        S::Columns(v) => Data::Columns(
            v.into_iter()
                .map(|d| SchemaColumn {
                    name: d.name,
                    data_type: d.data_type,
                    nullable: d.nullable,
                    primary_key_ordinal: d.primary_key_ordinal,
                    foreign_key: d.foreign_key,
                    default: d.default,
                    extra: d.extra,
                })
                .collect(),
        ),
    }
}

fn authorized(targets: &HashMap<String, Target>, key: &Key) -> bool {
    targets
        .get(&key.profile_id)
        .is_some_and(|t| t.generation == key.generation && t.fingerprint == key.fingerprint)
}
/// Recursive retained capacity estimate. Cache adds inline payload/key/bookkeeping.
/// This is an accounted payload budget, not a process RSS ceiling.
pub fn owned_bytes(key: &Key, data: &Data) -> usize {
    let scope = match &key.scope {
        Scope::Objects(s) => s.capacity(),
        Scope::Columns(r) | Scope::Details(r) => r
            .schema
            .capacity()
            .saturating_add(r.name.capacity())
            .saturating_add(r.kind.capacity()),
        _ => 0,
    };
    let base = key
        .profile_id
        .capacity()
        .saturating_add(key.fingerprint.capacity())
        .saturating_add(key.database.capacity())
        .saturating_add(scope);
    let strings = |v: &[&String]| {
        v.iter()
            .fold(0usize, |sum, s| sum.saturating_add(s.capacity()))
    };
    let nested = match data {
        Data::Databases(v) => v.iter().fold(
            v.capacity().saturating_mul(size_of::<DatabaseInfo>()),
            |n, x| n.saturating_add(x.name.capacity()),
        ),
        Data::Schemas(v) => v.iter().fold(
            v.capacity().saturating_mul(size_of::<SchemaInfo>()),
            |n, x| n.saturating_add(x.name.capacity()),
        ),
        Data::Objects(v) => v.iter().fold(
            v.capacity().saturating_mul(size_of::<SchemaObjectInfo>()),
            |n, x| n.saturating_add(strings(&[&x.schema, &x.name, &x.kind])),
        ),
        Data::Columns(v) => v.iter().fold(
            v.capacity().saturating_mul(size_of::<SchemaColumn>()),
            |n, x| {
                n.saturating_add(strings(&[&x.name, &x.data_type]))
                    .saturating_add(x.default.as_ref().map_or(0, String::capacity))
                    .saturating_add(x.extra.as_ref().map_or(0, String::capacity))
            },
        ),
        Data::Details { table, view } => table
            .as_ref()
            .map_or(0, |t| {
                size_of::<dbide_schema::Table>().saturating_add(table_bytes(t))
            })
            .saturating_add(view.as_ref().map_or(0, |v| {
                size_of::<dbide_schema::View>()
                    .saturating_add(v.name.capacity())
                    .saturating_add(v.definition.capacity())
            })),
    };
    base.saturating_add(nested)
}

fn validate_scope(
    scope: &Scope,
    schema: Option<&str>,
    relation: Option<&RelationRef>,
    data: Data,
    truncated: bool,
) -> MetadataResult {
    let matches = match scope {
        Scope::Objects(expected) => schema == Some(expected.as_str()),
        Scope::Columns(expected) => {
            relation.is_some_and(|r| r.schema == expected.schema && r.name == expected.name)
        }
        _ => false,
    };
    if matches {
        Ok((data, truncated))
    } else {
        Err("Metadata response scope mismatch; refresh this branch.".into())
    }
}
fn string_vec_bytes(v: &Vec<String>) -> usize {
    v.iter()
        .fold(v.capacity().saturating_mul(size_of::<String>()), |n, s| {
            n.saturating_add(s.capacity())
        })
}
fn table_bytes(t: &dbide_schema::Table) -> usize {
    let mut n = t
        .name
        .capacity()
        .saturating_add(string_vec_bytes(&t.unsupported_features));
    n = n.saturating_add(
        t.columns
            .capacity()
            .saturating_mul(size_of::<dbide_schema::Column>()),
    );
    for c in &t.columns {
        n = n
            .saturating_add(c.name.capacity())
            .saturating_add(c.data_type.capacity())
            .saturating_add(c.default.as_ref().map_or(0, String::capacity))
            .saturating_add(c.generated.as_ref().map_or(0, String::capacity));
    }
    if let Some(pk) = &t.primary_key {
        n = n
            .saturating_add(pk.name.as_ref().map_or(0, String::capacity))
            .saturating_add(string_vec_bytes(&pk.columns));
    }
    n = n.saturating_add(
        t.foreign_keys
            .capacity()
            .saturating_mul(size_of::<dbide_schema::ForeignKey>()),
    );
    for fk in &t.foreign_keys {
        n = n
            .saturating_add(fk.name.as_ref().map_or(0, String::capacity))
            .saturating_add(string_vec_bytes(&fk.columns))
            .saturating_add(string_vec_bytes(&fk.referenced_columns))
            .saturating_add(fk.references.schema.capacity())
            .saturating_add(fk.references.name.capacity());
    }
    n = n.saturating_add(
        t.indexes
            .capacity()
            .saturating_mul(size_of::<dbide_schema::Index>()),
    );
    for i in &t.indexes {
        n = n
            .saturating_add(i.name.capacity())
            .saturating_add(i.method.capacity())
            .saturating_add(i.predicate.as_ref().map_or(0, String::capacity))
            .saturating_add(string_vec_bytes(&i.columns))
            .saturating_add(string_vec_bytes(&i.expressions))
            .saturating_add(string_vec_bytes(&i.include))
            .saturating_add(string_vec_bytes(&i.unsupported_features));
    }
    n
}
fn capture_details(request: &ConnectionRequest, relation: &Relation) -> MetadataResult {
    if request.profile.engine != DatabaseEngine::PostgreSql {
        return Err("Detailed indexes / definitions currently require PostgreSQL; basic fields remain available.".into());
    }
    let mut probe = request.clone();
    probe.profile.connect_timeout_secs = probe.profile.connect_timeout_secs.min(4);
    let request = &probe;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let limits = dbide_workflows::CaptureLimits {
        max_records: 4096,
        max_relations: 500,
        max_columns: 2048,
        max_bytes: 1_000_000,
        timeout_secs: 4,
    };
    if relation.kind == "table" {
        let captured = runtime
            .block_on(dbide_workflows::capture_target_table_for_edit(
                request,
                &relation.schema,
                &relation.name,
                limits,
            ))
            .map_err(|e| e.to_string())?;
        let table = captured
            .snapshot
            .schemas
            .into_iter()
            .find(|s| s.name == relation.schema)
            .and_then(|s| s.tables.into_iter().find(|t| t.name == relation.name))
            .ok_or("Object no longer present in captured catalog.")?;
        Ok((
            Data::Details {
                table: Some(Box::new(table)),
                view: None,
            },
            false,
        ))
    } else {
        let captured = runtime
            .block_on(dbide_workflows::capture_view(
                request,
                &relation.schema,
                &relation.name,
                limits,
            ))
            .map_err(|e| e.to_string())?;
        Ok((
            Data::Details {
                table: None,
                view: Some(Box::new(captured.view)),
            },
            false,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (Navigator, ConnectionProfile, Key) {
        let mut navigator = Navigator::default();
        let profile = ConnectionProfile::fixture();
        navigator.sync_profiles(std::slice::from_ref(&profile));
        let key = navigator.root(&profile.id).unwrap();
        (navigator, profile, key)
    }
    fn request(profile: &ConnectionProfile, key: &Key) -> ConnectionRequest {
        let mut profile = profile.clone();
        profile.database = key.database.clone();
        ConnectionRequest {
            profile,
            password: "test-session-only".into(),
        }
    }
    fn enqueue(n: &mut Navigator, p: &ConnectionProfile, k: &Key) -> Task {
        n.expand(k.clone(), request(p, k), true).unwrap();
        n.queue.pop_front().unwrap()
    }
    fn databases(name: &str) -> Data {
        Data::Databases(vec![DatabaseInfo {
            name: name.into(),
            accessible: true,
            current: false,
        }])
    }
    #[test]
    fn same_named_databases_and_schemas_never_share_snapshots() {
        let (mut n, p, k) = setup();
        let a = k.child("a", Scope::Objects("public".into()));
        let b = k.child("b", Scope::Objects("public".into()));
        let task = enqueue(&mut n, &p, &a);
        n.finish(
            task,
            Ok((
                Data::Objects(vec![SchemaObjectInfo {
                    schema: "public".into(),
                    name: "a_only".into(),
                    kind: "table".into(),
                }]),
                false,
            )),
        );
        let task = enqueue(&mut n, &p, &b);
        n.finish(task, Ok((Data::Objects(vec![]), true)));
        assert_eq!(n.snapshot(&a).unwrap().len(), 1);
        assert_eq!(n.snapshot(&b).unwrap().len(), 0);
        assert!(n.snapshot(&b).unwrap().truncated);
        let wrong = request(&p, &a);
        assert!(n.expand(b.clone(), wrong, true).is_err());
        assert_ne!(a, k.child("a", Scope::Objects("another".into())));
    }
    #[test]
    fn stale_profile_ticket_collapse_and_cache_clear_cannot_publish() {
        let (mut n, mut p, k) = setup();
        let stale = enqueue(&mut n, &p, &k);
        p.host = "changed.invalid".into();
        n.sync_profiles(std::slice::from_ref(&p));
        n.finish(stale, Ok((databases("old"), false)));
        assert!(n.branch(&k).is_none());
        assert_eq!(n.stats().resident_entries, 0);
        let k = n.root(&p.id).unwrap();
        let stale = enqueue(&mut n, &p, &k);
        let fresh = enqueue(&mut n, &p, &k);
        n.finish(stale, Ok((databases("old"), false)));
        assert!(n.branch(&k).unwrap().loading);
        n.finish(fresh, Ok((databases("fresh"), false)));
        assert!(matches!(&n.snapshot(&k).unwrap().data,Data::Databases(v) if v[0].name=="fresh"));
        n.collapse(&k);
        let stale = enqueue(&mut n, &p, &k);
        n.clear();
        n.finish(stale, Ok((databases("late"), false)));
        assert!(n.branch(&k).is_none());
        assert_eq!(n.stats().resident_entries, 0);
        n.sync_profiles(&[]);
        assert!(n.root(&p.id).is_none());
        assert!(n.queue.is_empty());
    }
    #[test]
    fn profile_removal_readd_and_connection_refresh_get_new_generations() {
        let (mut n, p, k) = setup();
        let next = n.refresh_connection(&p.id).unwrap();
        assert_ne!(k.generation, next.generation);
        assert!(!n.authorized(&k));
        n.sync_profiles(&[]);
        n.sync_profiles(std::slice::from_ref(&p));
        assert_ne!(next.generation, n.root(&p.id).unwrap().generation);
        let current = n.root(&p.id).unwrap();
        let mut renamed = p.clone();
        renamed.name = "rename".into();
        n.sync_profiles(&[renamed]);
        assert_eq!(current, n.root(&p.id).unwrap());
        assert_eq!(n.profile(&current).unwrap().name, "rename");
    }
    #[test]
    fn queue_and_expanded_limits_reject_without_workers_or_silent_eviction() {
        let (mut n, p, k) = setup();
        for ix in 0..MAX_PENDING {
            let key = k.child(&format!("db{ix}"), Scope::Schemas);
            n.expand(key.clone(), request(&p, &key), false).unwrap();
        }
        let overflow = k.child("overflow", Scope::Schemas);
        assert!(
            n.expand(overflow.clone(), request(&p, &overflow), false)
                .unwrap_err()
                .contains("queue full")
        );
        assert_eq!(n.pending(), MAX_PENDING);
        assert_eq!(n.running(), 0);
        assert!(n.workers.iter().all(Option::is_none));
        n.clear();
        for ix in 0..MAX_BRANCHES {
            let key = k.child(&format!("db{ix}"), Scope::Schemas);
            n.branches.insert(
                key,
                Branch {
                    snapshot: None,
                    error: None,
                    loading: false,
                    ticket: 0,
                },
            );
        }
        assert!(
            n.expand(overflow.clone(), request(&p, &overflow), false)
                .unwrap_err()
                .contains("256")
        );
        assert_eq!(n.branches.len(), MAX_BRANCHES);
    }
    #[test]
    fn retained_capacities_and_inspector_leases_are_charged_on_refresh() {
        let (mut n, p, k) = setup();
        let mut value = String::with_capacity(4096);
        value.push_str("small");
        let data = Data::Databases(vec![DatabaseInfo {
            name: value,
            accessible: true,
            current: false,
        }]);
        assert!(owned_bytes(&k, &data) >= 4096);
        let task = enqueue(&mut n, &p, &k);
        n.finish(task, Ok((data, false)));
        let held = n.snapshot(&k).unwrap();
        let charged = held.accounted_bytes();
        let mut tight = Navigator::new(CacheConfig {
            max_accounted_bytes: charged + 100,
            max_live_entries: 1,
        });
        tight.sync_profiles(std::slice::from_ref(&p));
        let key = tight.root(&p.id).unwrap();
        let task = enqueue(&mut tight, &p, &key);
        tight.finish(task, Ok((databases("first"), false)));
        let lease = tight.snapshot(&key).unwrap();
        let task = enqueue(&mut tight, &p, &key);
        tight.finish(task, Ok((databases("second"), false)));
        assert!(
            tight
                .branch(&key)
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("live snapshots")
        );
        assert_eq!(tight.stats().live_entries, 1);
        drop(lease);
        let task = enqueue(&mut tight, &p, &key);
        tight.finish(task, Ok((databases("third"), false)));
        assert!(tight.snapshot(&key).is_some());
    }
    #[test]
    fn scopes_reject_mismatched_payload_and_stale_snapshot_is_explicit() {
        let (mut n, p, k) = setup();
        let task = enqueue(&mut n, &p, &k);
        n.finish(task, Ok((Data::Columns(vec![]), false)));
        assert!(
            n.branch(&k)
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("scope mismatch")
        );
        let snapshot = Snapshot {
            data: databases("x"),
            truncated: true,
            fetched_at: Instant::now() - TTL - Duration::from_secs(1),
            historical: false,
            captured_unix_ms: 0,
        };
        assert!(snapshot.stale());
        assert!(snapshot.truncated);
    }
    #[test]
    fn sqlite_logical_main_and_mysql_database_layer_are_exact() {
        let (mut n, mut p, _) = setup();
        p.engine = DatabaseEngine::Sqlite;
        p.sqlite_path = Some("/tmp/owned-navigation-test.sqlite".into());
        n.sync_profiles(std::slice::from_ref(&p));
        let root = n.root(&p.id).unwrap();
        assert_eq!(root.database, "main");
        assert_eq!(n.profile(&root).unwrap().database, "main");
        p.engine = DatabaseEngine::MySql;
        p.port = 3306;
        p.database = "actual_database".into();
        n.sync_profiles(std::slice::from_ref(&p));
        let root = n.root(&p.id).unwrap();
        let objects = root.child("actual_database", Scope::Objects("actual_database".into()));
        assert_eq!(n.profile(&objects).unwrap().database, "actual_database");
    }
    #[test]
    fn matching_operation_wrong_relation_is_terminal_not_loading_forever() {
        let (mut n, p, root) = setup();
        let relation = Relation {
            schema: "one".into(),
            name: "same".into(),
            kind: "table".into(),
        };
        let key = root.child("db", Scope::Columns(relation));
        let task = enqueue(&mut n, &p, &key);
        let result = validate_scope(
            &key.scope,
            None,
            Some(&RelationRef {
                schema: "two".into(),
                name: "same".into(),
            }),
            Data::Columns(vec![]),
            false,
        );
        n.finish(task, result);
        assert!(!n.branch(&key).unwrap().loading);
        assert!(n.branch(&key).unwrap().error.is_some());
        assert_eq!(n.stats().resident_entries, 0);
        assert!(
            validate_scope(
                &Scope::Objects("one".into()),
                Some("two"),
                None,
                Data::Objects(vec![]),
                false
            )
            .is_err()
        );
    }
    #[test]
    fn refresh_invalidates_collapsed_children_and_keeps_other_database_cache() {
        let (mut n, p, k) = setup();
        let a = k.child("a", Scope::Objects("public".into()));
        let b = k.child("b", Scope::Objects("public".into()));
        let relation = Relation {
            schema: "public".into(),
            name: "same".into(),
            kind: "table".into(),
        };
        let child = a.child("a", Scope::Columns(relation));
        let task = enqueue(&mut n, &p, &child);
        n.finish(task, Ok((Data::Columns(vec![]), false)));
        n.collapse(&child);
        let task = enqueue(&mut n, &p, &b);
        n.finish(task, Ok((Data::Objects(vec![]), false)));
        let b_snapshot = n.snapshot(&b).unwrap();
        let _task = enqueue(&mut n, &p, &a);
        assert!(n.cache.get(&child).is_none());
        assert!(n.cache.get(&b).is_some());
        assert_eq!(n.snapshot(&b).unwrap().fetched_at, b_snapshot.fetched_at);
        n.expand(child.clone(), request(&p, &child), false).unwrap();
        assert!(n.branch(&child).unwrap().loading);
    }
    #[test]
    fn refreshing_database_schema_list_preserves_connection_root_and_other_databases() {
        let (mut n, p, root) = setup();
        let database = root.child(&root.database, Scope::Schemas);
        let other = root.child("other_database", Scope::Schemas);
        let task = enqueue(&mut n, &p, &root);
        n.finish(task, Ok((databases(&root.database), false)));
        let task = enqueue(&mut n, &p, &database);
        n.finish(task, Ok((Data::Schemas(vec![]), false)));
        let task = enqueue(&mut n, &p, &other);
        n.finish(task, Ok((Data::Schemas(vec![]), false)));
        assert!(!root.inside(&database));
        let _refresh = enqueue(&mut n, &p, &database);
        assert!(n.branch(&root).is_some());
        assert!(n.snapshot(&root).is_some());
        assert!(n.snapshot(&other).is_some());
        assert!(n.branch(&database).unwrap().loading);
    }
    fn disk_roundtrip(n: &Navigator, key: &Key) -> Option<dbide_catalog_store::Snapshot> {
        let disk = n.disk.as_ref().unwrap();
        assert!(disk.load(u64::MAX, stored_key(key).unwrap()));
        let start = Instant::now();
        loop {
            if let Some(reply) = disk.try_next() {
                match reply {
                    dbide_catalog_store::Reply::Loaded {
                        ticket, snapshot, ..
                    } => {
                        assert_eq!(ticket, u64::MAX);
                        return snapshot.unwrap();
                    }
                    dbide_catalog_store::Reply::Diagnostic(error) => panic!("{error}"),
                }
            }
            assert!(start.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn disk_restore_is_lazy_historical_and_does_not_start_database_workers() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("catalog-cache");
        let mut n = Navigator::with_store(CacheConfig::default(), path.clone());
        let p = ConnectionProfile::fixture();
        n.sync_profiles(std::slice::from_ref(&p));
        assert!(!path.exists());
        let root = n.root(&p.id).unwrap();
        let key = root.child(&root.database, Scope::Objects("public".into()));
        let task = enqueue(&mut n, &p, &key);
        n.finish(
            task,
            Ok((
                Data::Objects(vec![SchemaObjectInfo {
                    schema: "public".into(),
                    name: "owned_table".into(),
                    kind: "table".into(),
                }]),
                true,
            )),
        );
        assert!(disk_roundtrip(&n, &key).is_some());
        drop(n);
        let mut restored = Navigator::with_store(CacheConfig::default(), path);
        restored.next_id().unwrap(); // Persistence deliberately excludes ephemeral generation.
        restored.sync_profiles(std::slice::from_ref(&p));
        assert_eq!(restored.stats().resident_entries, 0);
        let root = restored.root(&p.id).unwrap();
        let key = root.child(&root.database, Scope::Objects("public".into()));
        restored
            .expand(key.clone(), request(&p, &key), false)
            .unwrap();
        let start = Instant::now();
        while restored.snapshot(&key).is_none() {
            restored.poll();
            assert_eq!(
                restored.running(),
                0,
                "historical hit must not open a database"
            );
            assert!(start.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(1));
        }
        let snapshot = restored.snapshot(&key).unwrap();
        assert!(snapshot.historical && snapshot.stale() && snapshot.truncated);
        assert!(snapshot.captured_unix_ms > 0);
        assert_eq!(snapshot.len(), 1);
    }
    #[test]
    fn disk_clear_and_refresh_reject_obsolete_results_and_preserve_other_branches() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("catalog-cache");
        let mut n = Navigator::with_store(CacheConfig::default(), path);
        let p = ConnectionProfile::fixture();
        n.sync_profiles(std::slice::from_ref(&p));
        let root = n.root(&p.id).unwrap();
        let a = root.child("a", Scope::Schemas);
        let b = root.child("b", Scope::Schemas);
        for key in [&a, &b] {
            let task = enqueue(&mut n, &p, key);
            n.finish(task, Ok((Data::Schemas(vec![]), false)));
        }
        assert!(disk_roundtrip(&n, &a).is_some());
        let old = enqueue(&mut n, &p, &a);
        let _new = enqueue(&mut n, &p, &a);
        n.finish(old, Ok((Data::Schemas(vec![]), false)));
        assert!(disk_roundtrip(&n, &a).is_none());
        assert!(disk_roundtrip(&n, &b).is_some());
        n.clear();
        assert!(disk_roundtrip(&n, &b).is_none());
    }
    #[test]
    fn persisted_columns_preserve_default_and_identity_descriptors() {
        let column = SchemaColumn {
            name: "id".into(),
            data_type: "bigint".into(),
            nullable: false,
            primary_key_ordinal: Some(1),
            foreign_key: true,
            default: Some("nextval('owned_sequence')".into()),
            extra: Some("generated identity".into()),
        };
        let snapshot = Snapshot {
            data: Data::Columns(vec![column.clone()]),
            truncated: false,
            fetched_at: Instant::now(),
            historical: false,
            captured_unix_ms: 42,
        };
        let stored = stored_snapshot(&snapshot).unwrap();
        assert!(!stored.fields_partial);
        let Data::Columns(columns) = restored_data(stored.data) else {
            panic!("columns expected");
        };
        assert_eq!(columns, vec![column]);
        let details = Snapshot {
            data: Data::Details {
                table: None,
                view: None,
            },
            ..snapshot
        };
        assert!(matches!(
            stored_snapshot(&details).unwrap().data,
            dbide_catalog_store::Data::Details { .. }
        ));
    }
    #[test]
    fn persisted_detail_shard_restores_historical_indexes_without_database_io() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("catalog-cache");
        let mut n = Navigator::with_store(CacheConfig::default(), path);
        let p = ConnectionProfile::fixture();
        n.sync_profiles(std::slice::from_ref(&p));
        let root = n.root(&p.id).unwrap();
        let key = root.child(
            &root.database,
            Scope::Details(Relation {
                schema: "public".into(),
                name: "owned_table".into(),
                kind: "table".into(),
            }),
        );
        let table = dbide_schema::Table {
            name: "owned_table".into(),
            columns: vec![],
            primary_key: None,
            foreign_keys: vec![],
            indexes: vec![dbide_schema::Index {
                name: "owned_idx".into(),
                columns: vec!["id".into()],
                unique: true,
                method: "btree".into(),
                predicate: Some("id > 0".into()),
                expressions: vec![],
                include: vec![],
                constraint_owned: false,
                unsupported_features: vec![],
            }],
            unsupported_features: vec![],
        };
        let expected = table.clone();
        let task = enqueue(&mut n, &p, &key);
        n.finish(
            task,
            Ok((
                Data::Details {
                    table: Some(Box::new(table)),
                    view: None,
                },
                false,
            )),
        );
        assert!(disk_roundtrip(&n, &key).is_some());
        n.collapse(&key);
        n.cache.clear();
        n.expand(key.clone(), request(&p, &key), false).unwrap();
        let start = Instant::now();
        while n.snapshot(&key).is_none() {
            n.poll();
            assert_eq!(n.running(), 0);
            assert!(start.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(1));
        }
        let snapshot = n.snapshot(&key).unwrap();
        assert!(snapshot.historical && snapshot.stale());
        let Data::Details {
            table: Some(table),
            view: None,
        } = &snapshot.data
        else {
            panic!("table detail expected");
        };
        assert_eq!(**table, expected);
    }
}
