//! Owned PostgreSQL READ ONLY SQL cursor. The original SELECT is declared once;
//! subsequent demand executes only FETCH. No driver portal extensions or replay.
//!
//! Rows are PostgreSQL text output, with the same BYTEA/nonfinite restrictions and
//! source-byte accounting as standalone export. Driver allocation of one row or
//! protocol frame precedes these application limits. Pausing deliberately holds
//! an idle-in-transaction backend. Synchronous sinks must return promptly.
//! Native prepare validates columns but does not pin the later DECLARE plan or
//! provide a namespace snapshot. READ ONLY retains PostgreSQL's documented
//! transaction restrictions; functions may have other session/external effects.
//! PostgreSQL SQL FETCH materializes a wire batch before returning its rows.
//! Each logical demand therefore uses a small first FETCH (up to 16 rows), then
//! chunks of up to 64 rows, publishing each chunk before the demand completes.
//! Inner sorts/aggregates may still delay the first row.
use crate::export::{connect, contains_bytea, numeric_scalar, validate};
use dbide_driver_api::stream::{
    StreamError, StreamErrorKind, StreamEvent, StreamLimits, StreamSummary,
};
use dbide_profiles::ConnectionRequest;
use futures_util::StreamExt;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Receiver, TryRecvError},
    },
    time::Duration,
};
use tokio::time::Instant;
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

#[derive(Clone, Copy, Debug)]
pub struct CursorOptions {
    pub fetch_rows: u32,
    pub idle_timeout: Duration,
    pub lifetime_timeout: Duration,
}
impl Default for CursorOptions {
    fn default() -> Self {
        Self {
            fetch_rows: 200,
            idle_timeout: Duration::from_secs(60),
            lifetime_timeout: Duration::from_secs(300),
        }
    }
}
pub enum CursorEvent<'a> {
    Data(StreamEvent<'a>),
    /// Data ended for this batch. Even `eof: true` precedes cleanup; only the
    /// returned Ok summary confirms successful cursor/transaction retirement.
    BatchEnd {
        eof: bool,
        fetch_sequence: u64,
    },
}

/// Fetch the first batch automatically, then one batch per explicit demand.
/// The caller must supply a bounded channel (normally sync_channel(1)). More
/// than one outstanding demand is rejected. Dropping demand retires the backend.
/// Active time (including validation, connection, sink, CLOSE and ROLLBACK) is
/// limited by `limits.timeout <= 30s`; waiting demand uses only idle/lifetime time.
#[allow(clippy::too_many_arguments)]
pub async fn query_cursor<F>(
    request: ConnectionRequest,
    sql: &str,
    limits: StreamLimits,
    options: CursorOptions,
    cancel: Arc<AtomicBool>,
    demand: Receiver<u64>,
    mut sink: F,
) -> Result<StreamSummary, StreamError>
where
    F: for<'a> FnMut(CursorEvent<'a>) -> Result<(), String> + Send,
{
    let started = Instant::now();
    validate(&request, sql, &limits)?;
    if options.fetch_rows == 0
        || options.fetch_rows > 200
        || options.idle_timeout.is_zero()
        || options.idle_timeout > Duration::from_secs(300)
        || options.lifetime_timeout.is_zero()
        || options.lifetime_timeout > Duration::from_secs(600)
        || limits.timeout > Duration::from_secs(30)
    {
        return Err(error(
            StreamErrorKind::Configuration,
            "Cursor budgets must be positive and finite",
        ));
    }
    let mut budget = Budget {
        active_deadline: started + limits.timeout,
        lifetime: started + options.lifetime_timeout,
    };
    let mut control = Control {
        cancel,
        demand,
        pending: None,
    };
    let (client, driver, tls) = budget.run(connect(&request), &mut control).await?;
    let token = client.cancel_token();
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let name = format!("dbide_cursor_{}", SEQUENCE.fetch_add(1, Ordering::Relaxed));
    let result = async {
        let (numeric, expected_columns) = budget
            .run(
                async {
                    client
                        .batch_execute(
                            "SET application_name = 'dbide-readonly-cursor'; BEGIN READ ONLY",
                        )
                        .await
                        .map_err(execution_error)?;
                    let statement = client.prepare(sql).await.map_err(execution_error)?;
                    if !statement.params().is_empty() || statement.columns().is_empty() {
                        return Err(error(
                            StreamErrorKind::Configuration,
                            "Cursor requires a parameter-free SELECT with result columns",
                        ));
                    }
                    if statement
                        .columns()
                        .iter()
                        .any(|c| contains_bytea(c.type_()))
                    {
                        return Err(error(
                            StreamErrorKind::Configuration,
                            "BYTEA cursor requires an explicit SQL text conversion",
                        ));
                    }
                    let numeric = statement
                        .columns()
                        .iter()
                        .map(|c| numeric_scalar(c.type_()))
                        .collect::<Vec<_>>();
                    let columns = statement
                        .columns()
                        .iter()
                        .map(|c| c.name().to_owned())
                        .collect::<Vec<_>>();
                    // Prefix only; preserve original bytes including trailing semicolons
                    // and comments. Native prepare has independently required one query.
                    client
                        .batch_execute(&format!("DECLARE {name} NO SCROLL CURSOR FOR {sql}"))
                        .await
                        .map_err(execution_error)?;
                    Ok((numeric, columns))
                },
                &mut control,
            )
            .await?;
        let mut summary = StreamSummary {
            rows: 0,
            payload_bytes: 0,
        };
        let mut columns_emitted = false;
        let mut fetch_sequence = 1u64;
        loop {
            let deadline = budget.deadline()?;
            let eof = budget
                .run(
                    fetch(
                        &client,
                        &name,
                        &numeric,
                        &expected_columns,
                        &limits,
                        options.fetch_rows,
                        &mut summary,
                        &mut columns_emitted,
                        &mut sink,
                        &control.cancel.clone(),
                        deadline,
                        fetch_sequence,
                    ),
                    &mut control,
                )
                .await?;
            if eof {
                break;
            }
            budget.deadline()?;
            let paused = Instant::now();
            control
                .wait(options.idle_timeout, budget.lifetime, fetch_sequence + 1)
                .await?;
            // Shift only by actual demand wait, preserving aggregate active
            // accounting for all work between phases as well as inside them.
            budget.active_deadline += paused.elapsed();
            fetch_sequence += 1;
        }
        budget
            .run(
                async {
                    client
                        .batch_execute(&format!("CLOSE {name}; ROLLBACK"))
                        .await
                        .map_err(execution_error)
                },
                &mut control,
            )
            .await?;
        Ok(summary)
    }
    .await;
    // All failed active/paused operations attempt native cancellation and rollback,
    // then retire the socket. These phases share one three-second allowance.
    let cleanup = Instant::now() + Duration::from_secs(3);
    if result.is_err() {
        let _ = tokio::time::timeout_at(
            cleanup.min(Instant::now() + Duration::from_secs(2)),
            async {
                match tls {
                    Some(tls) => token.cancel_query(tls).await,
                    None => token.cancel_query(NoTls).await,
                }
            },
        )
        .await;
        let _ = tokio::time::timeout_at(cleanup, client.batch_execute("ROLLBACK")).await;
    }
    drop(client);
    if tokio::time::timeout_at(cleanup, driver.retire())
        .await
        .is_err()
        && result.is_ok()
    {
        return Err(error(
            StreamErrorKind::Timeout,
            "Cursor connection cleanup timed out",
        ));
    }
    result
}

struct Control {
    cancel: Arc<AtomicBool>,
    demand: Receiver<u64>,
    pending: Option<u64>,
}
impl Control {
    fn check(&mut self, deadline: Instant) -> Result<(), StreamError> {
        check_stop(&self.cancel, deadline)?;
        match self.demand.try_recv() {
            Ok(_) if self.pending.is_some() => {
                return Err(error(
                    StreamErrorKind::Configuration,
                    "Cursor demand exceeds one outstanding fetch",
                ));
            }
            Ok(sequence) => self.pending = Some(sequence),
            Err(TryRecvError::Disconnected) => {
                return Err(error(
                    StreamErrorKind::Cancelled,
                    "Cursor demand channel closed",
                ));
            }
            Err(TryRecvError::Empty) => {}
        }
        Ok(())
    }
    async fn wait(
        &mut self,
        idle: Duration,
        lifetime: Instant,
        expected: u64,
    ) -> Result<(), StreamError> {
        let deadline = lifetime.min(Instant::now() + idle);
        loop {
            self.check(deadline)?;
            if let Some(sequence) = self.pending.take() {
                check_stop(&self.cancel, deadline)?;
                if sequence != expected {
                    return Err(error(
                        StreamErrorKind::Configuration,
                        "Cursor demand sequence is stale or out of order",
                    ));
                }
                return Ok(());
            }
            tokio::time::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }
}
struct Budget {
    active_deadline: Instant,
    lifetime: Instant,
}
impl Budget {
    fn deadline(&self) -> Result<Instant, StreamError> {
        if Instant::now() >= self.active_deadline || Instant::now() >= self.lifetime {
            return Err(error(StreamErrorKind::Timeout, "Cursor timed out"));
        }
        Ok(self.active_deadline.min(self.lifetime))
    }
    async fn run<T>(
        &mut self,
        future: impl Future<Output = Result<T, StreamError>>,
        control: &mut Control,
    ) -> Result<T, StreamError> {
        let deadline = self.deadline()?;
        async {
            control.check(deadline)?;
            tokio::pin!(future);
            loop {
                tokio::select! {
                    biased;
                    _ = tokio::time::sleep_until(deadline) => return Err(error(StreamErrorKind::Timeout, "Cursor timed out")),
                    result = &mut future => { control.check(deadline)?; return result; },
                    _ = tokio::time::sleep(Duration::from_millis(10)) => control.check(deadline)?,
                }
            }
        }.await
    }
}
fn error(kind: StreamErrorKind, message: &str) -> StreamError {
    StreamError::new(kind, message)
}
fn execution_error(failure: tokio_postgres::Error) -> StreamError {
    // A fixed SQLSTATE is useful feedback without exposing SQL, server detail,
    // values, connection strings, or credentials in native error messages.
    let state = failure.code().map(|code| code.code()).filter(|code| {
        code.len() == 5
            && code
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    });
    StreamError::new(
        StreamErrorKind::Execution,
        state.map_or_else(
            || "PostgreSQL cursor execution failed".to_owned(),
            |code| format!("PostgreSQL cursor execution failed (SQLSTATE {code})"),
        ),
    )
}
fn sink_error(_: String) -> StreamError {
    error(StreamErrorKind::Sink, "Cursor sink failed")
}
fn check_stop(cancel: &AtomicBool, deadline: Instant) -> Result<(), StreamError> {
    if cancel.load(Ordering::Acquire) {
        return Err(error(StreamErrorKind::Cancelled, "Cursor cancelled"));
    }
    if Instant::now() >= deadline {
        return Err(error(StreamErrorKind::Timeout, "Cursor timed out"));
    }
    Ok(())
}
fn add_payload(
    summary: &mut StreamSummary,
    bytes: usize,
    limits: &StreamLimits,
) -> Result<(), StreamError> {
    let total = summary
        .payload_bytes
        .checked_add(bytes as u64)
        .ok_or_else(|| error(StreamErrorKind::Limit, "Cursor exceeds payload limit"))?;
    if bytes > limits.max_row_bytes || total > limits.max_payload_bytes {
        return Err(error(StreamErrorKind::Limit, "Cursor exceeds byte limit"));
    }
    summary.payload_bytes = total;
    Ok(())
}
#[allow(clippy::too_many_arguments)]
async fn fetch<F>(
    client: &Client,
    name: &str,
    numeric: &[bool],
    expected_columns: &[String],
    limits: &StreamLimits,
    batch_size: u32,
    summary: &mut StreamSummary,
    columns_emitted: &mut bool,
    sink: &mut F,
    cancel: &AtomicBool,
    deadline: Instant,
    fetch_sequence: u64,
) -> Result<bool, StreamError>
where
    F: for<'a> FnMut(CursorEvent<'a>) -> Result<(), String> + Send,
{
    let mut remaining = batch_size;
    let mut chunk_size = remaining.min(16);
    let mut eof = false;
    while remaining > 0 {
        check_stop(cancel, deadline)?;
        let fetched = fetch_chunk(
            client,
            name,
            numeric,
            expected_columns,
            limits,
            chunk_size,
            summary,
            columns_emitted,
            sink,
            cancel,
            deadline,
        )
        .await?;
        check_stop(cancel, deadline)?;
        if fetched < u64::from(chunk_size) {
            eof = true;
            break;
        }
        remaining -= chunk_size;
        chunk_size = remaining.min(64);
    }
    sink(CursorEvent::BatchEnd {
        eof,
        fetch_sequence,
    })
    .map_err(sink_error)?;
    check_stop(cancel, deadline)?;
    Ok(eof)
}

#[allow(clippy::too_many_arguments)]
async fn fetch_chunk<F>(
    client: &Client,
    name: &str,
    numeric: &[bool],
    expected_columns: &[String],
    limits: &StreamLimits,
    batch_size: u32,
    summary: &mut StreamSummary,
    columns_emitted: &mut bool,
    sink: &mut F,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<u64, StreamError>
where
    F: for<'a> FnMut(CursorEvent<'a>) -> Result<(), String> + Send,
{
    let stream = client
        .simple_query_raw(&format!("FETCH FORWARD {batch_size} FROM {name}"))
        .await
        .map_err(execution_error)?;
    tokio::pin!(stream);
    let mut described = false;
    let mut completed = false;
    let mut fetched = 0u64;
    while let Some(message) = stream.next().await {
        check_stop(cancel, deadline)?;
        match message.map_err(execution_error)? {
            SimpleQueryMessage::RowDescription(columns) => {
                if described
                    || completed
                    || columns.len() != expected_columns.len()
                    || columns
                        .iter()
                        .zip(expected_columns)
                        .any(|(col, expected)| col.name() != expected)
                {
                    return Err(error(
                        StreamErrorKind::Execution,
                        "Unexpected cursor result description",
                    ));
                }
                described = true;
                if !*columns_emitted {
                    let bytes = expected_columns
                        .iter()
                        .try_fold(0usize, |sum, col| sum.checked_add(col.len().max(1)))
                        .ok_or_else(|| {
                            error(StreamErrorKind::Limit, "Cursor header exceeds byte limit")
                        })?;
                    add_payload(summary, bytes, limits)?;
                    sink(CursorEvent::Data(StreamEvent::Columns(expected_columns)))
                        .map_err(sink_error)?;
                    *columns_emitted = true;
                }
            }
            SimpleQueryMessage::Row(row) => {
                if !described
                    || completed
                    || row.len() != numeric.len()
                    || fetched >= u64::from(batch_size)
                {
                    return Err(error(StreamErrorKind::Execution, "Unexpected cursor row"));
                }
                if summary.rows >= limits.max_rows {
                    return Err(error(StreamErrorKind::Limit, "Cursor exceeds row limit"));
                }
                let mut bytes = 0usize;
                for (index, is_numeric) in numeric.iter().enumerate() {
                    let field = row.try_get(index).map_err(|_| {
                        error(
                            StreamErrorKind::Execution,
                            "Cursor value is not valid UTF-8 text",
                        )
                    })?;
                    if *is_numeric
                        && field.is_some_and(|v| matches!(v, "NaN" | "Infinity" | "-Infinity"))
                    {
                        return Err(error(
                            StreamErrorKind::Execution,
                            "Nonfinite numeric cursor requires an explicit SQL text conversion",
                        ));
                    }
                    bytes = bytes
                        .checked_add(field.map_or(1, |v| v.len().max(1)))
                        .ok_or_else(|| {
                            error(StreamErrorKind::Limit, "Cursor row exceeds byte limit")
                        })?;
                }
                add_payload(summary, bytes, limits)?;
                let values = (0..row.len())
                    .map(|i| row.try_get(i).map(|v| v.map(str::to_owned)))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| {
                        error(
                            StreamErrorKind::Execution,
                            "Cursor value is not valid UTF-8 text",
                        )
                    })?;
                sink(CursorEvent::Data(StreamEvent::Row(&values))).map_err(sink_error)?;
                fetched += 1;
                summary.rows += 1;
            }
            SimpleQueryMessage::CommandComplete(rows) => {
                if completed || !described || rows != fetched {
                    return Err(error(
                        StreamErrorKind::Execution,
                        "Cursor completion count does not match fetched rows",
                    ));
                }
                completed = true;
            }
            _ => {
                return Err(error(
                    StreamErrorKind::Execution,
                    "Unsupported cursor protocol message",
                ));
            }
        }
    }
    check_stop(cancel, deadline)?;
    if !completed {
        return Err(error(
            StreamErrorKind::Execution,
            "Cursor fetch ended without command completion",
        ));
    }
    Ok(fetched)
}
