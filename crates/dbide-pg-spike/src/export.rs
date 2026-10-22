//! Standalone PostgreSQL text export. Owns and retires its connection on every path.
//!
//! Values use PostgreSQL's UTF-8 text output, not a lossless typed interchange format.
//! NULL stays distinct from empty text. BYTEA (including nested BYTEA types) must
//! be explicitly converted to text by the caller. Nonfinite numeric scalars are
//! rejected; an explicit SQL text cast opts into their textual representation.
//! Arrays/composites retain PostgreSQL's textual representation, including any
//! nonfinite members; no numeric interpretation of structured text is attempted.
//! Payload counts each header/value as max(UTF-8 byte length, 1); NULL counts one.
//! Native prepare validates one statement and observes its result types, then
//! simple-query text protocol executes the original SQL once. This does not pin
//! a prepared execution plan or provide a namespace snapshot across those steps.
//! READ ONLY enforces PostgreSQL's transaction write restrictions; SQL functions
//! may still have other effects, such as session settings or advisory locks.
//! One operation deadline includes local validation, connection, setup, and rows.
//! Connection cleanup has a separate fixed three-second allowance in total:
//! at most two seconds for native cancellation, then the remaining time for
//! socket-task retirement. No phase renews the operation deadline.
//! A sink must return promptly: synchronous sink work cannot be preempted.
use crate::{ConnectionDiagnostic, ConnectionTarget, read_root_certificate};
use dbide_driver_api::stream::{
    StreamError, StreamErrorKind, StreamEvent, StreamLimits, StreamSummary,
};
use dbide_profiles::{ConnectionRequest, DatabaseEngine, TlsMode};
use futures_util::StreamExt;
use postgres_native_tls::MakeTlsConnector;
use sqlparser::{
    ast::{Expr, Query, SetExpr, Statement, Value, Visit, Visitor},
    dialect::PostgreSqlDialect,
    parser::Parser,
};
use std::{
    future::Future,
    ops::ControlFlow,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio_postgres::{
    Client, NoTls, SimpleQueryMessage,
    types::{Kind, Type},
};

const APPLICATION_NAME: &str = "dbide-readonly-stream";

/// Execute exactly once on a new connection in a fixed READ ONLY transaction.
/// Never borrows a tab/session or retries SQL. Success requires command completion
/// and stream EOF; callers must discard any partial output on an error.
pub async fn stream_read_only<F>(
    request: ConnectionRequest,
    sql: &str,
    limits: StreamLimits,
    cancelled: Arc<AtomicBool>,
    mut sink: F,
) -> Result<StreamSummary, StreamError>
where
    F: for<'a> FnMut(StreamEvent<'a>) -> Result<(), String> + Send,
{
    let deadline = tokio::time::Instant::now()
        .checked_add(limits.timeout)
        .ok_or_else(|| {
            error(
                StreamErrorKind::Configuration,
                "Export timeout exceeds supported range",
            )
        })?;
    validate(&request, sql, &limits)?;
    let (client, driver, tls) = watch(connect(&request), &cancelled, deadline).await?;
    let token = client.cancel_token();
    let result = watch(async {
        client.batch_execute("BEGIN READ ONLY").await.map_err(execution_error)?;
        // Parse/describe is not execution. Native preparation independently rejects
        // multiple statements and validates result OIDs before executing user SQL.
        let statement = client.prepare(sql).await.map_err(execution_error)?;
        if !statement.params().is_empty() || statement.columns().is_empty() {
            return Err(error(StreamErrorKind::Configuration, "Export requires a parameter-free SELECT with result columns"));
        }
        if statement.columns().iter().any(|c| contains_bytea(c.type_())) {
            return Err(error(StreamErrorKind::Configuration, "BYTEA export requires an explicit SQL text conversion"));
        }
        let numeric = statement.columns().iter().map(|c| numeric_scalar(c.type_())).collect::<Vec<_>>();
        let stream = client.simple_query_raw(sql).await.map_err(execution_error)?;
        tokio::pin!(stream);
        let mut summary = StreamSummary { rows: 0, payload_bytes: 0 };
        let mut columns_seen = false;
        let mut completed = false;
        while let Some(message) = stream.next().await {
            check_stop(&cancelled, deadline)?;
            match message.map_err(execution_error)? {
                SimpleQueryMessage::RowDescription(columns) => {
                    if columns_seen || completed || columns.len() != statement.columns().len() {
                        return Err(error(StreamErrorKind::Execution, "Unexpected export result description"));
                    }
                    let bytes = columns.iter().try_fold(0usize, |sum, col| sum.checked_add(col.name().len().max(1)))
                        .ok_or_else(|| error(StreamErrorKind::Limit, "Export header exceeds byte limit"))?;
                    add_payload(&mut summary, bytes, &limits)?;
                    let names = columns.iter().map(|c| c.name().to_owned()).collect::<Vec<_>>();
                    sink(StreamEvent::Columns(&names)).map_err(sink_error)?;
                    columns_seen = true;
                }
                SimpleQueryMessage::Row(row) => {
                    if !columns_seen || completed || row.len() != numeric.len() {
                        return Err(error(StreamErrorKind::Execution, "Unexpected export row"));
                    }
                    if summary.rows >= limits.max_rows {
                        return Err(error(StreamErrorKind::Limit, "Export exceeds row limit"));
                    }
                    // Check borrowed UTF-8 values and budgets before allocating copies.
                    let mut bytes = 0usize;
                    for (index, is_numeric) in numeric.iter().enumerate() {
                        let field_bytes = match row.try_get(index).map_err(|_| error(StreamErrorKind::Execution, "Export value is not valid UTF-8 text"))? {
                            Some(value) => {
                                if *is_numeric && matches!(value, "NaN" | "Infinity" | "-Infinity") {
                                    return Err(error(StreamErrorKind::Execution, "Nonfinite numeric export requires an explicit SQL text conversion"));
                                }
                                value.len().max(1)
                            }
                            None => 1,
                        };
                        bytes = bytes.checked_add(field_bytes).ok_or_else(|| error(StreamErrorKind::Limit, "Export row exceeds byte limit"))?;
                    }
                    add_payload(&mut summary, bytes, &limits)?;
                    let values = (0..row.len()).map(|i| row.try_get(i).map(|v| v.map(str::to_owned)))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| error(StreamErrorKind::Execution, "Export value is not valid UTF-8 text"))?;
                    sink(StreamEvent::Row(&values)).map_err(sink_error)?;
                    summary.rows += 1;
                }
                SimpleQueryMessage::CommandComplete(rows) => {
                    if completed || !columns_seen || rows != summary.rows {
                        return Err(error(StreamErrorKind::Execution, "Export completion count does not match streamed rows"));
                    }
                    completed = true;
                }
                _ => return Err(error(StreamErrorKind::Execution, "Unsupported export protocol message")),
            }
        }
        check_stop(&cancelled, deadline)?;
        if !completed {
            return Err(error(StreamErrorKind::Execution, "Export ended without command completion"));
        }
        Ok(summary)
    }, &cancelled, deadline).await;
    let cleanup_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    if result.is_err() {
        // Explicit native cancel precedes socket retirement, including sink/limit
        // failures. Both cancellation and retirement share one cleanup deadline.
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            match tls {
                Some(tls) => token.cancel_query(tls).await,
                None => token.cancel_query(NoTls).await,
            }
        })
        .await;
    }
    drop(client);
    if tokio::time::timeout_at(cleanup_deadline, driver.retire())
        .await
        .is_err()
        && result.is_ok()
    {
        return Err(error(
            StreamErrorKind::Timeout,
            "Export connection cleanup timed out",
        ));
    }
    result
}

fn error(kind: StreamErrorKind, message: &str) -> StreamError {
    StreamError::new(kind, message)
}
fn execution_error(_: tokio_postgres::Error) -> StreamError {
    // Server text may contain submitted SQL/credentials; expose no raw diagnostic.
    error(
        StreamErrorKind::Execution,
        "PostgreSQL export execution failed",
    )
}
fn sink_error(_: String) -> StreamError {
    error(StreamErrorKind::Sink, "Export sink failed")
}

fn add_payload(
    summary: &mut StreamSummary,
    bytes: usize,
    limits: &StreamLimits,
) -> Result<(), StreamError> {
    let total = summary
        .payload_bytes
        .checked_add(bytes as u64)
        .ok_or_else(|| error(StreamErrorKind::Limit, "Export exceeds payload limit"))?;
    if bytes > limits.max_row_bytes || total > limits.max_payload_bytes {
        return Err(error(StreamErrorKind::Limit, "Export exceeds byte limit"));
    }
    summary.payload_bytes = total;
    Ok(())
}
fn check_stop(cancelled: &AtomicBool, deadline: tokio::time::Instant) -> Result<(), StreamError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(error(StreamErrorKind::Cancelled, "Export cancelled"));
    }
    if tokio::time::Instant::now() >= deadline {
        return Err(error(StreamErrorKind::Timeout, "Export timed out"));
    }
    Ok(())
}
async fn watch<T>(
    future: impl Future<Output = Result<T, StreamError>>,
    cancelled: &AtomicBool,
    deadline: tokio::time::Instant,
) -> Result<T, StreamError> {
    check_stop(cancelled, deadline)?;
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => return Err(error(StreamErrorKind::Timeout, "Export timed out")),
            result = &mut future => { check_stop(cancelled, deadline)?; return result; },
            _ = tokio::time::sleep(Duration::from_millis(10)) => check_stop(cancelled, deadline)?,
        }
    }
}

struct Driver(tokio::task::JoinHandle<()>);
impl Driver {
    async fn retire(mut self) {
        self.0.abort();
        // Waiting confirms the owned socket task has actually been dropped.
        let _ = (&mut self.0).await;
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn connect(
    request: &ConnectionRequest,
) -> Result<(Client, Driver, Option<MakeTlsConnector>), StreamError> {
    let p = &request.profile;
    let target = ConnectionTarget::from(p);
    let mut config = tokio_postgres::Config::new();
    config
        .host(&p.host)
        .port(p.port)
        .dbname(&p.database)
        .user(&p.username)
        .password(&request.password)
        .connect_timeout(Duration::from_secs(p.connect_timeout_secs))
        .application_name(APPLICATION_NAME)
        .options("-c client_encoding=UTF8 -c default_transaction_read_only=on");
    let diagnostic = |e: tokio_postgres::Error| {
        let safe = ConnectionDiagnostic::from_pg(&e, target.clone());
        StreamError::new(StreamErrorKind::Connection, safe.message)
    };
    if p.tls_mode == TlsMode::Disable {
        config.ssl_mode(tokio_postgres::config::SslMode::Disable);
        let (client, connection) = config.connect(NoTls).await.map_err(diagnostic)?;
        let driver = Driver(tokio::spawn(async move {
            let _ = connection.await;
        }));
        Ok((client, driver, None))
    } else {
        config.ssl_mode(tokio_postgres::config::SslMode::Require);
        let mut builder = native_tls::TlsConnector::builder();
        if let Some(path) = &p.root_cert_path {
            let pem = read_root_certificate(path.clone()).await.map_err(|_| {
                error(
                    StreamErrorKind::Connection,
                    "TLS certificate could not be loaded",
                )
            })?;
            let cert = native_tls::Certificate::from_pem(&pem)
                .map_err(|_| error(StreamErrorKind::Connection, "TLS certificate is invalid"))?;
            builder.add_root_certificate(cert);
        }
        let tls = MakeTlsConnector::new(
            builder
                .build()
                .map_err(|_| error(StreamErrorKind::Connection, "TLS configuration failed"))?,
        );
        let (client, connection) = config.connect(tls.clone()).await.map_err(diagnostic)?;
        let driver = Driver(tokio::spawn(async move {
            let _ = connection.await;
        }));
        Ok((client, driver, Some(tls)))
    }
}

fn contains_bytea(ty: &Type) -> bool {
    if *ty == Type::BYTEA {
        return true;
    }
    match ty.kind() {
        Kind::Domain(base) | Kind::Array(base) | Kind::Range(base) | Kind::Multirange(base) => {
            contains_bytea(base)
        }
        Kind::Composite(fields) => fields.iter().any(|f| contains_bytea(f.type_())),
        _ => false,
    }
}
fn numeric_scalar(ty: &Type) -> bool {
    if let Kind::Domain(base) = ty.kind() {
        return numeric_scalar(base);
    }
    matches!(*ty, Type::FLOAT4 | Type::FLOAT8 | Type::NUMERIC)
}

fn validate(
    request: &ConnectionRequest,
    sql: &str,
    limits: &StreamLimits,
) -> Result<(), StreamError> {
    if request.profile.engine != DatabaseEngine::PostgreSql || request.profile.validate().is_err() {
        return Err(error(
            StreamErrorKind::Configuration,
            "PostgreSQL export profile is invalid",
        ));
    }
    if sql.len() > 64 * 1024 || sql.trim().is_empty() {
        return Err(error(
            StreamErrorKind::Configuration,
            "Export SQL must contain at most 64 KiB",
        ));
    }
    if limits.max_rows == 0
        || limits.max_rows > 1_000_000_000
        || limits.max_row_bytes == 0
        || limits.max_row_bytes > 16 * 1024 * 1024
        || limits.max_payload_bytes == 0
        || limits.max_payload_bytes > 16 * 1024 * 1024 * 1024
        || limits.timeout.is_zero()
        || limits.timeout > Duration::from_secs(3600)
    {
        return Err(error(
            StreamErrorKind::Configuration,
            "Export budgets must be positive and finite",
        ));
    }
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, sql).map_err(|_| {
        error(
            StreamErrorKind::Configuration,
            "Export SQL failed SELECT validation",
        )
    })?;
    if statements.len() != 1 || !matches!(statements[0], Statement::Query(_)) {
        return Err(error(
            StreamErrorKind::Configuration,
            "Export requires exactly one SELECT statement",
        ));
    }
    if let ControlFlow::Break(()) = statements.visit(&mut ReadOnlySelect) {
        return Err(error(
            StreamErrorKind::Configuration,
            "Export accepts only SELECT/CTE without INTO, locks, or parameters",
        ));
    }
    Ok(())
}
struct ReadOnlySelect;
impl Visitor for ReadOnlySelect {
    type Break = ();
    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        if !query.locks.is_empty() || !query.pipe_operators.is_empty() || !select_body(&query.body)
        {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        if matches!(expr, Expr::Value(v) if matches!(v.value, Value::Placeholder(_))) {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }
}
fn select_body(body: &SetExpr) -> bool {
    match body {
        SetExpr::Select(select) => select.into.is_none(),
        SetExpr::Query(_) => true, // Every nested query is checked by the visitor.
        SetExpr::SetOperation { left, right, .. } => select_body(left) && select_body(right),
        _ => false,
    }
}
