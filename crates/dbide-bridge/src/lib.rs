//! Nonblocking native database bridge. Credentials stay in the worker, never in URLs or events.
mod adapter;
use adapter::{AdapterSession as Session, CancelResult};
use dbide_driver_api::Limits;
pub use dbide_pg_spike::{
    CommitPolicy, ConnectionDiagnostic, ConnectionErrorKind, ConnectionTarget, ExecutionKey,
    ExecutionReport, Outcome, ServerTransactionState,
};
pub use dbide_pg_spike::{ParameterizedStatement, TableIdentityGuard};
use dbide_profiles::ConnectionRequest;
use std::{
    collections::HashMap,
    future::{Future, pending},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, watch};

const RESERVED_EVENTS: usize = 32;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperationId(pub u64);
#[derive(Debug, Clone, Copy)]
pub enum CancelTarget {
    Execution(ExecutionKey),
    Operation(OperationId),
}
impl From<ExecutionKey> for CancelTarget {
    fn from(key: ExecutionKey) -> Self {
        Self::Execution(key)
    }
}
impl From<OperationId> for CancelTarget {
    fn from(operation: OperationId) -> Self {
        Self::Operation(operation)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeSubmitError {
    QueueFull,
    WorkerStopped,
    SqlTooLarge,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key_ordinal: Option<u16>,
    pub foreign_key: bool,
    pub default: Option<String>,
    pub extra: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaObject {
    pub schema: String,
    pub name: String,
    pub kind: String,
    pub columns: Vec<SchemaColumn>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaInfo {
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaObjectInfo {
    pub schema: String,
    pub name: String,
    pub kind: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationRef {
    pub schema: String,
    pub name: String,
}
/// Advertised catalog access; discovery does not attempt a connection to each database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseInfo {
    pub name: String,
    pub accessible: bool,
    pub current: bool,
}
#[derive(Debug)]
pub enum BridgeEvent {
    CommitPolicyChanged {
        operation: OperationId,
        policy: CommitPolicy,
        transaction_state: ServerTransactionState,
    },
    Configured {
        operation: OperationId,
    },
    Connecting {
        operation: OperationId,
        attempt: u32,
        target: ConnectionTarget,
    },
    ConnectionFailed {
        operation: OperationId,
        diagnostic: ConnectionDiagnostic,
    },
    ConnectionTested {
        operation: OperationId,
    },
    SchemaLoaded {
        operation: OperationId,
        objects: Vec<SchemaObject>,
        truncated: bool,
    },
    DatabasesLoaded {
        operation: OperationId,
        databases: Vec<DatabaseInfo>,
        truncated: bool,
    },
    SchemasLoaded {
        operation: OperationId,
        schemas: Vec<SchemaInfo>,
        truncated: bool,
    },
    ObjectsLoaded {
        operation: OperationId,
        schema: String,
        objects: Vec<SchemaObjectInfo>,
        truncated: bool,
    },
    ColumnsLoaded {
        operation: OperationId,
        relation: RelationRef,
        columns: Vec<SchemaColumn>,
        truncated: bool,
    },
    Connected {
        operation: OperationId,
    },
    Executing {
        operation: OperationId,
        execution: ExecutionKey,
    },
    Cancelling {
        operation: OperationId,
        execution: ExecutionKey,
    },
    Finished {
        operation: OperationId,
        report: ExecutionReport,
    },
    Disconnected {
        operation: Option<OperationId>,
    },
    Error {
        operation: Option<OperationId>,
        message: String,
    },
}
enum Command {
    Configure(ConnectionRequest),
    TestConnection(ConnectionRequest),
    InspectSchema,
    ListDatabases,
    Catalog(CatalogRequest),
    Connect,
    Execute(String),
    ApplyBatch {
        statements: Vec<ParameterizedStatement>,
        guard: Option<TableIdentityGuard>,
    },
    SetCommitPolicy(CommitPolicy),
    Commit,
    Rollback,
    Cancel(CancelTarget),
    Disconnect,
}
enum CatalogRequest {
    Schemas,
    Objects(String),
    Columns(RelationRef),
}
struct Request {
    operation: OperationId,
    command: Command,
    reserved: usize,
    connection_epoch: u64,
}
type Finish = Pin<Box<dyn Future<Output = Result<ExecutionReport, &'static str>> + Send>>;
struct Active {
    operation: OperationId,
    key: ExecutionKey,
    finish: Finish,
}

/// All methods are nonblocking. Poll try_next to release admission budget.
pub struct DatabaseBridge {
    commands: mpsc::Sender<Request>,
    events: mpsc::Receiver<BridgeEvent>,
    shutdown: watch::Sender<bool>,
    interrupt_connection: watch::Sender<u64>,
    notify_cancel: watch::Sender<u64>,
    cancelled: Arc<Mutex<HashMap<OperationId, usize>>>,
    legacy: bool,
    sequence: AtomicU64,
    reserved: Arc<AtomicUsize>,
}
impl DatabaseBridge {
    pub fn new() -> std::io::Result<Self> {
        Self::build(None, false)
    }
    fn build(initial: Option<ConnectionRequest>, legacy: bool) -> std::io::Result<Self> {
        let (commands, receiver) = mpsc::channel(16);
        // Each admitted command reserves its maximum event count before enqueue.
        // 64 slots includes slack for a single internal runtime-build failure.
        let (events_sender, events) = mpsc::channel(64);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (interrupt_connection, connection_interrupt) = watch::channel(0);
        let (notify_cancel, cancel_notifications) = watch::channel(0);
        let cancelled = Arc::new(Mutex::new(HashMap::new()));
        let control = ConnectionControl {
            interrupt: connection_interrupt,
            cancel_notifications,
            cancelled: cancelled.clone(),
        };
        let reserved = Arc::new(AtomicUsize::new(0));
        let worker_reserved = reserved.clone();
        std::thread::Builder::new()
            .name("dbide-database-bridge".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => {
                        runtime.block_on(worker(
                            receiver,
                            events_sender,
                            shutdown_receiver,
                            control,
                            worker_reserved,
                            initial,
                            legacy,
                        ));
                        // Closing this private runtime aborts all driver tasks and drops their sockets.
                        // No JoinHandle is joined on the UI thread.
                        runtime.shutdown_timeout(Duration::from_millis(100));
                    }
                    Err(_) => {
                        let _ = events_sender.try_send(BridgeEvent::Error {
                            operation: None,
                            message: "database worker runtime unavailable".into(),
                        });
                    }
                }
            })?;
        Ok(Self {
            commands,
            events,
            shutdown,
            interrupt_connection,
            notify_cancel,
            cancelled,
            legacy,
            sequence: AtomicU64::new(1),
            reserved,
        })
    }
    pub fn configure(&self, request: ConnectionRequest) -> Result<OperationId, BridgeSubmitError> {
        self.submit(Command::Configure(request), 1)
    }
    pub fn test_connection(
        &self,
        request: ConnectionRequest,
    ) -> Result<OperationId, BridgeSubmitError> {
        self.submit(Command::TestConnection(request), 2)
    }
    pub fn inspect_schema(&self) -> Result<OperationId, BridgeSubmitError> {
        self.submit(Command::InspectSchema, 3)
    }
    /// Lists advertised databases using a separate bounded metadata connection.
    pub fn list_databases(&self) -> Result<OperationId, BridgeSubmitError> {
        self.submit(Command::ListDatabases, 3)
    }
    pub fn refresh_databases(&self) -> Result<OperationId, BridgeSubmitError> {
        self.list_databases()
    }
    pub fn list_schemas(&self) -> Result<OperationId, BridgeSubmitError> {
        self.submit(Command::Catalog(CatalogRequest::Schemas), 3)
    }
    pub fn list_objects(&self, schema: String) -> Result<OperationId, BridgeSubmitError> {
        if schema.len() > 4096 {
            return Err(BridgeSubmitError::SqlTooLarge);
        }
        self.submit(Command::Catalog(CatalogRequest::Objects(schema)), 3)
    }
    pub fn list_columns(&self, relation: RelationRef) -> Result<OperationId, BridgeSubmitError> {
        if relation.schema.len() > 4096 || relation.name.len() > 4096 {
            return Err(BridgeSubmitError::SqlTooLarge);
        }
        self.submit(Command::Catalog(CatalogRequest::Columns(relation)), 3)
    }
    pub fn connect(&self) -> Result<OperationId, BridgeSubmitError> {
        self.submit(Command::Connect, 2)
    }
    pub fn execute(&self, sql: String) -> Result<OperationId, BridgeSubmitError> {
        if sql.len() > 64 * 1024 {
            return Err(BridgeSubmitError::SqlTooLarge);
        }
        self.submit(Command::Execute(sql), 4)
    }
    /// Applies one exclusive PostgreSQL editing batch on the existing session only.
    /// No reconnect: edits must retain the physical context used to inspect their rows.
    pub fn apply_batch(
        &self,
        statements: Vec<ParameterizedStatement>,
    ) -> Result<OperationId, BridgeSubmitError> {
        self.enqueue_batch(statements, None)
    }
    /// Checks the catalog identity under a table lock before applying PostgreSQL edits.
    /// Retains the same physical connection and transaction policy as `apply_batch`.
    pub fn apply_guarded_batch(
        &self,
        guard: TableIdentityGuard,
        statements: Vec<ParameterizedStatement>,
    ) -> Result<OperationId, BridgeSubmitError> {
        if guard.schema.len() > 256
            || guard.table.len() > 256
            || guard
                .metadata_fingerprint
                .as_ref()
                .is_some_and(|value| value.len() > 64)
        {
            return Err(BridgeSubmitError::SqlTooLarge);
        }
        self.enqueue_batch(statements, Some(guard))
    }
    fn enqueue_batch(
        &self,
        statements: Vec<ParameterizedStatement>,
        guard: Option<TableIdentityGuard>,
    ) -> Result<OperationId, BridgeSubmitError> {
        let sql_bytes = statements
            .iter()
            .fold(0usize, |n, s| n.saturating_add(s.sql.len()));
        let parameter_bytes = statements
            .iter()
            .flat_map(|s| &s.parameters)
            .flatten()
            .fold(0usize, |n, p| n.saturating_add(p.len()));
        let parameter_count = statements
            .iter()
            .fold(0usize, |n, s| n.saturating_add(s.parameters.len()));
        if statements.len() > 256
            || sql_bytes > 64 * 1024
            || parameter_bytes > 4 * 1024 * 1024
            || parameter_count > 4096
        {
            return Err(BridgeSubmitError::SqlTooLarge);
        }
        self.submit(Command::ApplyBatch { statements, guard }, 2)
    }
    pub fn set_commit_policy(
        &self,
        policy: CommitPolicy,
    ) -> Result<OperationId, BridgeSubmitError> {
        self.submit(Command::SetCommitPolicy(policy), 1)
    }
    pub fn commit(&self) -> Result<OperationId, BridgeSubmitError> {
        self.submit(Command::Commit, 2)
    }
    pub fn rollback(&self) -> Result<OperationId, BridgeSubmitError> {
        self.submit(Command::Rollback, 2)
    }
    pub fn cancel(
        &self,
        target: impl Into<CancelTarget>,
    ) -> Result<OperationId, BridgeSubmitError> {
        let target = target.into();
        let pending = match target {
            CancelTarget::Operation(id) if !self.legacy => Some(id),
            _ => None,
        };
        if let Some(id) = pending {
            *self.cancelled.lock().unwrap().entry(id).or_default() += 1;
        }
        let submitted = self.submit(Command::Cancel(target), 1);
        if submitted.is_ok() {
            self.notify_cancel.send_modify(|n| *n = n.wrapping_add(1));
        } else if let Some(id) = pending {
            clear_cancel(&self.cancelled, id);
        }
        submitted
    }
    pub fn disconnect(&self) -> Result<OperationId, BridgeSubmitError> {
        self.submit(Command::Disconnect, 1)
    }
    pub fn try_next(&mut self) -> Option<BridgeEvent> {
        let event = self.events.try_recv().ok()?;
        // The only unreserved event is runtime construction failure, operation=None.
        if !matches!(
            &event,
            BridgeEvent::Error {
                operation: None,
                ..
            }
        ) {
            self.reserved.fetch_sub(1, Ordering::AcqRel);
        }
        Some(event)
    }
    fn submit(&self, command: Command, count: usize) -> Result<OperationId, BridgeSubmitError> {
        self.reserved
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n + count <= RESERVED_EVENTS).then_some(n + count)
            })
            .map_err(|_| BridgeSubmitError::QueueFull)?;
        let permit = match self.commands.try_reserve() {
            Ok(permit) => permit,
            Err(error) => {
                self.reserved.fetch_sub(count, Ordering::AcqRel);
                return Err(if matches!(error, mpsc::error::TrySendError::Closed(_)) {
                    BridgeSubmitError::WorkerStopped
                } else {
                    BridgeSubmitError::QueueFull
                });
            }
        };
        let operation = OperationId(self.sequence.fetch_add(1, Ordering::Relaxed));
        // A rejected Disconnect must not interrupt an already admitted connection.
        if matches!(&command, Command::Disconnect) {
            self.interrupt_connection
                .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        }
        let connection_epoch = *self.interrupt_connection.borrow();
        permit.send(Request {
            operation,
            command,
            reserved: count,
            connection_epoch,
        });
        Ok(operation)
    }
}
impl Drop for DatabaseBridge {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}
fn emit(sender: &mpsc::Sender<BridgeEvent>, event: BridgeEvent) -> bool {
    // Full is unreachable: at most32 reserved events and one internal event fit64.
    // Receiver closure is the only expected failure; terminate the private runtime.
    sender.try_send(event).is_ok()
}
fn error(sender: &mpsc::Sender<BridgeEvent>, operation: OperationId, message: &str) -> bool {
    emit(
        sender,
        BridgeEvent::Error {
            operation: Some(operation),
            message: message.into(),
        },
    )
}
async fn finish(active: &mut Option<Active>) -> Result<ExecutionReport, &'static str> {
    match active {
        Some(active) => active.finish.as_mut().await,
        None => pending().await,
    }
}
async fn cancel_active(session: &Session, active: &mut Active) -> CancelResult {
    let key = active.key;
    // Executing means submitted, so an immediate UI cancel can precede the
    // driver's Running transition. Wait for Running OR terminal completion.
    tokio::select! {
        result = active.finish.as_mut() => {
            active.finish = Box::pin(async move { result });
            return CancelResult::NotRunning;
        }
        _ = async {
            while !session.is_running(key).await {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        } => {}
    }
    session.cancel(key).await
}
/// Compatibility fixture wrapper retains its original explicit-reconnect event contract.
pub struct FixtureBridge(DatabaseBridge);
impl FixtureBridge {
    pub fn new() -> std::io::Result<Self> {
        let profile = dbide_profiles::ConnectionProfile::fixture();
        Ok(Self(DatabaseBridge::build(
            Some(ConnectionRequest {
                profile,
                password: "dbide_fixture_only".into(),
            }),
            true,
        )?))
    }
}
impl std::ops::Deref for FixtureBridge {
    type Target = DatabaseBridge;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for FixtureBridge {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
struct Budget<'a> {
    sender: &'a mpsc::Sender<BridgeEvent>,
    reserved: &'a AtomicUsize,
    remaining: usize,
}
impl Budget<'_> {
    fn send(&mut self, event: BridgeEvent) -> bool {
        self.remaining -= 1;
        emit(self.sender, event)
    }
    fn error(&mut self, operation: OperationId, message: &str) -> bool {
        self.send(BridgeEvent::Error {
            operation: Some(operation),
            message: message.into(),
        })
    }
    fn terminal_lease(&mut self) {
        self.remaining -= 1;
    }
}
impl Drop for Budget<'_> {
    fn drop(&mut self) {
        self.reserved.fetch_sub(self.remaining, Ordering::AcqRel);
    }
}
fn clear_cancel(cancelled: &Mutex<HashMap<OperationId, usize>>, id: OperationId) {
    let mut targets = cancelled.lock().unwrap();
    if let Some(count) = targets.get_mut(&id) {
        *count -= 1;
        if *count == 0 {
            targets.remove(&id);
        }
    }
}
struct ConnectionControl {
    interrupt: watch::Receiver<u64>,
    cancel_notifications: watch::Receiver<u64>,
    cancelled: Arc<Mutex<HashMap<OperationId, usize>>>,
}
impl ConnectionControl {
    fn is_cancelled(&self, operation: OperationId, epoch: u64) -> bool {
        *self.interrupt.borrow() != epoch || self.cancelled.lock().unwrap().contains_key(&operation)
    }
    async fn wait(&mut self, operation: OperationId, epoch: u64) {
        loop {
            self.interrupt.borrow_and_update();
            self.cancel_notifications.borrow_and_update();
            if self.is_cancelled(operation, epoch) {
                return;
            }
            tokio::select! {_=self.interrupt.changed()=>{},_=self.cancel_notifications.changed()=>{}}
        }
    }
}
async fn open(
    request: &ConnectionRequest,
    operation: OperationId,
    budget: &mut Budget<'_>,
    legacy: bool,
    control: &mut ConnectionControl,
    epoch: u64,
) -> Result<Session, ()> {
    if !legacy
        && !budget.send(BridgeEvent::Connecting {
            operation,
            attempt: 1,
            target: adapter::target(request),
        })
    {
        return Err(());
    }
    tokio::select! {
        biased;
        _ = control.wait(operation,epoch) => {
            budget.error(operation,"Connection attempt cancelled; SQL was not submitted");
            Err(())
        }
        result = Session::connect_request(request,Limits::default()) => match result {
            Ok(session)=>Ok(session),
            Err(diagnostic)=>{budget.send(BridgeEvent::ConnectionFailed {operation,diagnostic});Err(())}
        }
    }
}
async fn metadata(session: &Session) -> Result<(Vec<SchemaObject>, bool), &'static str> {
    let sql = session.metadata_sql();
    let report = tokio::time::timeout(
        Duration::from_secs(4),
        session.execute(sql, CommitPolicy::Auto),
    )
    .await
    .map_err(|_| "Schema inspection timed out")??;
    if report.outcome != Outcome::Completed {
        return Err("Schema inspection failed; check catalog access permissions");
    }
    let mut objects: Vec<SchemaObject> = vec![];
    let mut count = 0;
    for event in report.events {
        if let dbide_pg_spike::Event::Row { values } = event {
            count += 1;
            if count > 2048 {
                break;
            }
            let value = |i: usize| values.get(i).cloned().flatten().unwrap_or_default();
            let schema = value(0);
            let name = value(1);
            if objects
                .last()
                .is_none_or(|o| o.schema != schema || o.name != name)
            {
                objects.push(SchemaObject {
                    schema,
                    name,
                    kind: object_kind(&value(2)),
                    columns: vec![],
                });
            }
            if let Some(column) = column_from_values(&values) {
                objects.last_mut().unwrap().columns.push(column);
            }
        }
    }
    Ok((objects, report.truncated || count > 2048))
}
const MAX_DATABASES: usize = 256;
const MAX_DATABASE_NAME_BYTES: usize = 256;
const MAX_DATABASE_BYTES: usize = 64 * 1024;
async fn databases(
    session: &Session,
    request: &ConnectionRequest,
) -> Result<(Vec<DatabaseInfo>, bool), &'static str> {
    use dbide_profiles::DatabaseEngine;
    let mysql = request.profile.engine == DatabaseEngine::MySql;
    let report = tokio::time::timeout(
        Duration::from_secs(4),
        session.execute(session.databases_sql(), CommitPolicy::Auto),
    )
    .await
    .map_err(|_| "Database discovery timed out")??;
    if report.outcome != Outcome::Completed {
        return Err("Database discovery failed; check catalog/list permissions");
    }
    let mut databases = Vec::new();
    let mut retained_bytes = 0usize;
    let mut truncated = report.truncated;
    let mut result_set = 0;
    let mut mysql_current = None;
    for event in report.events {
        match event {
            dbide_driver_api::Event::ResultSetStarted { .. } => result_set += 1,
            dbide_driver_api::Event::Row { values } => {
                if mysql && result_set == 1 {
                    mysql_current = values.first().cloned().flatten();
                    continue;
                }
                let Some(name) = values.first().cloned().flatten() else {
                    truncated = true;
                    continue;
                };
                if databases.len() >= MAX_DATABASES
                    || name.len() > MAX_DATABASE_NAME_BYTES
                    || name.chars().any(char::is_control)
                    || name.is_empty()
                    || retained_bytes.saturating_add(name.len()) > MAX_DATABASE_BYTES
                {
                    truncated = true;
                    continue;
                }
                retained_bytes += name.len();
                let bool_at = |i: usize| values.get(i).and_then(|v| v.as_deref()) == Some("true");
                databases.push(DatabaseInfo {
                    current: if mysql {
                        mysql_current.as_ref() == Some(&name)
                    } else {
                        bool_at(2)
                    },
                    name,
                    accessible: mysql || bool_at(1),
                });
            }
            _ => {}
        }
    }
    Ok((databases, truncated))
}
fn column_from_values(values: &[Option<String>]) -> Option<SchemaColumn> {
    let value = |i: usize| values.get(i).cloned().flatten();
    Some(SchemaColumn {
        name: value(3)?,
        data_type: value(4).unwrap_or_default(),
        nullable: value(5).as_deref() == Some("true"),
        primary_key_ordinal: value(6)
            .and_then(|v| v.parse::<u16>().ok())
            .filter(|n| *n > 0),
        foreign_key: value(7).as_deref() == Some("true"),
        default: value(8),
        extra: value(9),
    })
}
fn object_kind(value: &str) -> String {
    match value {
        "v" => "view",
        "m" => "materialized view",
        "f" => "foreign table",
        _ => "table",
    }
    .into()
}
enum CatalogResult {
    Schemas(Vec<SchemaInfo>),
    Objects {
        schema: String,
        objects: Vec<SchemaObjectInfo>,
    },
    Columns {
        relation: RelationRef,
        columns: Vec<SchemaColumn>,
    },
}
fn valid_catalog_request(request: &CatalogRequest) -> bool {
    let valid = |s: &str| !s.is_empty() && s.len() <= 4096 && !s.contains('\0');
    match request {
        CatalogRequest::Schemas => true,
        CatalogRequest::Objects(schema) => valid(schema),
        CatalogRequest::Columns(relation) => valid(&relation.schema) && valid(&relation.name),
    }
}
async fn catalog(
    session: &Session,
    request: CatalogRequest,
) -> Result<(CatalogResult, bool), &'static str> {
    let sql = session.catalog_sql(&request);
    let report = tokio::time::timeout(
        Duration::from_secs(4),
        session.execute(&sql, CommitPolicy::Auto),
    )
    .await
    .map_err(|_| "Metadata loading timed out")??;
    if report.outcome != Outcome::Completed {
        return Err("Metadata loading failed; check catalog access permissions");
    }
    let mut truncated = report.truncated;
    let cap = if matches!(request, CatalogRequest::Schemas) {
        256
    } else {
        2048
    };
    let mut rows = 0;
    let mut schemas = Vec::new();
    let mut objects = Vec::new();
    let mut columns = Vec::new();
    for event in report.events {
        if let dbide_driver_api::Event::Row { values } = event {
            rows += 1;
            if rows > cap {
                truncated = true;
                break;
            }
            let value = |i: usize| values.get(i).cloned().flatten();
            match &request {
                CatalogRequest::Schemas => {
                    if let Some(name) =
                        value(0).filter(|name| !name.is_empty() && name.len() <= 4096)
                    {
                        schemas.push(SchemaInfo { name });
                    } else {
                        truncated = true;
                    }
                }
                CatalogRequest::Objects(_) => {
                    if let (Some(schema), Some(name), Some(kind)) = (value(0), value(1), value(2)) {
                        if schema.len() > 4096 || name.len() > 4096 {
                            truncated = true;
                            continue;
                        }
                        objects.push(SchemaObjectInfo {
                            schema,
                            name,
                            kind: object_kind(&kind),
                        });
                    } else {
                        truncated = true;
                    }
                }
                CatalogRequest::Columns(_) => {
                    if let Some(column) = column_from_values(&values) {
                        columns.push(column);
                    }
                }
            }
        }
    }
    let result = match request {
        CatalogRequest::Schemas => CatalogResult::Schemas(schemas),
        CatalogRequest::Objects(schema) => CatalogResult::Objects { schema, objects },
        CatalogRequest::Columns(relation) => {
            if rows == 0 && !truncated {
                return Err("Relation no longer exists or is not visible; refresh its object list");
            }
            CatalogResult::Columns { relation, columns }
        }
    };
    Ok((result, truncated))
}
async fn worker(
    mut commands: mpsc::Receiver<Request>,
    events: mpsc::Sender<BridgeEvent>,
    mut shutdown: watch::Receiver<bool>,
    mut control: ConnectionControl,
    reserved: Arc<AtomicUsize>,
    mut configured: Option<ConnectionRequest>,
    legacy: bool,
) {
    let mut session: Option<Session> = None;
    let mut active: Option<Active> = None;
    let mut commit_policy = CommitPolicy::Auto;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            result = finish(&mut active), if active.is_some() => {
                let completed = active.take().unwrap();
                match result {
                    Ok(report) => {
                        if report.connection_retired { session = None; }
                        if !emit(&events, BridgeEvent::Finished { operation: completed.operation, report }) { break; }
                    }
                    Err(_) => {
                        if let Some(old) = session.take() {old.close();}
                        if !error(&events, completed.operation, "Execution stopped; outcome unknown. Review server state before running SQL again.") { break; }
                    }
                }
            }
            request = commands.recv() => {
                let Some(request) = request else { break; };
                let operation = request.operation;
                let request_epoch = request.connection_epoch;
                let mut budget = Budget {sender:&events,reserved:&reserved,remaining:request.reserved};
                match request.command {
                    Command::SetCommitPolicy(policy) => {
                        if active.is_some() {if !budget.error(operation,"Cannot change commit policy during execution") {break;} continue;}
                        commit_policy=policy;
                        let transaction_state=session.as_ref().map_or(ServerTransactionState::Unknown,Session::transaction_state);
                        if !budget.send(BridgeEvent::CommitPolicyChanged{operation,policy,transaction_state}) {break;}
                    }
                    Command::Commit | Command::Rollback => {
                        if active.is_some() {if !budget.error(operation,"An execution is already running") {break;} continue;}
                        let Some(current)=session.as_ref().filter(|s|!s.is_closed()) else {if !budget.error(operation,"Commit/Rollback requires the existing connection; reconnect cannot recover a lost transaction") {break;} continue;};
                        if !legacy && control.is_cancelled(operation,request_epoch) {if !budget.error(operation,"Transaction control cancelled before SQL dispatch") {break;} continue;}
                        let commit=matches!(request.command,Command::Commit);
                        if commit && current.transaction_state()==ServerTransactionState::Failed {if !budget.error(operation,"Transaction is failed; Rollback is required before Commit") {break;} continue;}
                        let submitted=if commit {current.submit_commit(commit_policy)} else {current.submit_rollback(commit_policy)};
                        match submitted {
                            Ok(execution)=>{let key=execution.key;active=Some(Active{operation,key,finish:Box::pin(execution.finish())});budget.terminal_lease();if !budget.send(BridgeEvent::Executing{operation,execution:key}) {break;}},
                            Err(message)=>{if !budget.error(operation,message) {break;}}
                        }
                    }
                    Command::ApplyBatch{statements,guard} => {
                        if active.is_some() {if !budget.error(operation,"An execution is already running") {break;} continue;}
                        let Some(current)=session.as_ref().filter(|s|!s.is_closed()) else {if !budget.error(operation,"Apply requires the existing connection; reconnect and reload rows before editing") {break;} continue;};
                        if !legacy && control.is_cancelled(operation,request_epoch) {if !budget.error(operation,"Apply cancelled before SQL dispatch") {break;} continue;}
                        let submitted=match guard {Some(guard)=>current.submit_guarded_batch(guard,statements,commit_policy),None=>current.submit_batch(statements,commit_policy)};
                        match submitted {
                            Ok(execution)=>{let key=execution.key;active=Some(Active{operation,key,finish:Box::pin(execution.finish())});budget.terminal_lease();if !budget.send(BridgeEvent::Executing{operation,execution:key}) {break;}},
                            Err(message)=>{if !budget.error(operation,message) {break;}}
                        }
                    }
                    Command::Configure(request) => {
                        if active.is_some() { if !budget.error(operation,"Cannot change profile during execution; disconnect or cancel first") {break;} continue; }
                        if session.as_ref().is_some_and(|s|matches!(s.transaction_state(),ServerTransactionState::Active|ServerTransactionState::Failed)) {if !budget.error(operation,"Commit/Rollback or explicitly Disconnect before changing profiles; current transaction was preserved") {break;} continue;}
                        if request.profile.validate().is_err() {
                            let diagnostic=ConnectionDiagnostic::configuration(adapter::target(&request));
                            if !budget.send(BridgeEvent::ConnectionFailed{operation,diagnostic}) {break;} continue;
                        }
                        if let Some(old)=session.take() {old.close();}
                        configured=Some(request);
                        if !budget.send(BridgeEvent::Configured{operation}) {break;}
                    }
                    Command::TestConnection(request) => {
                        if active.is_some() {if !budget.error(operation,"Connection test is unavailable during execution") {break;} continue;}
                        let connected=tokio::select! {_=shutdown.changed()=>break,result=open(&request,operation,&mut budget,false,&mut control,request_epoch)=>result};
                        if let Ok(probe)=connected {probe.close();if !budget.send(BridgeEvent::ConnectionTested{operation}) {break;}}
                    }
                    Command::InspectSchema => {
                        if active.is_some() {if !budget.error(operation,"Wait for execution to finish before inspecting schema") {break;} continue;}
                        let Some(request)=configured.as_ref() else {if !budget.error(operation,"Configure a connection profile first") {break;} continue;};
                        let connected=tokio::select! {_=shutdown.changed()=>break,result=open(request,operation,&mut budget,false,&mut control,request_epoch)=>result};
                        if let Ok(probe)=connected {
                            let result=tokio::select! {_=shutdown.changed()=>{probe.close();break;},_=control.wait(operation,request_epoch)=>Err("Schema inspection cancelled"),result=metadata(&probe)=>result};
                            probe.close();
                            match result {Ok((objects,truncated))=>{if !budget.send(BridgeEvent::SchemaLoaded{operation,objects,truncated}) {break;}},Err(message)=>{if !budget.error(operation,message) {break;}}}
                        }
                    }
                    Command::ListDatabases => {
                        if active.is_some() {if !budget.error(operation,"Wait for execution to finish before listing databases") {break;} continue;}
                        let Some(request)=configured.as_ref() else {if !budget.error(operation,"Configure a connection profile first") {break;} continue;};
                        let connected=tokio::select! {_=shutdown.changed()=>break,result=open(request,operation,&mut budget,false,&mut control,request_epoch)=>result};
                        if let Ok(probe)=connected {
                            let result=tokio::select! {_=shutdown.changed()=>{probe.close();break;},_=control.wait(operation,request_epoch)=>Err("Database discovery cancelled"),result=databases(&probe,request)=>result};
                            probe.close();
                            match result {Ok((databases,truncated))=>{if !budget.send(BridgeEvent::DatabasesLoaded{operation,databases,truncated}) {break;}},Err(message)=>{if !budget.error(operation,message) {break;}}}
                        }
                    }
                    Command::Catalog(query) => {
                        if active.is_some() {if !budget.error(operation,"Wait for execution to finish before loading metadata") {break;} continue;}
                        if !valid_catalog_request(&query) {if !budget.error(operation,"Metadata scope is invalid") {break;} continue;}
                        let Some(request)=configured.as_ref() else {if !budget.error(operation,"Configure a connection profile first") {break;} continue;};
                        let connected=tokio::select! {_=shutdown.changed()=>break,result=open(request,operation,&mut budget,false,&mut control,request_epoch)=>result};
                        if let Ok(probe)=connected {
                            let result=tokio::select! {_=shutdown.changed()=>{probe.close();break;},_=control.wait(operation,request_epoch)=>Err("Metadata loading cancelled"),result=catalog(&probe,query)=>result};
                            probe.close();
                            match result {Ok((result,truncated))=>{
                                let event=match result {CatalogResult::Schemas(schemas)=>BridgeEvent::SchemasLoaded{operation,schemas,truncated},CatalogResult::Objects{schema,objects}=>BridgeEvent::ObjectsLoaded{operation,schema,objects,truncated},CatalogResult::Columns{relation,columns}=>BridgeEvent::ColumnsLoaded{operation,relation,columns,truncated}};
                                if !budget.send(event) {break;}
                            },Err(message)=>{if !budget.error(operation,message) {break;}}}
                        }
                    }
                    Command::Connect | Command::Execute(_) => {
                        if active.is_some() {if !budget.error(operation,"An execution is already running") {break;} continue;}
                        let execute=match request.command {Command::Execute(sql)=>Some(sql),_=>None};
                        if session.as_ref().is_some_and(Session::is_closed) && let Some(old)=session.take(){old.close();}
                        if session.is_none() {
                            if legacy && execute.is_some() {if !budget.error(operation,"execute requires one idle fixture session") {break;} continue;}
                            let Some(request)=configured.as_ref() else {if !budget.error(operation,"Configure a connection profile first") {break;} continue;};
                            let connected=tokio::select! {_=shutdown.changed()=>break,result=open(request,operation,&mut budget,legacy,&mut control,request_epoch)=>result};
                            let Ok(connected)=connected else {continue;};
                            session=Some(connected);
                            if !budget.send(BridgeEvent::Connected{operation}) {break;}
                        } else if execute.is_none() && !budget.send(BridgeEvent::Connected{operation}) {break;}
                        if let Some(sql)=execute {
                            if !legacy && control.is_cancelled(operation,request_epoch) {if !budget.error(operation,"Execution cancelled before SQL dispatch") {break;} continue;}
                            // Exactly one submission. Never replay a failed/unknown dispatched query.
                            match session.as_ref().unwrap().submit(&sql,commit_policy) {
                                Ok(execution)=>{
                                    let key=execution.key;active=Some(Active{operation,key,finish:Box::pin(execution.finish())});
                                    budget.terminal_lease();
                                    if !budget.send(BridgeEvent::Executing{operation,execution:key}) {break;}
                                },
                                Err(message)=>{if !budget.error(operation,message) {break;}}
                            }
                        }
                    }
                    Command::Cancel(target) => {
                        if let CancelTarget::Operation(id)=target {clear_cancel(&control.cancelled,id);}
                        let Some(current)=active.as_mut() else {if !budget.error(operation,"No running execution") {break;} continue;};
                        let matches=match target {CancelTarget::Execution(key)=>current.key==key,CancelTarget::Operation(id)=>current.operation==id};
                        if !matches {if !budget.error(operation,"Cancel target is no longer the current execution; cancel not sent") {break;} continue;}
                        let key=current.key;
                        let cancel=tokio::select! {_=shutdown.changed()=>break,result=cancel_active(session.as_ref().unwrap(),current)=>result};
                        match cancel {CancelResult::NotRunning=>{if !budget.error(operation,"Execution already completed; cancel not sent") {break;}},_=>{if !budget.send(BridgeEvent::Cancelling{operation,execution:key}) {break;}}}
                    }
                    Command::Disconnect => {
                        if let Some(current)=active.as_mut() {
                            let completed_operation=current.operation;
                            tokio::select! {_=shutdown.changed()=>break,_=cancel_active(session.as_ref().unwrap(),current)=>{}}
                            let result=tokio::select! {_=shutdown.changed()=>break,result=tokio::time::timeout(Duration::from_secs(6),finish(&mut active))=>result};
                            active=None;
                            match result {Ok(Ok(report))=>{if !emit(&events,BridgeEvent::Finished{operation:completed_operation,report}) {break;}},_=>{if !error(&events,completed_operation,"Disconnect retired execution; outcome unknown") {break;}}}
                        }
                        if let Some(old)=session.take(){old.close();}
                        if !budget.send(BridgeEvent::Disconnected{operation:Some(operation)}) {break;}
                    }
                }
            }
        }
    }
    if let (Some(session), Some(active)) = (session.as_ref(), active.as_mut()) {
        let _ = tokio::time::timeout(Duration::from_secs(8), async {
            let _ = cancel_active(session, active).await;
            let _ = active.finish.as_mut().await;
        })
        .await;
    }
    if let Some(session) = session {
        session.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_admission_and_drop_do_not_need_database() {
        let bridge = FixtureBridge::new().unwrap();
        let mut full = false;
        for _ in 0..100 {
            if bridge.execute("SELECT 1".into()) == Err(BridgeSubmitError::QueueFull) {
                full = true;
                break;
            }
        }
        assert!(
            full,
            "unpolled UI must eventually experience admission backpressure"
        );
        let start = std::time::Instant::now();
        drop(bridge);
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "UI Drop blocked"
        );
    }
}
