//! PostgreSQL M0 contract experiment, not the product driver API.
//! Verified TLS, wire transaction status, and bound CRUD batches; no pooling, SQL replay, full typed rows, or COPY.
use dbide_profiles::{ConnectionRequest, DatabaseEngine, TlsMode};
use futures_util::{StreamExt, future::poll_fn};
use postgres_native_tls::MakeTlsConnector;
use serde::Serialize;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};
use tokio_postgres::{AsyncMessage, CancelToken, NoTls, SimpleQueryMessage};
pub mod batch;
mod table_metadata;
pub use table_metadata::{
    MAX_TABLE_METADATA_BYTES, table_metadata_fingerprint, table_metadata_query,
};
pub mod connection;
pub mod cursor;
pub mod export;
pub use batch::{ParameterizedStatement, TableIdentityGuard};
pub use connection::{ConnectionDiagnostic, ConnectionErrorKind, ConnectionTarget};

use dbide_driver_api::next_session_id;
pub use dbide_driver_api::{
    CommitPolicy, Event, ExecutionKey, ExecutionReport, Limits, Notice, Outcome, ServerError,
    ServerTransactionState,
};

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
enum RequestKind {
    Batch,
    Sql,
    Commit,
    Rollback,
}
struct Request {
    key: ExecutionKey,
    kind: RequestKind,
    sql: String,
    batch: Vec<ParameterizedStatement>,
    guard: Option<TableIdentityGuard>,
    policy: CommitPolicy,
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
            .map_err(|_| "session worker stopped without a report")
    }
}
#[derive(Clone)]
pub struct Session {
    sender: mpsc::Sender<Request>,
    active: Arc<AsyncMutex<Option<Active>>>,
    cancel: CancelToken,
    driver_abort: tokio::task::AbortHandle,
    sequence: Arc<AtomicU64>,
    id: u64,
    generation: u64,
    limits: Limits,
    closed: Arc<AtomicBool>,
    cancel_tls: Option<MakeTlsConnector>,
    transaction_state: Arc<AtomicU8>,
}
impl Session {
    /// Intended ONLY for isolated test PostgreSQL. URL must be supplied explicitly.
    pub async fn connect(url: &str, limits: Limits) -> Result<Self, tokio_postgres::Error> {
        let (client, connection) = tokio_postgres::connect(url, NoTls).await?;
        Ok(Self::start(client, connection, limits, None))
    }
    /// Typed configuration avoids URI interpolation and never logs credentials.
    pub async fn connect_request(
        request: &ConnectionRequest,
        limits: Limits,
    ) -> Result<Self, ConnectionDiagnostic> {
        let target = ConnectionTarget::from(&request.profile);
        if request.profile.engine != DatabaseEngine::PostgreSql {
            let mut error = ConnectionDiagnostic::configuration(target);
            error.message = "PostgreSQL adapter cannot open this database engine";
            error.hint = "Select the matching database engine adapter";
            return Err(error);
        }
        request
            .profile
            .validate()
            .map_err(|_| ConnectionDiagnostic::configuration(target.clone()))?;
        let p = &request.profile;
        let mut config = tokio_postgres::Config::new();
        config
            .host(&p.host)
            .port(p.port)
            .dbname(&p.database)
            .user(&p.username)
            .password(&request.password)
            .connect_timeout(Duration::from_secs(p.connect_timeout_secs));
        if p.read_only {
            config.options("-c default_transaction_read_only=on");
        }
        let connect = async {
            if p.tls_mode == TlsMode::Disable {
                config.ssl_mode(tokio_postgres::config::SslMode::Disable);
                let (client, connection) = config
                    .connect(NoTls)
                    .await
                    .map_err(|e| ConnectionDiagnostic::from_pg(&e, target.clone()))?;
                Ok(Self::start(client, connection, limits, None))
            } else {
                config.ssl_mode(tokio_postgres::config::SslMode::Require);
                // Both TLS modes verify system trust and hostname. Never downgrade to plaintext.
                let mut builder = native_tls::TlsConnector::builder();
                if let Some(path) = &p.root_cert_path {
                    let pem = read_root_certificate(path.clone())
                        .await
                        .map_err(|_| ConnectionDiagnostic::tls(target.clone()))?;
                    let certificate = native_tls::Certificate::from_pem(&pem)
                        .map_err(|_| ConnectionDiagnostic::tls(target.clone()))?;
                    builder.add_root_certificate(certificate);
                }
                let native = builder
                    .build()
                    .map_err(|_| ConnectionDiagnostic::tls(target.clone()))?;
                let tls = MakeTlsConnector::new(native);
                let (client, connection) = config
                    .connect(tls.clone())
                    .await
                    .map_err(|e| ConnectionDiagnostic::from_pg(&e, target.clone()))?;
                Ok(Self::start(client, connection, limits, Some(tls)))
            }
        };
        tokio::time::timeout(Duration::from_secs(p.connect_timeout_secs), connect)
            .await
            .unwrap_or_else(|_| Err(ConnectionDiagnostic::timeout(target)))
    }
    fn start<S, T>(
        client: tokio_postgres::Client,
        mut connection: tokio_postgres::Connection<S, T>,
        limits: Limits,
        cancel_tls: Option<MakeTlsConnector>,
    ) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let cancel = client.cancel_token();
        let notices = Arc::new(Mutex::new(Vec::<Notice>::new()));
        let notice_sink = notices.clone();
        let closed = Arc::new(AtomicBool::new(false));
        let driver_closed = closed.clone();
        let actor_closed = closed.clone();
        let driver = tokio::spawn(async move {
            loop {
                match poll_fn(|cx| connection.poll_message(cx)).await {
                    Some(Ok(AsyncMessage::Notice(n))) => {
                        let mut sink = notice_sink.lock().unwrap();
                        if sink.len() < 16 {
                            sink.push(Notice {
                                severity: n.severity().into(),
                                sqlstate: n.code().code().into(),
                                message: n.message().chars().take(1024).collect(),
                            });
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
            driver_closed.store(true, Ordering::Release);
        });
        let driver_abort = driver.abort_handle();
        let active = Arc::new(AsyncMutex::new(None));
        let actor_active = active.clone();
        let (sender, mut receiver) = mpsc::channel::<Request>(16);
        let transaction_state = Arc::new(AtomicU8::new(ServerTransactionState::Unknown as u8));
        let actor_state = transaction_state.clone();
        let actor_cancel_tls = cancel_tls.clone();
        tokio::spawn(async move {
            let initial =
                tokio::time::timeout(Duration::from_secs(5), read_transaction_state(&client, ""))
                    .await
                    .ok()
                    .flatten();
            let mut server_state = initial.unwrap_or(ServerTransactionState::Unknown);
            actor_state.store(server_state as u8, Ordering::Release);
            let mut broken = initial.is_none();
            while let Some(request) = receiver.recv().await {
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
                if !broken {
                    notices.lock().unwrap().clear();
                    *actor_active.lock().await = Some(Active {
                        key: request.key,
                        retire: false,
                    });
                    if request.kind == RequestKind::Batch {
                        let result = batch::run(
                            &client,
                            batch::BatchRequest {
                                statements: &request.batch,
                                guard: request.guard.as_ref(),
                                policy: request.policy,
                                initial: server_state,
                                cancel_tls: actor_cancel_tls.as_ref(),
                            },
                            &mut report,
                            &actor_active,
                            limits,
                        )
                        .await;
                        broken |= result.broken;
                        server_state = result.state;
                    } else {
                        let mut dispatch = true;
                        if request.kind == RequestKind::Commit
                            && server_state == ServerTransactionState::Failed
                        {
                            dispatch = false;
                            report.terminal_error = Some(ServerError {
                                sqlstate: None,
                                message:
                                    "Transaction is failed; Rollback is required before Commit"
                                        .into(),
                            });
                        }
                        if dispatch
                            && request.kind == RequestKind::Sql
                            && request.policy == CommitPolicy::Manual
                            && server_state == ServerTransactionState::Idle
                        {
                            match tokio::time::timeout(
                                Duration::from_secs(5),
                                read_transaction_state(&client, "BEGIN"),
                            )
                            .await
                            .ok()
                            .flatten()
                            {
                                Some(ServerTransactionState::Active) => {
                                    server_state = ServerTransactionState::Active;
                                    actor_state.store(server_state as u8, Ordering::Release);
                                }
                                _ => {
                                    broken = true;
                                    dispatch = false;
                                    report.outcome = Outcome::OutcomeUnknown;
                                    report.terminal_error=Some(ServerError{sqlstate:None,message:"Could not establish manual transaction; user SQL was not submitted".into()});
                                }
                            }
                        }
                        if dispatch && actor_active.lock().await.as_ref().is_some_and(|a| a.retire)
                        {
                            dispatch = false;
                            broken = true;
                            report.terminal_error = Some(ServerError {
                                sqlstate: None,
                                message: "Execution cancelled before SQL dispatch".into(),
                            });
                        }
                        let mut failure = None;
                        if dispatch {
                            match client.simple_query_raw(&request.sql).await {
                                Ok(stream) => {
                                    tokio::pin!(stream);
                                    while let Some(item) = stream.next().await {
                                        match item {
                                            Ok(SimpleQueryMessage::RowDescription(cols)) => retain(
                                                &mut report,
                                                Event::ResultSetStarted {
                                                    columns: cols
                                                        .iter()
                                                        .map(|c| c.name().to_owned())
                                                        .collect(),
                                                },
                                                limits,
                                            ),
                                            Ok(SimpleQueryMessage::Row(row)) => retain(
                                                &mut report,
                                                Event::Row {
                                                    values: (0..row.len())
                                                        .map(|i| row.get(i).map(str::to_owned))
                                                        .collect(),
                                                },
                                                limits,
                                            ),
                                            Ok(SimpleQueryMessage::CommandComplete(
                                                affected_rows,
                                            )) => retain(
                                                &mut report,
                                                Event::CommandComplete { affected_rows },
                                                limits,
                                            ),
                                            Ok(_) => {}
                                            Err(error) => {
                                                failure = Some(error);
                                                break;
                                            }
                                        }
                                    }
                                }
                                Err(error) => failure = Some(error),
                            }
                            report.outcome = Outcome::Completed;
                        }
                        if let Some(error) = failure {
                            let state = error.code().map(|s| s.code().to_owned());
                            report.outcome = if state.as_deref() == Some("57014") {
                                Outcome::Cancelled
                            } else if error.as_db_error().is_some() {
                                Outcome::Failed
                            } else {
                                broken = true;
                                Outcome::OutcomeUnknown
                            };
                            let message = error
                                .as_db_error()
                                .map(|db| db.message().to_owned())
                                .unwrap_or_else(|| error.to_string())
                                .chars()
                                .take(1024)
                                .collect::<String>();
                            report.terminal_error = Some(ServerError {
                                sqlstate: state.clone(),
                                message: message.clone(),
                            });
                            retain(
                                &mut report,
                                Event::Error {
                                    sqlstate: state,
                                    message,
                                },
                                limits,
                            );
                        }
                    }
                    // ErrorResponse precedes ReadyForQuery. An empty FIFO barrier
                    // consumes actual ReadyForQuery status before any later user SQL.
                    if !broken {
                        match tokio::time::timeout(
                            Duration::from_secs(5),
                            read_transaction_state(&client, ""),
                        )
                        .await
                        .ok()
                        .flatten()
                        {
                            Some(state) => server_state = state,
                            None => {
                                broken = true;
                                report.outcome = Outcome::OutcomeUnknown;
                            }
                        }
                    }
                    // CancelRequest has no acknowledgement and can arrive late even after terminal
                    // query response. Retire this backend after EVERY accepted cancel attempt.
                    let mut lease = actor_active.lock().await;
                    broken |= lease.as_ref().is_some_and(|a| a.retire) || client.is_closed();
                    *lease = None;
                    report.connection_retired = broken;
                    if broken {
                        server_state = ServerTransactionState::Unknown;
                    }
                    report.transaction_state = server_state;
                    actor_state.store(server_state as u8, Ordering::Release);
                    report.notices = std::mem::take(&mut *notices.lock().unwrap());
                    if broken {
                        actor_closed.store(true, Ordering::Release);
                        driver.abort();
                    }
                }
                report.elapsed_micros = started.elapsed().as_micros();
                let _ = request.reply.send(report);
            }
            actor_state.store(ServerTransactionState::Unknown as u8, Ordering::Release);
            actor_closed.store(true, Ordering::Release);
            driver.abort();
        });
        Self {
            sender,
            active,
            cancel,
            driver_abort,
            sequence: Arc::new(AtomicU64::new(1)),
            id: next_session_id(),
            generation: 1,
            limits,
            closed,
            cancel_tls,
            transaction_state,
        }
    }
    pub fn transaction_state(&self) -> ServerTransactionState {
        if self.is_closed() {
            return ServerTransactionState::Unknown;
        }
        match self.transaction_state.load(Ordering::Acquire) {
            x if x == ServerTransactionState::Idle as u8 => ServerTransactionState::Idle,
            x if x == ServerTransactionState::Active as u8 => ServerTransactionState::Active,
            x if x == ServerTransactionState::Failed as u8 => ServerTransactionState::Failed,
            _ => ServerTransactionState::Unknown,
        }
    }
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.driver_abort.is_finished()
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.driver_abort.abort();
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
    pub async fn commit(&self, policy: CommitPolicy) -> Result<ExecutionReport, &'static str> {
        self.submit_commit(policy)?.finish().await
    }
    pub async fn rollback(&self, policy: CommitPolicy) -> Result<ExecutionReport, &'static str> {
        self.submit_rollback(policy)?.finish().await
    }
    fn submit_kind(
        &self,
        sql: &str,
        policy: CommitPolicy,
        kind: RequestKind,
    ) -> Result<Execution, &'static str> {
        self.enqueue(sql, policy, kind, vec![], None)
    }
    pub fn submit_batch(
        &self,
        statements: Vec<ParameterizedStatement>,
        policy: CommitPolicy,
    ) -> Result<Execution, &'static str> {
        batch::validate(&statements, self.limits)?;
        self.enqueue("", policy, RequestKind::Batch, statements, None)
    }
    pub async fn execute_batch(
        &self,
        statements: Vec<ParameterizedStatement>,
        policy: CommitPolicy,
    ) -> Result<ExecutionReport, &'static str> {
        self.submit_batch(statements, policy)?.finish().await
    }
    pub fn submit_guarded_batch(
        &self,
        guard: TableIdentityGuard,
        statements: Vec<ParameterizedStatement>,
        policy: CommitPolicy,
    ) -> Result<Execution, &'static str> {
        batch::validate_guarded(&guard, &statements, self.limits)?;
        self.enqueue("", policy, RequestKind::Batch, statements, Some(guard))
    }
    pub async fn execute_guarded_batch(
        &self,
        guard: TableIdentityGuard,
        statements: Vec<ParameterizedStatement>,
        policy: CommitPolicy,
    ) -> Result<ExecutionReport, &'static str> {
        self.submit_guarded_batch(guard, statements, policy)?
            .finish()
            .await
    }
    fn enqueue(
        &self,
        sql: &str,
        policy: CommitPolicy,
        kind: RequestKind,
        batch: Vec<ParameterizedStatement>,
        guard: Option<TableIdentityGuard>,
    ) -> Result<Execution, &'static str> {
        if sql.len() > self.limits.sql_bytes {
            return Err("SQL exceeds spike byte limit");
        }
        let key = ExecutionKey {
            session: self.id,
            generation: self.generation,
            execution: self.sequence.fetch_add(1, Ordering::Relaxed),
        };
        let (reply, receiver) = oneshot::channel();
        self.sender
            .try_send(Request {
                key,
                kind,
                sql: sql.into(),
                batch,
                guard,
                policy,
                reply,
            })
            .map_err(|_| "session queue full or closed")?;
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
    pub async fn is_running(&self, key: ExecutionKey) -> bool {
        self.active
            .lock()
            .await
            .as_ref()
            .is_some_and(|a| a.key == key)
    }
    /// Separate path; never enqueued behind the query. Dropping this future after lease
    /// acquisition still leaves retire=true, preventing reuse of an ambiguously cancelled backend.
    pub async fn cancel(&self, key: ExecutionKey) -> CancelResult {
        let mut lease = self.active.lock().await;
        let Some(active) = lease.as_mut().filter(|a| a.key == key && !a.retire) else {
            return CancelResult::NotRunning;
        };
        active.retire = true;
        // Even a lost cancel request or dropped cancel future cannot leave a busy
        // connection waiting forever. Retirement deadline is independent of actor I/O.
        let abort = self.driver_abort.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            abort.abort();
        });
        let send = async {
            match &self.cancel_tls {
                Some(tls) => self.cancel.cancel_query(tls.clone()).await,
                None => self.cancel.cancel_query(NoTls).await,
            }
        };
        match tokio::time::timeout(Duration::from_secs(3), send).await {
            Ok(Ok(())) => CancelResult::RequestedConnectionWillRetire,
            _ => CancelResult::RequestFailedConnectionWillRetire,
        }
    }
}
/// Reads protocol status only, never infers transaction state from SQL text.
async fn read_transaction_state(
    client: &tokio_postgres::Client,
    sql: &str,
) -> Option<ServerTransactionState> {
    let stream = client.simple_query_raw(sql).await.ok()?;
    tokio::pin!(stream);
    while let Some(item) = stream.next().await {
        item.ok()?;
    }
    match stream.as_ref().get_ref().transaction_status()? {
        tokio_postgres::TransactionStatus::Idle => Some(ServerTransactionState::Idle),
        tokio_postgres::TransactionStatus::InTransaction => Some(ServerTransactionState::Active),
        tokio_postgres::TransactionStatus::InError => Some(ServerTransactionState::Failed),
    }
}

/// Blocking filesystem work stays off the actor runtime. On Unix, nonblocking
/// open prevents a FIFO/device path from blocking before regular-file validation.
async fn read_root_certificate(path: String) -> Result<Vec<u8>, ()> {
    tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > 128 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "CA must be a regular PEM file under128KiB",
            ));
        }
        let mut pem = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(file, 128 * 1024 + 1), &mut pem)?;
        if pem.len() > 128 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "CA exceeds128KiB",
            ));
        }
        Ok(pem)
    })
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}

fn retain(report: &mut ExecutionReport, event: Event, limits: Limits) {
    // Account allocated scalar strings and container slots rather than row count alone.
    let bytes = std::mem::size_of::<Event>()
        + match &event {
            Event::ResultSetStarted { columns } => {
                columns.capacity() * std::mem::size_of::<String>()
                    + columns.iter().map(|s| s.capacity()).sum::<usize>()
            }
            Event::Row { values } => {
                values.capacity() * std::mem::size_of::<Option<String>>()
                    + values.iter().flatten().map(|s| s.capacity()).sum::<usize>()
            }
            Event::Error { sqlstate, message } => {
                message.capacity() + sqlstate.as_ref().map_or(0, |s| s.capacity())
            }
            Event::CommandComplete { .. } | Event::CommandCompleteUnknown => 0,
        };
    if !report.truncated
        && report.events.len() < limits.events
        && report.retained_bytes.saturating_add(bytes) <= limits.retained_bytes
    {
        report.retained_bytes += bytes;
        report.events.push(event);
    } else {
        report.truncated = true;
    }
}
pub mod contract;

#[cfg(all(test, unix))]
mod root_certificate_tests {
    use super::*;
    #[tokio::test]
    async fn fifo_root_is_rejected_without_blocking_connection_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let fifo = temp.path().join("ca.pem");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let mut profile = dbide_profiles::ConnectionProfile::fixture();
        profile.tls_mode = TlsMode::VerifyFull;
        profile.root_cert_path = Some(fifo.to_string_lossy().into_owned());
        let request = ConnectionRequest {
            profile,
            password: "fixture-only".into(),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            Session::connect_request(&request, Limits::default()),
        )
        .await
        .expect("FIFO blocked the current-thread connection runtime");
        assert_eq!(
            result.err().expect("FIFO must be rejected").kind,
            ConnectionErrorKind::Tls
        );
    }
}
