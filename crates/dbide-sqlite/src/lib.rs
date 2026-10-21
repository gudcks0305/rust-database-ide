//! Native SQLite adapter. One connection belongs to one background worker.
//! Native Batch/cancel invariants extend the repository-owned M0 SQLite spike.
use dbide_profiles::{ConnectionRequest, DatabaseEngine};
use rusqlite::{
    Batch, Connection, ErrorCode, OpenFlags,
    fallible_iterator::FallibleIterator,
    hooks::{AuthAction, AuthContext, Authorization},
    types::ValueRef,
};
use std::{
    fmt,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

pub use dbide_driver_api::{
    CommitPolicy, Event, ExecutionKey, ExecutionReport, Limits, Outcome, ServerError,
    ServerTransactionState,
};

const QUEUE_CAPACITY: usize = 8;
const MAX_SQL_BYTES: usize = 1024 * 1024;
const MAX_RETAINED_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENTS: usize = 65_536;
pub const MAX_STATEMENTS: usize = 256;
/// Lock contention is bounded independently of profile connection setup timeout.
pub const BUSY_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionErrorKind {
    Configuration,
    NotFound,
    Permission,
    Busy,
    Io,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectError {
    pub kind: ConnectionErrorKind,
    pub message: String,
    pub hint: String,
}
impl ConnectError {
    fn new(kind: ConnectionErrorKind) -> Self {
        let (message, hint) = match kind {
            ConnectionErrorKind::Configuration => (
                "Invalid SQLite configuration",
                "Choose SQLite and provide a valid file path or :memory:",
            ),
            ConnectionErrorKind::NotFound => (
                "SQLite database file was not found",
                "Choose an existing database or explicitly enable file creation",
            ),
            ConnectionErrorKind::Permission => (
                "SQLite database access was denied",
                "Check database and parent directory permissions; use read-only when appropriate",
            ),
            ConnectionErrorKind::Busy => (
                "SQLite database is busy",
                "Finish the competing transaction and retry",
            ),
            ConnectionErrorKind::Io => (
                "SQLite database could not be opened",
                "Check the file, parent directory, storage, and database format",
            ),
        };
        Self {
            kind,
            message: message.into(),
            hint: hint.into(),
        }
    }
}
impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}; {}", self.message, self.hint)
    }
}
impl std::error::Error for ConnectError {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelResult {
    Requested,
    NotRunning,
}
struct Active {
    key: ExecutionKey,
    cancelled: bool,
}
#[derive(Default)]
struct Control {
    active: Option<Active>,
    closed: bool,
}
fn lock(control: &Mutex<Control>) -> MutexGuard<'_, Control> {
    control.lock().unwrap_or_else(|e| e.into_inner())
}
struct Shared {
    sender: mpsc::SyncSender<Request>,
    control: Arc<Mutex<Control>>,
    interrupt: rusqlite::InterruptHandle,
    state: Arc<AtomicU8>,
    finished: Arc<AtomicBool>,
    sequence: AtomicU64,
    id: u64,
    limits: Limits,
}
impl Shared {
    fn close(&self) {
        let mut control = lock(&self.control);
        control.closed = true;
        self.interrupt.interrupt();
    }
}
impl Drop for Shared {
    fn drop(&mut self) {
        // No join, blocking native work, or runtime dependency on the UI thread.
        self.close();
    }
}
#[derive(Clone)]
pub struct Session {
    shared: Arc<Shared>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestKind {
    Sql,
    Commit,
    Rollback,
}
#[cfg(test)]
struct Probe {
    entered: mpsc::SyncSender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
struct Request {
    key: ExecutionKey,
    sql: String,
    policy: CommitPolicy,
    kind: RequestKind,
    abandoned: Arc<AtomicBool>,
    reply: oneshot::Sender<ExecutionReport>,
    #[cfg(test)]
    probe: Option<Arc<Probe>>,
}
pub struct Execution {
    pub key: ExecutionKey,
    reply: Option<oneshot::Receiver<ExecutionReport>>,
    abandoned: Arc<AtomicBool>,
}
impl Execution {
    pub async fn finish(mut self) -> Result<ExecutionReport, &'static str> {
        self.reply
            .take()
            .expect("execution receiver is consumed once")
            .await
            .map_err(|_| "session worker stopped without a report")
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        // Queued dropped tickets never start; active dropped tickets stop at the
        // next native progress callback or statement boundary.
        self.abandoned.store(true, Ordering::Release);
    }
}
impl Session {
    pub async fn connect_request(
        request: &ConnectionRequest,
        limits: Limits,
    ) -> Result<Self, ConnectError> {
        if request.profile.engine != DatabaseEngine::Sqlite || request.profile.validate().is_err() {
            return Err(ConnectError::new(ConnectionErrorKind::Configuration));
        }
        // Password/network fields are deliberately neither cloned nor passed to SQLite.
        let path = request
            .profile
            .sqlite_path
            .clone()
            .ok_or_else(|| ConnectError::new(ConnectionErrorKind::Configuration))?;
        let read_only = request.profile.read_only;
        let create = request.profile.sqlite_create;
        let limits = Limits {
            retained_bytes: limits.retained_bytes.min(MAX_RETAINED_BYTES),
            events: limits.events.min(MAX_EVENTS),
            sql_bytes: limits.sql_bytes.min(MAX_SQL_BYTES),
        };
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (ready, initialized) = oneshot::channel();
        let control = Arc::new(Mutex::new(Control::default()));
        let state = Arc::new(AtomicU8::new(ServerTransactionState::Idle as u8));
        let finished = Arc::new(AtomicBool::new(false));
        let worker_control = control.clone();
        let worker_state = state.clone();
        let worker_finished = finished.clone();
        // The detached handle never owns another sender. The worker exits after
        // close/drop, rejects queued SQL, then closes its physical connection.
        thread::Builder::new()
            .name("dbide-sqlite".into())
            .spawn(move || {
                struct Finish(Arc<AtomicBool>);
                impl Drop for Finish {
                    fn drop(&mut self) {
                        self.0.store(true, Ordering::Release);
                    }
                }
                let _finish = Finish(worker_finished);
                let connection = match open_connection(&path, read_only, create) {
                    Ok(connection) => connection,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if ready.send(Ok(connection.get_interrupt_handle())).is_err() {
                    return;
                }
                worker(
                    connection,
                    receiver,
                    worker_control,
                    worker_state,
                    limits,
                    read_only,
                );
            })
            .map_err(|_| ConnectError::new(ConnectionErrorKind::Io))?;
        let interrupt = initialized
            .await
            .map_err(|_| ConnectError::new(ConnectionErrorKind::Io))??;
        Ok(Self {
            shared: Arc::new(Shared {
                sender,
                control,
                interrupt,
                state,
                finished,
                sequence: AtomicU64::new(1),
                id: dbide_driver_api::next_session_id(),
                limits,
            }),
        })
    }
    pub fn submit(&self, sql: &str, policy: CommitPolicy) -> Result<Execution, &'static str> {
        self.submit_kind(sql, policy, RequestKind::Sql)
    }
    pub fn submit_commit(&self, policy: CommitPolicy) -> Result<Execution, &'static str> {
        self.submit_kind("COMMIT", policy, RequestKind::Commit)
    }
    pub fn submit_rollback(&self, policy: CommitPolicy) -> Result<Execution, &'static str> {
        self.submit_kind("ROLLBACK", policy, RequestKind::Rollback)
    }
    fn submit_kind(
        &self,
        sql: &str,
        policy: CommitPolicy,
        kind: RequestKind,
    ) -> Result<Execution, &'static str> {
        if sql.len() > self.shared.limits.sql_bytes {
            return Err("SQL exceeds SQLite byte limit");
        }
        let control = lock(&self.shared.control);
        if control.closed || self.shared.finished.load(Ordering::Acquire) {
            return Err("session is closed");
        }
        let key = ExecutionKey {
            session: self.shared.id,
            generation: 1,
            execution: self.shared.sequence.fetch_add(1, Ordering::Relaxed),
        };
        let (reply, receiver) = oneshot::channel();
        let abandoned = Arc::new(AtomicBool::new(false));
        self.shared
            .sender
            .try_send(Request {
                key,
                sql: sql.into(),
                policy,
                kind,
                abandoned: abandoned.clone(),
                reply,
                #[cfg(test)]
                probe: None,
            })
            .map_err(|_| "session queue full or closed")?;
        Ok(Execution {
            key,
            reply: Some(receiver),
            abandoned,
        })
    }
    pub async fn execute(
        &self,
        sql: &str,
        policy: CommitPolicy,
    ) -> Result<ExecutionReport, &'static str> {
        self.submit(sql, policy)?.finish().await
    }
    pub async fn commit(&self, policy: CommitPolicy) -> Result<ExecutionReport, &'static str> {
        self.submit_commit(policy)?.finish().await
    }
    pub async fn rollback(&self, policy: CommitPolicy) -> Result<ExecutionReport, &'static str> {
        self.submit_rollback(policy)?.finish().await
    }
    pub fn close(&self) {
        self.shared.close();
    }
    pub fn is_closed(&self) -> bool {
        lock(&self.shared.control).closed || self.shared.finished.load(Ordering::Acquire)
    }
    pub fn transaction_state(&self) -> ServerTransactionState {
        if self.is_closed() {
            ServerTransactionState::Unknown
        } else if self.shared.state.load(Ordering::Acquire) == ServerTransactionState::Idle as u8 {
            ServerTransactionState::Idle
        } else {
            ServerTransactionState::Active
        }
    }
    pub async fn is_running(&self, key: ExecutionKey) -> bool {
        lock(&self.shared.control)
            .active
            .as_ref()
            .is_some_and(|active| active.key == key)
    }
    pub async fn cancel(&self, key: ExecutionKey) -> CancelResult {
        // The worker clears the lease and admits the next execution under this
        // same lock. A stale interrupt cannot cross into the next execution.
        let mut control = lock(&self.shared.control);
        if let Some(active) = control
            .active
            .as_mut()
            .filter(|active| active.key == key && !active.cancelled)
        {
            active.cancelled = true;
            self.shared.interrupt.interrupt();
            CancelResult::Requested
        } else {
            CancelResult::NotRunning
        }
    }
}
fn open_connection(path: &str, read_only: bool, create: bool) -> Result<Connection, ConnectError> {
    if path != ":memory:" && (!create || read_only) {
        std::fs::metadata(path).map_err(|error| {
            ConnectError::new(match error.kind() {
                std::io::ErrorKind::NotFound => ConnectionErrorKind::NotFound,
                std::io::ErrorKind::PermissionDenied => ConnectionErrorKind::Permission,
                _ => ConnectionErrorKind::Io,
            })
        })?;
    }
    // URI interpretation and extension loading are not enabled.
    let mut flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    if (create || path == ":memory:") && !read_only {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let connection = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(connection_error)?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(connection_error)?;
    if read_only {
        connection
            .pragma_update(None, "query_only", true)
            .map_err(connection_error)?;
    }
    install_authorizer(&connection, read_only, false).map_err(connection_error)?;
    Ok(connection)
}
fn connection_error(error: rusqlite::Error) -> ConnectError {
    ConnectError::new(match error.sqlite_error_code() {
        Some(
            ErrorCode::PermissionDenied
            | ErrorCode::ReadOnly
            | ErrorCode::AuthorizationForStatementDenied,
        ) => ConnectionErrorKind::Permission,
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => ConnectionErrorKind::Busy,
        _ => ConnectionErrorKind::Io,
    })
}
fn install_authorizer(
    connection: &Connection,
    read_only: bool,
    budget_exhausted: bool,
) -> rusqlite::Result<()> {
    connection.authorizer(Some(move |context: AuthContext<'_>| {
        if budget_exhausted {
            return Authorization::Deny;
        }
        if let AuthAction::Pragma {
            pragma_name,
            pragma_value: Some(_),
        } = context.action
            && pragma_name.eq_ignore_ascii_case("busy_timeout")
        {
            return Authorization::Deny;
        }
        if !read_only {
            return Authorization::Allow;
        }
        match context.action {
            AuthAction::Read { .. }
            | AuthAction::Select
            | AuthAction::Transaction { .. }
            | AuthAction::Savepoint { .. }
            | AuthAction::Recursive => Authorization::Allow,
            AuthAction::Function { function_name }
                if !function_name.eq_ignore_ascii_case("load_extension") =>
            {
                Authorization::Allow
            }
            AuthAction::Pragma {
                pragma_name,
                pragma_value,
            } => {
                let name = pragma_name.to_ascii_lowercase();
                let metadata = matches!(
                    name.as_str(),
                    "table_info"
                        | "table_xinfo"
                        | "index_list"
                        | "index_info"
                        | "index_xinfo"
                        | "foreign_key_list"
                );
                let scalar = matches!(
                    name.as_str(),
                    "database_list"
                        | "collation_list"
                        | "compile_options"
                        | "foreign_keys"
                        | "user_version"
                        | "schema_version"
                        | "application_id"
                        | "query_only"
                        | "journal_mode"
                );
                if metadata || (scalar && pragma_value.is_none()) {
                    Authorization::Allow
                } else {
                    Authorization::Deny
                }
            }
            _ => Authorization::Deny,
        }
    }))
}
fn observed_state(connection: &Connection) -> ServerTransactionState {
    if connection.is_autocommit() {
        ServerTransactionState::Idle
    } else {
        ServerTransactionState::Active
    }
}
fn interrupted(control: &Mutex<Control>, abandoned: &AtomicBool) -> bool {
    let control = lock(control);
    control.closed
        || control
            .active
            .as_ref()
            .is_some_and(|active| active.cancelled)
        || abandoned.load(Ordering::Acquire)
}
fn worker(
    connection: Connection,
    receiver: mpsc::Receiver<Request>,
    control: Arc<Mutex<Control>>,
    state: Arc<AtomicU8>,
    limits: Limits,
    read_only: bool,
) {
    loop {
        let request = match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if lock(&control).closed {
                    break;
                }
                continue;
            }
        };
        let admission = {
            let mut control = lock(&control);
            if control.closed || request.abandoned.load(Ordering::Acquire) {
                false
            } else {
                control.active = Some(Active {
                    key: request.key,
                    cancelled: false,
                });
                true
            }
        };
        let mut report = empty_report(&request, &connection);
        if !admission {
            report.outcome = Outcome::Rejected;
            report.connection_retired = lock(&control).closed;
            set_error(
                &mut report,
                "SessionClosing",
                "Execution was rejected before starting",
            );
        } else {
            let progress_control = control.clone();
            let abandoned = request.abandoned.clone();
            #[cfg(test)]
            let mut probe = request.probe.clone();
            let result = connection.progress_handler(
                1000,
                Some(move || {
                    #[cfg(test)]
                    if let Some(probe) = probe.take() {
                        let _ = probe.entered.send(());
                        let _ = probe
                            .resume
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(5));
                    }
                    interrupted(&progress_control, &abandoned)
                }),
            );
            if let Err(error) = result {
                native_error(&mut report, error);
            } else {
                execute(
                    &connection,
                    &request,
                    &control,
                    limits,
                    read_only,
                    &mut report,
                );
            }
            let _ = connection.progress_handler(0, None::<fn() -> bool>);
            let mut control = lock(&control);
            report.connection_retired = control.closed;
            control.active = None;
        }
        report.transaction_state = observed_state(&connection);
        state.store(report.transaction_state as u8, Ordering::Release);
        let _ = request.reply.send(report);
    }
    lock(&control).closed = true;
}
fn empty_report(request: &Request, connection: &Connection) -> ExecutionReport {
    ExecutionReport {
        key: request.key,
        commit_policy: request.policy,
        transaction_state: observed_state(connection),
        outcome: Outcome::Completed,
        terminal_error: None,
        events: vec![],
        notices: vec![],
        retained_bytes: 0,
        truncated: false,
        connection_retired: false,
        elapsed_micros: 0,
    }
}
fn set_error(report: &mut ExecutionReport, code: &str, message: &str) {
    report.terminal_error = Some(ServerError {
        sqlstate: Some(code.into()),
        message: bounded(message, 2048),
    });
}
fn native_error(report: &mut ExecutionReport, error: rusqlite::Error) {
    report.outcome = if error.sqlite_error_code() == Some(ErrorCode::OperationInterrupted) {
        Outcome::Cancelled
    } else {
        Outcome::Failed
    };
    let code = error
        .sqlite_error_code()
        .map(|code| format!("SQLITE_{code:?}"));
    report.terminal_error = Some(ServerError {
        sqlstate: code,
        message: bounded(&error.to_string(), 2048),
    });
}
fn bounded(text: &str, bytes: usize) -> String {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].into()
}
fn retain(report: &mut ExecutionReport, limits: Limits, event: Event, bytes: usize) {
    if report.events.len() >= limits.events
        || bytes > limits.retained_bytes.saturating_sub(report.retained_bytes)
    {
        report.truncated = true;
    } else {
        report.retained_bytes += bytes;
        report.events.push(event);
    }
}
fn execute(
    connection: &Connection,
    request: &Request,
    control: &Mutex<Control>,
    limits: Limits,
    read_only: bool,
    report: &mut ExecutionReport,
) {
    let start = Instant::now();
    let result: rusqlite::Result<()> = (|| {
        if request.kind == RequestKind::Sql
            && request.policy == CommitPolicy::Manual
            && connection.is_autocommit()
        {
            if interrupted(control, &request.abandoned) {
                return Err(interrupt_error());
            }
            connection.execute_batch("BEGIN")?;
        }
        let mut batch = Batch::new(connection, &request.sql);
        let mut statements = 0;
        loop {
            if interrupted(control, &request.abandoned) {
                return Err(interrupt_error());
            }
            // SQLite can run PRAGMA side effects at prepare time. Guard BEFORE
            // Batch::next at the statement cap, including native empty tails.
            let at_limit = statements >= MAX_STATEMENTS;
            if at_limit {
                install_authorizer(connection, read_only, true)?;
            }
            let next = batch.next();
            if at_limit {
                install_authorizer(connection, read_only, false)?;
                if matches!(next, Ok(None)) {
                    break;
                }
                return Err(rusqlite::Error::InvalidParameterName(
                    "SQLite statement budget exceeded; remaining script was not executed".into(),
                ));
            }
            let Some(mut statement) = next? else {
                break;
            };
            statements += 1;
            if statement.column_count() == 0 {
                statement.execute([])?;
                // DDL/control and trigger effects make sqlite3_changes stale or
                // misleading. Unknown is deliberate until native classification.
                retain(report, limits, Event::CommandCompleteUnknown, 16);
                continue;
            }
            let names = statement.column_names();
            let name_bytes = names
                .iter()
                .fold(16usize, |size, name| size.saturating_add(name.len() + 24));
            if report.events.len() < limits.events
                && name_bytes <= limits.retained_bytes.saturating_sub(report.retained_bytes)
            {
                retain(
                    report,
                    limits,
                    Event::ResultSetStarted {
                        columns: names.into_iter().map(str::to_owned).collect(),
                    },
                    name_bytes,
                );
            } else {
                report.truncated = true;
            }
            let mut rows = statement.query([])?;
            let mut produced_rows = 0u64;
            while let Some(row) = rows.next()? {
                produced_rows = produced_rows.saturating_add(1);
                if interrupted(control, &request.abandoned) {
                    return Err(interrupt_error());
                }
                if report.truncated || report.events.len() >= limits.events {
                    report.truncated = true;
                    continue;
                }
                let columns = row.as_ref().column_count();
                let mut bytes = 16usize;
                for column in 0..columns {
                    let size = match row.get_ref(column)? {
                        ValueRef::Null => 8,
                        ValueRef::Integer(_) | ValueRef::Real(_) => 32,
                        ValueRef::Text(value) => value.len().saturating_mul(3),
                        ValueRef::Blob(value) => value.len().saturating_mul(2).saturating_add(3),
                    };
                    bytes = bytes.saturating_add(size).saturating_add(24);
                }
                if bytes > limits.retained_bytes.saturating_sub(report.retained_bytes) {
                    report.truncated = true;
                    continue;
                }
                let values = (0..columns)
                    .map(|column| {
                        Ok(match row.get_ref(column)? {
                            ValueRef::Null => None,
                            ValueRef::Integer(value) => Some(value.to_string()),
                            ValueRef::Real(value) => Some(value.to_string()),
                            ValueRef::Text(value) => {
                                Some(String::from_utf8_lossy(value).into_owned())
                            }
                            ValueRef::Blob(value) => {
                                use std::fmt::Write;
                                let mut text = String::with_capacity(value.len() * 2 + 3);
                                text.push_str("x'");
                                for byte in value {
                                    write!(&mut text, "{byte:02x}").expect("String writes succeed");
                                }
                                text.push('\'');
                                Some(text)
                            }
                        })
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                retain(report, limits, Event::Row { values }, bytes);
            }
            // A completed result stream needs its own terminator; otherwise a
            // following DDL command is mistaken for this result's completion.
            // Only emit after SQLite returns end-of-stream, never after error.
            retain(
                report,
                limits,
                Event::CommandComplete {
                    affected_rows: produced_rows,
                },
                16,
            );
        }
        Ok(())
    })();
    if let Err(error) = result {
        native_error(report, error);
    }
    report.elapsed_micros = start.elapsed().as_micros();
}
fn interrupt_error() -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_INTERRUPT),
        Some("SQLite execution interrupted".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbide_profiles::ConnectionProfile;

    const LONG: &str = "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n";
    fn request(path: &str, create: bool, read_only: bool) -> ConnectionRequest {
        ConnectionRequest {
            profile: ConnectionProfile {
                engine: DatabaseEngine::Sqlite,
                sqlite_path: Some(path.into()),
                sqlite_create: create,
                read_only,
                // SQLite must ignore irrelevant network/certificate fields.
                host: "invalid://network".into(),
                port: 0,
                username: String::new(),
                database: String::new(),
                root_cert_path: Some("nonexistent-certificate".into()),
                ..ConnectionProfile::default()
            },
            password: "sqlite-password-must-be-ignored".into(),
        }
    }
    async fn memory() -> Session {
        Session::connect_request(&request(":memory:", false, false), Limits::default())
            .await
            .unwrap()
    }
    async fn run(session: &Session, sql: &str) -> ExecutionReport {
        session.execute(sql, CommitPolicy::Auto).await.unwrap()
    }
    fn values(report: &ExecutionReport) -> Vec<Vec<Option<String>>> {
        report
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Row { values } => Some(values.clone()),
                _ => None,
            })
            .collect()
    }
    fn ok(report: &ExecutionReport) {
        assert_eq!(report.outcome, Outcome::Completed, "{report:?}");
    }
    fn native_gate(
        session: &Session,
        sql: &str,
    ) -> (Execution, mpsc::Receiver<()>, mpsc::SyncSender<()>) {
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        let key = ExecutionKey {
            session: session.shared.id,
            generation: 1,
            execution: session.shared.sequence.fetch_add(1, Ordering::Relaxed),
        };
        let (reply, receiver) = oneshot::channel();
        let abandoned = Arc::new(AtomicBool::new(false));
        session
            .shared
            .sender
            .try_send(Request {
                key,
                sql: sql.into(),
                policy: CommitPolicy::Auto,
                kind: RequestKind::Sql,
                abandoned: abandoned.clone(),
                reply,
                probe: Some(Arc::new(Probe {
                    entered,
                    resume: Mutex::new(resume),
                })),
            })
            .unwrap();
        (
            Execution {
                key,
                reply: Some(receiver),
                abandoned,
            },
            ready,
            release,
        )
    }
    async fn wait_finished(finished: &AtomicBool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !finished.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("native worker must exit after shutdown");
    }

    #[tokio::test]
    async fn native_script_boundaries_preserve_partial_results_and_storage_values() {
        let session = memory().await;
        let report = run(&session, "CREATE TABLE t(v); CREATE TABLE audit(v); CREATE TRIGGER log AFTER INSERT ON t BEGIN INSERT INTO audit VALUES('trigger; body'); INSERT INTO audit VALUES(new.v); END; INSERT INTO t VALUES('한글; 값'); -- ; comment\n SELECT v FROM audit ORDER BY rowid; SELECT NULL,'',9223372036854775807,1.25,x'0001ff'; SELECT * FROM missing; SELECT 999").await;
        assert_eq!(report.outcome, Outcome::Failed);
        assert_eq!(
            values(&report),
            vec![
                vec![Some("trigger; body".into())],
                vec![Some("한글; 값".into())],
                vec![
                    None,
                    Some("".into()),
                    Some(i64::MAX.to_string()),
                    Some("1.25".into()),
                    Some("x'0001ff'".into())
                ]
            ]
        );
        assert!(report.terminal_error.is_some());
        assert_eq!(report.transaction_state, ServerTransactionState::Idle);
        assert_eq!(
            report
                .events
                .iter()
                .filter(|event| matches!(event, Event::CommandCompleteUnknown))
                .count(),
            4
        );
        ok(&run(&session, "SELECT 42").await);
    }

    #[tokio::test]
    async fn completed_result_streams_separate_ddl_and_preserve_late_error() {
        let session = memory().await;
        let report = run(&session, "SELECT NULL AS first; CREATE TABLE t(v); SELECT 2 AS second; SELECT * FROM missing; SELECT 3 AS unexecuted").await;
        assert_eq!(report.outcome, Outcome::Failed);
        assert!(report.terminal_error.is_some());
        assert_eq!(
            report.events,
            vec![
                Event::ResultSetStarted {
                    columns: vec!["first".into()]
                },
                Event::Row { values: vec![None] },
                Event::CommandComplete { affected_rows: 1 },
                Event::CommandCompleteUnknown,
                Event::ResultSetStarted {
                    columns: vec!["second".into()]
                },
                Event::Row {
                    values: vec![Some("2".into())]
                },
                Event::CommandComplete { affected_rows: 1 },
            ],
            "SELECT, DDL, SELECT must form three completed pages before the late error"
        );
        let empty = run(&session, "SELECT v FROM t WHERE 0; SELECT 4 AS final").await;
        ok(&empty);
        assert_eq!(
            empty.events,
            vec![
                Event::ResultSetStarted {
                    columns: vec!["v".into()]
                },
                Event::CommandComplete { affected_rows: 0 },
                Event::ResultSetStarted {
                    columns: vec!["final".into()]
                },
                Event::Row {
                    values: vec![Some("4".into())]
                },
                Event::CommandComplete { affected_rows: 1 },
            ]
        );
    }

    #[tokio::test]
    async fn failed_result_stream_has_no_false_completion_count() {
        let session = memory().await;
        let report = run(
            &session,
            "SELECT 1 AS n UNION ALL SELECT abs(-9223372036854775808); CREATE TABLE unexecuted(v)",
        )
        .await;
        assert_eq!(report.outcome, Outcome::Failed);
        assert!(report.terminal_error.is_some());
        assert_eq!(
            report.events,
            vec![
                Event::ResultSetStarted {
                    columns: vec!["n".into()]
                },
                Event::Row {
                    values: vec![Some("1".into())]
                },
            ],
            "SQLite step error preserves earlier rows but cannot claim stream completion"
        );
        assert_eq!(
            run(&session, "SELECT * FROM unexecuted").await.outcome,
            Outcome::Failed
        );
    }

    #[tokio::test]
    async fn manual_transactions_observe_native_state_and_external_visibility() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.sqlite");
        let config = request(path.to_str().unwrap(), true, false);
        let writer = Session::connect_request(&config, Limits::default())
            .await
            .unwrap();
        let observer = Session::connect_request(&config, Limits::default())
            .await
            .unwrap();
        ok(&run(&writer, "CREATE TABLE t(v UNIQUE)").await);
        let report = writer
            .execute("INSERT INTO t VALUES(1)", CommitPolicy::Manual)
            .await
            .unwrap();
        ok(&report);
        assert_eq!(report.transaction_state, ServerTransactionState::Active);
        assert_eq!(writer.transaction_state(), ServerTransactionState::Active);
        assert_eq!(
            values(&run(&observer, "SELECT count(*) FROM t").await),
            vec![vec![Some("0".into())]]
        );
        let failure = writer
            .execute("INSERT INTO t VALUES(1)", CommitPolicy::Manual)
            .await
            .unwrap();
        assert_eq!(failure.outcome, Outcome::Failed);
        assert_eq!(
            failure.transaction_state,
            ServerTransactionState::Active,
            "SQLite statement errors do not invent a PostgreSQL failed transaction"
        );
        let committed = writer.commit(CommitPolicy::Manual).await.unwrap();
        ok(&committed);
        assert_eq!(
            committed.transaction_state,
            ServerTransactionState::Idle,
            "commit control bypasses manual auto-BEGIN"
        );
        assert_eq!(
            values(&run(&observer, "SELECT count(*) FROM t").await),
            vec![vec![Some("1".into())]]
        );
        ok(&writer
            .execute("INSERT INTO t VALUES(2)", CommitPolicy::Manual)
            .await
            .unwrap());
        ok(&writer.rollback(CommitPolicy::Manual).await.unwrap());
        assert_eq!(
            values(&run(&observer, "SELECT count(*) FROM t").await),
            vec![vec![Some("1".into())]]
        );
        ok(&run(
            &writer,
            "BEGIN; SAVEPOINT s; INSERT INTO t VALUES(3); ROLLBACK TO s; RELEASE s; COMMIT",
        )
        .await);
        assert_eq!(writer.transaction_state(), ServerTransactionState::Idle);
    }

    #[tokio::test]
    async fn session_connection_and_temp_tables_are_dedicated() {
        let a = memory().await;
        let b = memory().await;
        ok(&run(
            &a,
            "CREATE TEMP TABLE private(v); INSERT INTO private VALUES(7)",
        )
        .await);
        assert_eq!(
            run(&b, "SELECT * FROM private").await.outcome,
            Outcome::Failed
        );
        assert_eq!(
            values(&run(&a, "SELECT * FROM private").await),
            vec![vec![Some("7".into())]]
        );
        assert_ne!(
            run(&a, "SELECT 1").await.key.session,
            run(&b, "SELECT 1").await.key.session
        );
        ok(&run(&a.clone(), "SELECT * FROM private").await);
    }

    #[tokio::test]
    async fn existing_files_only_explicit_create_and_safe_connection_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.sqlite");
        let config = request(path.to_str().unwrap(), false, false);
        let error = match Session::connect_request(&config, Limits::default()).await {
            Err(error) => error,
            Ok(_) => panic!("must not create missing file"),
        };
        assert_eq!(error.kind, ConnectionErrorKind::NotFound);
        assert!(!path.exists());
        let text = format!("{error:?} {error}");
        assert!(!text.contains(path.to_str().unwrap()));
        assert!(!text.contains(&config.password));
        let created = Session::connect_request(
            &request(path.to_str().unwrap(), true, false),
            Limits::default(),
        )
        .await
        .unwrap();
        assert!(path.exists());
        ok(&run(&created, "CREATE TABLE t(v)").await);
        let mut invalid = config.clone();
        invalid.profile.engine = DatabaseEngine::PostgreSql;
        assert!(matches!(
            Session::connect_request(&invalid, Limits::default()).await,
            Err(ConnectError {
                kind: ConnectionErrorKind::Configuration,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn read_only_native_flags_and_authorizer_cannot_be_disabled_by_sql() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("readonly.sqlite");
        let writer = Session::connect_request(
            &request(path.to_str().unwrap(), true, false),
            Limits::default(),
        )
        .await
        .unwrap();
        ok(&run(&writer, "CREATE TABLE t(v); INSERT INTO t VALUES(5)").await);
        let reader = Session::connect_request(
            &request(path.to_str().unwrap(), false, true),
            Limits::default(),
        )
        .await
        .unwrap();
        for sql in [
            "INSERT INTO t VALUES(6)",
            "CREATE TEMP TABLE bypass(v)",
            "PRAGMA query_only=OFF; INSERT INTO t VALUES(6)",
            "PRAGMA main.query_only=OFF",
            "PRAGMA user_version=42",
            "ATTACH ':memory:' AS bypass",
            "VACUUM",
            "PRAGMA writable_schema=ON",
            "PRAGMA optimize",
        ] {
            assert_eq!(run(&reader, sql).await.outcome, Outcome::Failed, "{sql}");
        }
        let read = run(
            &reader,
            "SELECT v FROM t; PRAGMA table_info('t'); PRAGMA query_only; PRAGMA user_version",
        )
        .await;
        ok(&read);
        let rows = values(&read);
        assert_eq!(rows.first().unwrap(), &vec![Some("5".into())]);
        assert_eq!(rows[rows.len() - 2], vec![Some("1".into())]);
        assert_eq!(rows.last().unwrap(), &vec![Some("0".into())]);
        let readonly_memory =
            Session::connect_request(&request(":memory:", false, true), Limits::default())
                .await
                .unwrap();
        assert_eq!(
            run(&readonly_memory, "CREATE TABLE no(v)").await.outcome,
            Outcome::Failed
        );
        ok(&run(&readonly_memory, "SELECT 1").await);
    }

    #[tokio::test]
    async fn competing_writer_timeout_and_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("busy.sqlite");
        let config = request(path.to_str().unwrap(), true, false);
        let a = Session::connect_request(&config, Limits::default())
            .await
            .unwrap();
        let b = Session::connect_request(&config, Limits::default())
            .await
            .unwrap();
        ok(&run(
            &a,
            "CREATE TABLE t(v); BEGIN IMMEDIATE; INSERT INTO t VALUES(1)",
        )
        .await);
        let started = Instant::now();
        let blocked = run(&b, "INSERT INTO t VALUES(2)").await;
        assert_eq!(blocked.outcome, Outcome::Failed);
        assert_eq!(
            blocked.terminal_error.unwrap().sqlstate.as_deref(),
            Some("SQLITE_DatabaseBusy")
        );
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert!(started.elapsed() < Duration::from_secs(3));
        ok(&run(&a, "ROLLBACK").await);
        ok(&run(&b, "INSERT INTO t VALUES(2)").await);
    }

    #[tokio::test]
    async fn retention_caps_drain_writes_and_preserve_terminal_error() {
        let limits = Limits {
            retained_bytes: 256,
            events: 2,
            ..Limits::default()
        };
        let session = Session::connect_request(&request(":memory:", false, false), limits)
            .await
            .unwrap();
        ok(&run(&session, "CREATE TABLE t(v)").await);
        let report = run(&session, "SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3; INSERT INTO t VALUES(99); SELECT * FROM missing; INSERT INTO t VALUES(100)").await;
        assert_eq!(report.outcome, Outcome::Failed);
        assert!(report.truncated);
        assert!(report.events.len() <= 2);
        assert!(report.retained_bytes <= 256);
        assert!(report.terminal_error.is_some());
        assert_eq!(
            values(&report),
            vec![vec![Some("1".into())]],
            "earlier result survives later truncation/error"
        );
        assert_eq!(
            values(&run(&session, "SELECT count(*) FROM t").await),
            vec![vec![Some("1".into())]],
            "truncation drains through writes; real error stops trailing SQL"
        );
        let huge = run(
            &session,
            "SELECT zeroblob(1000000); INSERT INTO t VALUES(101)",
        )
        .await;
        assert!(huge.truncated);
        ok(&huge);
        assert_eq!(
            values(&run(&session, "SELECT count(*) FROM t").await),
            vec![vec![Some("2".into())]]
        );
        let zero = Session::connect_request(
            &request(":memory:", false, false),
            Limits {
                retained_bytes: 0,
                events: 0,
                ..Limits::default()
            },
        )
        .await
        .unwrap();
        let report = run(
            &zero,
            "CREATE TABLE t(v); INSERT INTO t VALUES(1); SELECT * FROM missing",
        )
        .await;
        assert!(
            report.events.is_empty()
                && report.retained_bytes == 0
                && report.truncated
                && report.terminal_error.is_some()
        );
        assert_eq!(report.outcome, Outcome::Failed);
    }

    #[tokio::test]
    async fn statement_cap_blocks_prepare_time_pragma_and_accepts_empty_tail() {
        let session = memory().await;
        ok(&run(&session, "PRAGMA foreign_keys=OFF").await);
        let prefix = "SELECT 1;".repeat(MAX_STATEMENTS);
        let report = run(&session, &(prefix.clone() + "PRAGMA foreign_keys=ON")).await;
        assert_eq!(report.outcome, Outcome::Failed);
        assert_eq!(
            values(&run(&session, "PRAGMA foreign_keys").await),
            vec![vec![Some("0".into())]]
        );
        ok(&run(
            &session,
            &(prefix + "; ; /* native empty tail */ -- comment\n"),
        )
        .await);
        ok(&run(&session, "PRAGMA foreign_keys=ON").await);
        assert_eq!(
            values(&run(&session, "PRAGMA foreign_keys").await),
            vec![vec![Some("1".into())]],
            "temporary budget authorizer is removed"
        );
    }

    #[tokio::test]
    async fn native_cancel_exact_key_and_connection_reuse() {
        let session = memory().await;
        let (execution, ready, release) = native_gate(&session, LONG);
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let old = execution.key;
        assert!(session.is_running(old).await);
        assert_eq!(
            session
                .cancel(ExecutionKey {
                    generation: old.generation + 1,
                    ..old
                })
                .await,
            CancelResult::NotRunning
        );
        assert_eq!(session.cancel(old).await, CancelResult::Requested);
        release.send(()).unwrap();
        let report = tokio::time::timeout(Duration::from_secs(2), execution.finish())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(report.outcome, Outcome::Cancelled);
        assert_eq!(report.transaction_state, ServerTransactionState::Idle);
        assert!(!report.connection_retired && !session.is_closed());
        assert_eq!(session.cancel(old).await, CancelResult::NotRunning);
        let (next, ready, release) = native_gate(
            &session,
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) SELECT sum(x) FROM n",
        );
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(session.cancel(old).await, CancelResult::NotRunning);
        release.send(()).unwrap();
        let report = next.finish().await.unwrap();
        ok(&report);
        assert_eq!(values(&report), vec![vec![Some("50005000".into())]]);
        ok(&run(&session, "SELECT 42").await);
    }

    #[tokio::test]
    async fn shutdown_rejects_queued_sql_and_drop_does_not_join_caller() {
        let session = memory().await;
        let finished = session.shared.finished.clone();
        let (execution, ready, release) = native_gate(&session, LONG);
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let queued = session.submit(LONG, CommitPolicy::Auto).unwrap();
        session.close();
        assert!(session.submit("SELECT 1", CommitPolicy::Auto).is_err());
        let started = Instant::now();
        drop(session);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "Drop must not join the gated worker"
        );
        release.send(()).unwrap();
        assert_eq!(
            execution.finish().await.unwrap().outcome,
            Outcome::Cancelled
        );
        let rejected = queued.finish().await.unwrap();
        assert_eq!(rejected.outcome, Outcome::Rejected);
        assert!(rejected.events.is_empty() && rejected.connection_retired);
        wait_finished(&finished).await;
    }

    #[tokio::test]
    async fn queue_input_and_abandoned_execution_are_bounded() {
        let session = memory().await;
        assert!(
            session
                .submit(
                    &" ".repeat(Limits::default().sql_bytes + 1),
                    CommitPolicy::Auto
                )
                .is_err()
        );
        let (execution, ready, release) = native_gate(&session, LONG);
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut pending = Vec::new();
        for _ in 0..QUEUE_CAPACITY {
            pending.push(
                session
                    .submit("CREATE TABLE should_not_run(v)", CommitPolicy::Auto)
                    .unwrap(),
            );
        }
        assert!(session.submit("SELECT 1", CommitPolicy::Auto).is_err());
        drop(pending);
        assert_eq!(session.cancel(execution.key).await, CancelResult::Requested);
        release.send(()).unwrap();
        assert_eq!(
            execution.finish().await.unwrap().outcome,
            Outcome::Cancelled
        );
        assert_eq!(
            run(&session, "SELECT * FROM should_not_run").await.outcome,
            Outcome::Failed
        );
    }
}

#[cfg(test)]
mod additional_tests {
    use super::*;
    use dbide_profiles::ConnectionProfile;

    #[tokio::test]
    async fn engine_rollback_error_returns_actual_idle_state() {
        let session = Session::connect_request(
            &ConnectionRequest {
                profile: ConnectionProfile {
                    engine: DatabaseEngine::Sqlite,
                    sqlite_path: Some(":memory:".into()),
                    ..ConnectionProfile::default()
                },
                password: String::new(),
            },
            Limits::default(),
        )
        .await
        .unwrap();
        session
            .execute(
                "CREATE TABLE t(v UNIQUE); INSERT INTO t VALUES(1)",
                CommitPolicy::Auto,
            )
            .await
            .unwrap();
        let failed = session
            .execute("INSERT OR ROLLBACK INTO t VALUES(1)", CommitPolicy::Manual)
            .await
            .unwrap();
        assert_eq!(failed.outcome, Outcome::Failed);
        assert_eq!(failed.transaction_state, ServerTransactionState::Idle);
        assert_eq!(session.transaction_state(), ServerTransactionState::Idle);
        let control = session
            .execute("PRAGMA busy_timeout=300000", CommitPolicy::Auto)
            .await
            .unwrap();
        assert_eq!(
            control.outcome,
            Outcome::Failed,
            "SQL must not replace the worker's bounded busy handler"
        );
        let report = session
            .execute("PRAGMA busy_timeout", CommitPolicy::Auto)
            .await
            .unwrap();
        assert!(report.events.iter().any(
            |event| matches!(event, Event::Row { values } if values == &[Some("250".into())])
        ));
    }
}
