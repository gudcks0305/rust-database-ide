//! Dedicated MySQL text-protocol sessions. No pooling, reconnect, or SQL replay.
pub mod export;
pub use dbide_driver_api::{
    CommitPolicy, DatabaseEngine, Event, ExecutionKey, ExecutionReport, Limits, Outcome,
    ServerError, ServerTransactionState,
};
use dbide_profiles::{ConnectionRequest, TlsMode};
use mysql_async::{
    Conn, Opts, OptsBuilder, SslOpts, Value,
    consts::{ColumnType, StatusFlags},
    prelude::Queryable,
};
use serde::Serialize;
use std::{
    fmt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Notify, mpsc, oneshot};

const IO_DEADLINE: Duration = Duration::from_secs(300);
const CONTROL_DEADLINE: Duration = Duration::from_secs(5);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectError {
    Configuration,
    Tls,
    Timeout,
    Authentication,
    Database,
    Transport,
}
impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MySQL connection failed: {self:?}")
    }
}
impl std::error::Error for ConnectError {}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CancelResult {
    RequestedConnectionWillRetire,
    RequestFailedConnectionWillRetire,
    NotRunning,
}
struct Active {
    key: ExecutionKey,
    retire: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Sql,
    Commit,
    Rollback,
}
struct Request {
    key: ExecutionKey,
    sql: String,
    policy: CommitPolicy,
    kind: Kind,
    reply: oneshot::Sender<ExecutionReport>,
}
pub struct Execution {
    pub key: ExecutionKey,
    reply: oneshot::Receiver<ExecutionReport>,
}
impl Execution {
    pub async fn finish(self) -> Result<ExecutionReport, &'static str> {
        self.reply
            .await
            .map_err(|_| "MySQL session worker stopped without a report")
    }
}
#[derive(Clone)]
pub struct Session {
    sender: mpsc::Sender<Request>,
    active: Arc<Mutex<Option<Active>>>,
    closed: Arc<AtomicBool>,
    state: Arc<AtomicU8>,
    sequence: Arc<AtomicU64>,
    id: u64,
    limits: Limits,
    opts: Opts,
    thread_id: u32,
    stop: Arc<Notify>,
}
pub type MySqlSession = Session;
impl Session {
    pub async fn connect_request(
        request: &ConnectionRequest,
        limits: Limits,
    ) -> Result<Self, ConnectError> {
        let opts = build_opts(request)?;
        let timeout = Duration::from_secs(request.profile.connect_timeout_secs);
        let mut conn = tokio::time::timeout(timeout, Conn::new(opts.clone()))
            .await
            .map_err(|_| ConnectError::Timeout)?
            .map_err(connect_error)?;
        // Session default; SQL-capable users can change it, so this is not an authorization boundary.
        if request.profile.read_only {
            tokio::time::timeout(
                timeout,
                conn.query_drop("SET SESSION TRANSACTION READ ONLY"),
            )
            .await
            .map_err(|_| ConnectError::Timeout)?
            .map_err(connect_error)?;
        }
        let initial = wire_state(&conn);
        let thread_id = conn.id();
        let (sender, receiver) = mpsc::channel(16);
        let session = Self {
            sender,
            active: Arc::new(Mutex::new(None)),
            closed: Arc::new(AtomicBool::new(false)),
            state: Arc::new(AtomicU8::new(initial as u8)),
            sequence: Arc::new(AtomicU64::new(1)),
            id: dbide_driver_api::next_session_id(),
            limits,
            opts,
            thread_id,
            stop: Arc::new(Notify::new()),
        };
        tokio::spawn(actor(
            conn,
            receiver,
            session.active.clone(),
            session.closed.clone(),
            session.state.clone(),
            session.stop.clone(),
            limits,
        ));
        Ok(session)
    }
    pub fn transaction_state(&self) -> ServerTransactionState {
        if self.is_closed() {
            return ServerTransactionState::Unknown;
        }
        decode_state(self.state.load(Ordering::Acquire))
    }
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.stop.notify_one();
    }
    pub fn submit(&self, sql: &str, policy: CommitPolicy) -> Result<Execution, &'static str> {
        self.submit_kind(sql, policy, Kind::Sql)
    }
    pub fn submit_commit(&self, policy: CommitPolicy) -> Result<Execution, &'static str> {
        self.submit_kind("COMMIT", policy, Kind::Commit)
    }
    pub fn submit_rollback(&self, policy: CommitPolicy) -> Result<Execution, &'static str> {
        self.submit_kind("ROLLBACK", policy, Kind::Rollback)
    }
    fn submit_kind(
        &self,
        sql: &str,
        policy: CommitPolicy,
        kind: Kind,
    ) -> Result<Execution, &'static str> {
        if self.is_closed() {
            return Err("MySQL session is closed; reconnect explicitly");
        }
        if sql.len() > self.limits.sql_bytes {
            return Err("SQL exceeds MySQL byte limit");
        }
        let key = ExecutionKey {
            session: self.id,
            generation: 1,
            execution: self.sequence.fetch_add(1, Ordering::Relaxed),
        };
        let (reply, receiver) = oneshot::channel();
        self.sender
            .try_send(Request {
                key,
                sql: sql.into(),
                policy,
                kind,
                reply,
            })
            .map_err(|_| "MySQL session queue full or closed")?;
        Ok(Execution {
            key,
            reply: receiver,
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
    pub async fn is_running(&self, key: ExecutionKey) -> bool {
        self.active
            .lock()
            .await
            .as_ref()
            .is_some_and(|a| a.key == key)
    }
    /// Matches exact session/generation/execution before touching the owned server thread.
    /// Control task owns its lease even if the caller drops this future. Actor cannot reuse
    /// or release the target connection while KILL is in flight.
    pub async fn cancel(&self, key: ExecutionKey) -> CancelResult {
        let mut lease = self.active.clone().lock_owned().await;
        let Some(active) = lease.as_mut().filter(|a| a.key == key && !a.retire) else {
            return CancelResult::NotRunning;
        };
        active.retire = true;
        self.stop.notify_one();
        let opts = self.opts.clone();
        let thread = self.thread_id;
        let stop = self.stop.clone();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let result = tokio::time::timeout(CONTROL_DEADLINE, async {
                let mut control = Conn::new(opts).await?;
                control.query_drop(format!("KILL QUERY {thread}")).await?;
                Ok::<_, mysql_async::Error>(())
            })
            .await;
            stop.notify_one();
            drop(lease);
            let _ = tx.send(if matches!(result, Ok(Ok(()))) {
                CancelResult::RequestedConnectionWillRetire
            } else {
                CancelResult::RequestFailedConnectionWillRetire
            });
        });
        rx.await
            .unwrap_or(CancelResult::RequestFailedConnectionWillRetire)
    }
}
fn build_opts(request: &ConnectionRequest) -> Result<Opts, ConnectError> {
    request
        .profile
        .validate()
        .map_err(|_| ConnectError::Configuration)?;
    let p = &request.profile;
    if p.engine != DatabaseEngine::MySql {
        return Err(ConnectError::Configuration);
    }
    let mut builder = OptsBuilder::default()
        .ip_or_hostname(p.host.clone())
        .tcp_port(p.port)
        .user(Some(p.username.clone()))
        .pass(Some(request.password.clone()))
        .db_name(Some(p.database.clone()))
        .prefer_socket(false);
    if p.tls_mode != TlsMode::Disable {
        let mut tls = SslOpts::default();
        if let Some(path) = &p.root_cert_path {
            let metadata = std::fs::metadata(path).map_err(|_| ConnectError::Tls)?;
            if !metadata.is_file() || metadata.len() > 1024 * 1024 {
                return Err(ConnectError::Tls);
            }
            tls = tls.with_root_certs(vec![PathBuf::from(path).into()]);
        }
        builder = builder.ssl_opts(tls);
    }
    Ok(builder.into())
}
fn connect_error(error: mysql_async::Error) -> ConnectError {
    match error {
        mysql_async::Error::Server(e) if e.code == 1045 => ConnectError::Authentication,
        mysql_async::Error::Server(e) if e.code == 1049 => ConnectError::Database,
        mysql_async::Error::Io(mysql_async::IoError::Tls(_))
        | mysql_async::Error::Driver(mysql_async::DriverError::NoClientSslFlagFromServer) => {
            ConnectError::Tls
        }
        _ => ConnectError::Transport,
    }
}
fn decode_state(value: u8) -> ServerTransactionState {
    if value == ServerTransactionState::Idle as u8 {
        ServerTransactionState::Idle
    } else if value == ServerTransactionState::Active as u8 {
        ServerTransactionState::Active
    } else {
        ServerTransactionState::Unknown
    }
}
fn wire_state(conn: &Conn) -> ServerTransactionState {
    // Successful user commands already supplied a native OK/EOF status packet.
    // ERR carries no transaction flags and mysql_async clears last_ok_packet;
    // return Unknown then. DO 0 and even COM_PING overwrite ROW_COUNT().
    conn.last_ok_packet()
        .map(|packet| {
            if packet
                .status_flags()
                .contains(StatusFlags::SERVER_STATUS_IN_TRANS)
            {
                ServerTransactionState::Active
            } else {
                ServerTransactionState::Idle
            }
        })
        .unwrap_or(ServerTransactionState::Unknown)
}
async fn actor(
    mut conn: Conn,
    mut receiver: mpsc::Receiver<Request>,
    active: Arc<Mutex<Option<Active>>>,
    closed: Arc<AtomicBool>,
    state: Arc<AtomicU8>,
    stop: Arc<Notify>,
    limits: Limits,
) {
    let mut broken = false;
    loop {
        let request = tokio::select! {biased; _=stop.notified()=>break, request=receiver.recv()=>match request {Some(r)=>r,None=>break}};
        let started = Instant::now();
        let mut report = ExecutionReport {
            key: request.key,
            commit_policy: request.policy,
            transaction_state: ServerTransactionState::Unknown,
            outcome: Outcome::Rejected,
            terminal_error: None,
            events: vec![],
            notices: vec![],
            retained_bytes: 0,
            truncated: false,
            connection_retired: broken,
            elapsed_micros: 0,
        };
        if !broken && !closed.load(Ordering::Acquire) {
            *active.lock().await = Some(Active {
                key: request.key,
                retire: false,
            });
            let execution = async {
                if request.kind == Kind::Sql && request.policy == CommitPolicy::Manual {
                    match decode_state(state.load(Ordering::Acquire)) {
                        ServerTransactionState::Idle => {
                            conn.query_drop("START TRANSACTION").await?
                        }
                        ServerTransactionState::Active => {}
                        _ => {
                            report.terminal_error = Some(ServerError {
                                sqlstate: None,
                                message: "Transaction state unknown; manual SQL was not submitted"
                                    .into(),
                            });
                            return Ok(());
                        }
                    }
                }
                report.outcome = Outcome::Completed;
                match collect(&mut conn, &request.sql, &mut report, limits).await {
                    Ok(()) => {}
                    Err(error) => {
                        let transport = !matches!(error, mysql_async::Error::Server(_));
                        record_error(&mut report, &error, limits);
                        if transport {
                            return Err(error);
                        }
                    }
                }
                let actual = wire_state(&conn);
                report.transaction_state = actual;
                state.store(actual as u8, Ordering::Release);
                Ok::<_, mysql_async::Error>(())
            };
            tokio::select! {
                biased;
                _=stop.notified()=>{unknown(&mut report,"MySQL connection retired during execution; outcome may be unknown");broken=true;},
                result=tokio::time::timeout(IO_DEADLINE,execution)=>match result {Ok(Ok(()))=>{},Ok(Err(e))=>{record_error(&mut report,&e,limits);broken=true;},Err(_)=>{unknown(&mut report,"MySQL execution deadline exceeded; outcome may be unknown");broken=true;}}
            }
            let mut lease = active.lock().await;
            broken |= lease.as_ref().is_some_and(|a| a.retire)
                || closed.load(Ordering::Acquire)
                || conn.is_disconnected();
            *lease = None;
            report.connection_retired = broken;
            if broken {
                report.transaction_state = ServerTransactionState::Unknown;
                closed.store(true, Ordering::Release);
                state.store(ServerTransactionState::Unknown as u8, Ordering::Release);
            }
        }
        report.elapsed_micros = started.elapsed().as_micros();
        let _ = request.reply.send(report);
        if broken {
            break;
        }
    }
    // Drop the dedicated stream. No reconnect/commit/rollback replay.
    drop(conn);
    closed.store(true, Ordering::Release);
    state.store(ServerTransactionState::Unknown as u8, Ordering::Release);
    receiver.close();
    while let Some(request) = receiver.recv().await {
        let _ = request.reply.send(ExecutionReport {
            key: request.key,
            commit_policy: request.policy,
            transaction_state: ServerTransactionState::Unknown,
            outcome: Outcome::Rejected,
            terminal_error: Some(ServerError {
                sqlstate: None,
                message: "MySQL connection retired; reconnect explicitly".into(),
            }),
            events: vec![],
            notices: vec![],
            retained_bytes: 0,
            truncated: false,
            connection_retired: true,
            elapsed_micros: 0,
        });
    }
}
async fn collect(
    conn: &mut Conn,
    sql: &str,
    report: &mut ExecutionReport,
    limits: Limits,
) -> Result<(), mysql_async::Error> {
    let mut results = conn.query_iter(sql).await?;
    loop {
        let columns = results.columns_ref().to_vec();
        if columns.is_empty() {
            retain(
                report,
                Event::CommandComplete {
                    affected_rows: results.affected_rows(),
                },
                limits,
            );
        } else {
            retain(
                report,
                Event::ResultSetStarted {
                    columns: columns.iter().map(|c| c.name_str().into_owned()).collect(),
                },
                limits,
            );
        }
        let mut observed_rows = 0_u64;
        while let Some(row) = match results.next().await {
            Ok(row) => row,
            Err(error) => {
                // A later-result ERR is cached once by mysql_async. Consume that
                // cached response before reading status; never resend user SQL.
                if !error.is_fatal()
                    && let Err(cleanup) = results.drop_result().await
                    && cleanup.is_fatal()
                {
                    return Err(cleanup);
                }
                return Err(error);
            }
        } {
            observed_rows = observed_rows.saturating_add(1);
            if report.truncated {
                continue;
            }
            let values = row
                .unwrap()
                .into_iter()
                .zip(&columns)
                .map(|(v, c)| display_value(v, is_binary(c.column_type(), c.character_set())))
                .collect();
            retain(report, Event::Row { values }, limits);
        }
        if !columns.is_empty() {
            // SELECT completion closes its own result set before the next DML
            // command's update count. Count wire rows even after retention stops.
            retain(
                report,
                Event::CommandComplete {
                    affected_rows: observed_rows,
                },
                limits,
            );
        }
        if results.is_empty() {
            break;
        }
    }
    // is_empty() can be true for a cached later-result ERR. drop_result()
    // surfaces and consumes that terminal response before reading native status.
    results.drop_result().await
}
fn is_binary(kind: ColumnType, charset: u16) -> bool {
    charset == 63
        && matches!(
            kind,
            ColumnType::MYSQL_TYPE_BIT
                | ColumnType::MYSQL_TYPE_GEOMETRY
                | ColumnType::MYSQL_TYPE_STRING
                | ColumnType::MYSQL_TYPE_VAR_STRING
                | ColumnType::MYSQL_TYPE_VARCHAR
                | ColumnType::MYSQL_TYPE_BLOB
                | ColumnType::MYSQL_TYPE_TINY_BLOB
                | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
                | ColumnType::MYSQL_TYPE_LONG_BLOB
        )
}
fn display_value(value: Value, binary: bool) -> Option<String> {
    match value {
        Value::NULL => None,
        Value::Bytes(bytes) => Some(if binary {
            format!(
                "0x{}",
                bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
            )
        } else {
            match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(e) => format!(
                    "0x{}",
                    e.into_bytes()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                ),
            }
        }),
        other => Some(other.as_sql(false)),
    }
}
fn retain(report: &mut ExecutionReport, event: Event, limits: Limits) {
    let size = match &event {
        Event::ResultSetStarted { columns } => columns.iter().map(String::len).sum::<usize>() + 32,
        Event::Row { values } => {
            values
                .iter()
                .map(|v| v.as_ref().map_or(1, String::len) + 8)
                .sum::<usize>()
                + 24
        }
        Event::Error { sqlstate, message } => {
            sqlstate.as_ref().map_or(0, String::len) + message.len() + 24
        }
        _ => 32,
    };
    if report.events.len() >= limits.events
        || size > limits.retained_bytes.saturating_sub(report.retained_bytes)
    {
        report.truncated = true;
        return;
    }
    report.retained_bytes += size;
    report.events.push(event);
}
fn record_error(report: &mut ExecutionReport, error: &mysql_async::Error, limits: Limits) {
    let (sqlstate, message, outcome) = match error {
        mysql_async::Error::Server(e) => (
            Some(e.state.clone()),
            e.message.chars().take(1024).collect::<String>(),
            if e.code == 1317 {
                Outcome::Cancelled
            } else {
                Outcome::Failed
            },
        ),
        _ => (
            None,
            "MySQL transport/protocol failure; submitted SQL outcome may be unknown".into(),
            Outcome::OutcomeUnknown,
        ),
    };
    report.outcome = outcome;
    report.terminal_error = Some(ServerError {
        sqlstate: sqlstate.clone(),
        message: message.clone(),
    });
    retain(report, Event::Error { sqlstate, message }, limits);
}
fn unknown(report: &mut ExecutionReport, message: &str) {
    report.outcome = Outcome::OutcomeUnknown;
    report.terminal_error = Some(ServerError {
        sqlstate: None,
        message: message.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbide_profiles::ConnectionProfile;
    #[test]
    fn tls_is_verified_and_socket_fallback_disabled() {
        let request = ConnectionRequest {
            profile: ConnectionProfile {
                engine: DatabaseEngine::MySql,
                ..ConnectionProfile::default()
            },
            password: "secret".into(),
        };
        for mode in [TlsMode::Require, TlsMode::VerifyFull] {
            let mut request = request.clone();
            request.profile.tls_mode = mode;
            let opts = build_opts(&request).unwrap();
            let ssl = opts.ssl_opts().unwrap();
            assert!(!ssl.skip_domain_validation());
            assert!(!ssl.accept_invalid_certs());
            assert!(ssl.tls_hostname_override().is_none());
            assert!(!opts.prefer_socket());
        }
    }
    #[test]
    fn null_empty_decimal_unicode_and_binary_are_distinct() {
        assert_eq!(display_value(Value::NULL, false), None);
        assert_eq!(display_value(Value::Bytes(vec![]), false), Some("".into()));
        assert_eq!(
            display_value(Value::Bytes(b"1234567890.000000001".to_vec()), false),
            Some("1234567890.000000001".into())
        );
        assert_eq!(
            display_value(Value::Bytes("한글 😀".as_bytes().to_vec()), false),
            Some("한글 😀".into())
        );
        assert_eq!(
            display_value(Value::Bytes(vec![0, 255]), true),
            Some("0x00ff".into())
        );
        assert_eq!(
            display_value(Value::Bytes(vec![255]), false),
            Some("0xff".into())
        );
    }
    #[test]
    fn budgets_are_bounded_and_terminal_error_is_separate() {
        let mut r = ExecutionReport {
            key: ExecutionKey {
                session: 1,
                generation: 1,
                execution: 1,
            },
            commit_policy: CommitPolicy::Auto,
            transaction_state: ServerTransactionState::Unknown,
            outcome: Outcome::Completed,
            terminal_error: None,
            events: vec![],
            notices: vec![],
            retained_bytes: 0,
            truncated: false,
            connection_retired: false,
            elapsed_micros: 0,
        };
        retain(
            &mut r,
            Event::Row {
                values: vec![Some("한".repeat(5))],
            },
            Limits {
                events: 0,
                retained_bytes: 0,
                sql_bytes: 10,
            },
        );
        unknown(&mut r, "terminal survives full budget");
        assert!(r.events.is_empty());
        assert!(r.truncated);
        assert_eq!(r.retained_bytes, 0);
        assert!(r.terminal_error.is_some());
    }
}
