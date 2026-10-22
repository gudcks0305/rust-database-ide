//! Progressive, bounded captures on independent connections. A committed prefix
//! remains readable after failure or cancellation; only `Complete` means EOF and
//! native cleanup succeeded. These captures are never CSV export inputs.

use crate::{
    MAX_HEADER_BYTES, MAX_PAGES, MAX_ROWS, PageIndex, PageLimits, StoredPage, encode_record,
    execution,
};
use arc_swap::ArcSwap;
use dbide_driver_api::{DatabaseEngine, stream::StreamEvent};
use dbide_export::{ExportError, ExportErrorKind, ExportProgress, QueryRequest, QuerySummary};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveMode {
    Stream,
    PostgresCursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveState {
    Starting,
    Streaming,
    AwaitingFetch,
    Complete,
    Cancelled,
    Failed,
}

impl LiveState {
    fn terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Cancelled | Self::Failed)
    }
}

#[derive(Clone, Debug)]
pub struct LiveSnapshot {
    pub state: LiveState,
    /// Fully written and committed rows; never includes an unfinished record.
    pub rows: u64,
    pub pages: usize,
    /// Changes for every fully written row or producer state transition. The
    /// final page may grow; earlier pages remain immutable across batches.
    pub version: u64,
    pub error: Option<String>,
    /// Authoritative native counters and captured target, published only on EOF.
    pub summary: Option<QuerySummary>,
}

struct Metadata {
    columns: Arc<Vec<String>>,
    pages: Vec<PageIndex>,
    encoded_bytes: u64,
    version: u64,
}

struct Shared {
    snapshot: ArcSwap<LiveSnapshot>,
    metadata: Mutex<Metadata>,
    spool: OnceLock<Arc<NamedTempFile>>,
    cancel: Arc<AtomicBool>,
    fetch_pending: AtomicBool,
    next_fetch: AtomicU64,
    limits: PageLimits,
}

/// UI control handle. Wrap in `Arc` when sharing: dropping the last control
/// handle cancels the producer without waiting on network, disk, or a worker.
/// The worker holds shared storage, never a control handle.
pub struct LiveQuery {
    shared: Arc<Shared>,
    demand: mpsc::SyncSender<u64>,
    mode: LiveMode,
    identity: u64,
}

impl LiveQuery {
    /// Constant-size immutable snapshot load. No disk I/O, metadata mutex, queue
    /// drain, or waiting for the producer occurs on this UI-safe path.
    pub fn snapshot(&self) -> LiveSnapshot {
        let mut snapshot = (**self.shared.snapshot.load()).clone();
        if snapshot.state == LiveState::AwaitingFetch
            && self.shared.fetch_pending.load(Ordering::Acquire)
        {
            snapshot.state = LiveState::Streaming;
        }
        snapshot
    }

    pub fn identity(&self) -> u64 {
        self.identity
    }

    pub fn is_finished(&self) -> bool {
        self.shared.snapshot.load().state.terminal()
    }

    /// Read on a background thread. Metadata is copied under a short lock;
    /// reopening, seeking, reading and parsing use an independent file handle.
    pub fn read_page(&self, index: usize) -> Result<StoredPage, String> {
        self.read_page_versioned(index).map(|(page, _)| page)
    }

    /// Returns the exact published metadata version used for this read, even if
    /// the producer commits more records while disk I/O is in progress.
    pub fn read_page_versioned(&self, index: usize) -> Result<(StoredPage, u64), String> {
        let (page, columns, version) = {
            let metadata = self
                .shared
                .metadata
                .lock()
                .map_err(|_| "result metadata poisoned")?;
            let page = *metadata
                .pages
                .get(index)
                .ok_or("page index out of bounds")?;
            if page.len > self.shared.limits.max_page_bytes
                || page.count > self.shared.limits.max_rows_per_page
                || page
                    .offset
                    .checked_add(page.len as u64)
                    .is_none_or(|end| end > metadata.encoded_bytes)
            {
                return Err("invalid live page index".into());
            }
            (page, metadata.columns.clone(), metadata.version)
        };
        let spool = self.shared.spool.get().ok_or("result spool unavailable")?;
        let mut file = spool
            .reopen()
            .map_err(|error| format!("cannot open live page: {error}"))?;
        file.seek(SeekFrom::Start(page.offset))
            .map_err(|error| format!("cannot seek live page: {error}"))?;
        let mut bytes = vec![0; page.len];
        file.read_exact(&mut bytes)
            .map_err(|error| format!("cannot read live page: {error}"))?;
        let mut rows = Vec::with_capacity(page.count);
        for record in bytes.split_inclusive(|byte| *byte == b'\n') {
            if rows.len() >= page.count || record.last() != Some(&b'\n') {
                return Err("invalid live page record count or delimiter".into());
            }
            let row: Vec<Option<String>> = serde_json::from_slice(&record[..record.len() - 1])
                .map_err(|error| format!("invalid live page JSON: {error}"))?;
            if row.len() != columns.len() {
                return Err("invalid live page row width".into());
            }
            rows.push(row);
        }
        if rows.len() != page.count {
            return Err("invalid live page record count".into());
        }
        Ok((
            StoredPage {
                columns: (*columns).clone(),
                rows,
                row_start: page.row_start,
            },
            version,
        ))
    }

    /// One bounded, deduplicated demand. Fetch never runs SQL or waits here.
    pub fn fetch_next(&self) -> Result<(), String> {
        if self.mode != LiveMode::PostgresCursor {
            return Err("this live query streams automatically".into());
        }
        if self.shared.snapshot.load().state != LiveState::AwaitingFetch {
            return Err("cursor is not awaiting a fetch".into());
        }
        if self
            .shared
            .fetch_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }
        if let Err(error) = self
            .demand
            .try_send(self.shared.next_fetch.load(Ordering::Acquire))
        {
            self.shared.fetch_pending.store(false, Ordering::Release);
            return Err(format!("cannot request cursor batch: {error}"));
        }
        Ok(())
    }

    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Release);
    }
}

impl Drop for LiveQuery {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Starts a private worker immediately. Configuration checks are memory-only;
/// opening sources, private file creation, and all native work happen there.
/// Streams have a total deadline capped at 30 seconds. PostgreSQL cursors use an
/// aggregate 30-second active budget, 60-second idle budget, and 300-second life.
/// Native cleanup has its separately bounded allowance; OS writes cannot be
/// preempted. Neither cleanup nor a late OS write can turn expiry into success.
pub fn start_live_query(
    request: QueryRequest,
    limits: PageLimits,
    mode: LiveMode,
) -> Result<LiveQuery, ExportError> {
    let started = Instant::now();
    validate_request(&request, limits, mode)?;
    let (query, demand) = new_control(limits, mode);
    let shared = query.shared.clone();
    std::thread::Builder::new()
        .name("dbide-live-result".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(request, mode, shared.clone(), demand, started)
            }));
            let result = outcome.unwrap_or_else(|_| Err(execution("live query worker failed")));
            if let Err(error) = result {
                let state = if shared.cancel.load(Ordering::Acquire)
                    || error.kind == ExportErrorKind::Cancelled
                {
                    LiveState::Cancelled
                } else {
                    LiveState::Failed
                };
                publish_state(&shared, state, Some(error.message), None);
            }
        })
        .map_err(execution)?;
    Ok(query)
}

fn new_control(limits: PageLimits, mode: LiveMode) -> (LiveQuery, mpsc::Receiver<u64>) {
    let (sender, receiver) = mpsc::sync_channel(1);
    let shared = Arc::new(Shared {
        snapshot: ArcSwap::from_pointee(LiveSnapshot {
            state: LiveState::Starting,
            rows: 0,
            pages: 0,
            version: 0,
            error: None,
            summary: None,
        }),
        metadata: Mutex::new(Metadata {
            columns: Arc::new(Vec::new()),
            pages: Vec::new(),
            encoded_bytes: 0,
            version: 0,
        }),
        spool: OnceLock::new(),
        cancel: Arc::new(AtomicBool::new(false)),
        fetch_pending: AtomicBool::new(false),
        next_fetch: AtomicU64::new(2),
        limits,
    });
    (
        LiveQuery {
            shared,
            demand: sender,
            mode,
            identity: dbide_driver_api::next_session_id(),
        },
        receiver,
    )
}

fn configuration(message: impl ToString) -> ExportError {
    ExportError {
        kind: ExportErrorKind::Configuration,
        message: message.to_string(),
    }
}

fn validate_request(
    request: &QueryRequest,
    limits: PageLimits,
    mode: LiveMode,
) -> Result<(), ExportError> {
    crate::validate(limits, request)?;
    if request.options.max_row_bytes == 0
        || request.options.max_row_bytes > dbide_export::MAX_ROW_BYTES
        || request.options.timeout.is_zero()
        || request.options.timeout > dbide_export::MAX_TIMEOUT
    {
        return Err(configuration(
            "live source requires positive bounded row bytes and timeout <=600 seconds",
        ));
    }
    if request.sql.trim().is_empty() || request.sql.len() > dbide_project::MAX_SQL_BYTES {
        return Err(configuration(
            "SQL must be nonempty and at most 65536 UTF-8 bytes; nothing submitted",
        ));
    }
    dbide_project::validate_target_profile(&request.connection.profile).map_err(configuration)?;
    if request
        .options
        .expect_sql_sha256
        .as_deref()
        .is_some_and(|hash| hash != dbide_project::digest_sql(&request.sql))
    {
        return Err(configuration("SQL SHA-256 changed; nothing submitted"));
    }
    let profile = &request.connection.profile;
    let database = if profile.engine == DatabaseEngine::Sqlite {
        "main"
    } else {
        &profile.database
    };
    let target =
        dbide_profiles::StoredExecutionTarget::capture(profile, database).map_err(configuration)?;
    if request
        .options
        .expect_target_fingerprint
        .as_deref()
        .is_some_and(|hash| hash != target.endpoint_fingerprint)
    {
        return Err(configuration(
            "target fingerprint changed; nothing submitted",
        ));
    }
    if mode == LiveMode::PostgresCursor && profile.engine != DatabaseEngine::PostgreSql {
        return Err(configuration("server cursor mode requires PostgreSQL"));
    }
    Ok(())
}

fn publish_state(
    shared: &Shared,
    state: LiveState,
    error: Option<String>,
    summary: Option<QuerySummary>,
) {
    let previous = shared.snapshot.load();
    let version = previous.version + 1;
    if let Ok(mut metadata) = shared.metadata.lock() {
        metadata.version = version;
    }
    shared.snapshot.store(Arc::new(LiveSnapshot {
        state,
        rows: previous.rows,
        pages: previous.pages,
        version,
        error,
        summary,
    }));
}

struct LiveBuilder {
    shared: Arc<Shared>,
    file: File,
    columns: bool,
    rows: u64,
    encoded_bytes: u64,
    current: Option<PageIndex>,
    pages: usize,
    indices: Vec<PageIndex>,
    started: Instant,
    timeout: Duration,
}

impl LiveBuilder {
    fn new(shared: Arc<Shared>, timeout: Duration, started: Instant) -> Result<Self, ExportError> {
        let spool = Arc::new(
            tempfile::Builder::new()
                .prefix("dbide-live-result-")
                .tempfile()
                .map_err(execution)?,
        );
        let file = spool.reopen().map_err(execution)?;
        shared
            .spool
            .set(spool)
            .map_err(|_| execution("live result spool already initialized"))?;
        let builder = Self {
            shared,
            file,
            columns: false,
            rows: 0,
            encoded_bytes: 0,
            current: None,
            pages: 0,
            indices: Vec::new(),
            started,
            timeout,
        };
        builder.check()?;
        Ok(builder)
    }

    fn check(&self) -> Result<(), ExportError> {
        if self.shared.cancel.load(Ordering::Acquire) {
            Err(ExportError {
                kind: ExportErrorKind::Cancelled,
                message: "live query cancelled".into(),
            })
        } else if self.started.elapsed() >= self.timeout {
            Err(execution("live query capture deadline exceeded"))
        } else {
            Ok(())
        }
    }

    fn push(&mut self, event: StreamEvent<'_>) -> Result<(), String> {
        self.check().map_err(|error| error.message)?;
        match event {
            StreamEvent::Columns(columns) => {
                if self.columns {
                    return Err("source emitted columns more than once".into());
                }
                let encoded = encode_record(
                    columns,
                    self.shared.limits.max_page_bytes.min(MAX_HEADER_BYTES),
                )?;
                self.append(&encoded)?;
                self.shared
                    .metadata
                    .lock()
                    .map_err(|_| "result metadata poisoned")?
                    .columns = Arc::new(columns.to_vec());
                self.columns = true;
                publish_state(&self.shared, LiveState::Streaming, None, None);
            }
            StreamEvent::Row(row) => {
                if !self.columns {
                    return Err("source emitted row before columns".into());
                }
                if row.len()
                    != self
                        .shared
                        .metadata
                        .lock()
                        .map_err(|_| "result metadata poisoned")?
                        .columns
                        .len()
                {
                    return Err("source row width differs from columns".into());
                }
                if self.rows >= MAX_ROWS {
                    return Err("result store row limit exceeded".into());
                }
                let encoded =
                    encode_record(row, self.shared.limits.max_page_bytes).map_err(|error| {
                        format!(
                            "cannot encode result row within {}-byte page limit: {error}",
                            self.shared.limits.max_page_bytes
                        )
                    })?;
                if self.current.is_some_and(|page| {
                    page.count == self.shared.limits.max_rows_per_page
                        || page.len + encoded.len() > self.shared.limits.max_page_bytes
                }) {
                    self.current = None;
                }
                let new_page = self.current.is_none();
                if new_page {
                    if self.pages >= MAX_PAGES {
                        return Err("result store page limit exceeded (65536)".into());
                    }
                    self.current = Some(PageIndex {
                        row_start: self.rows,
                        count: 0,
                        offset: self.encoded_bytes,
                        len: 0,
                    });
                }
                self.append(&encoded)?;
                self.check().map_err(|error| error.message)?;
                let page = self.current.as_mut().expect("current page created");
                page.count += 1;
                page.len += encoded.len();
                self.rows += 1;
                if new_page {
                    self.indices.push(*page);
                    self.pages += 1;
                } else {
                    *self.indices.last_mut().expect("current page indexed") = *page;
                }
                // Publish before returning to a producer that may block for an
                // arbitrary time before its next callback. A row-triggered
                // timer would leave the last received row hidden during a stall.
                self.publish(LiveState::Streaming, None)
                    .map_err(|error| error.message)?;
            }
        }
        self.check().map_err(|error| error.message)
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        let next = self
            .encoded_bytes
            .checked_add(bytes.len() as u64)
            .filter(|next| *next <= self.shared.limits.max_spool_bytes)
            .ok_or("encoded result spool byte limit exceeded")?;
        self.file
            .write_all(bytes)
            .map_err(|error| format!("cannot write live result spool: {error}"))?;
        self.encoded_bytes = next;
        Ok(())
    }

    fn batch_end(&mut self, eof: bool, fetch_sequence: u64) -> Result<(), String> {
        self.end_batch(eof, fetch_sequence, || {})
    }

    fn end_batch(
        &mut self,
        eof: bool,
        fetch_sequence: u64,
        after_publication: impl FnOnce(),
    ) -> Result<(), String> {
        self.check().map_err(|error| error.message)?;
        // Never append a later FETCH to a published previous batch's last page.
        self.current = None;
        self.shared
            .next_fetch
            .store(fetch_sequence + 1, Ordering::Release);
        // Open admission before publishing AwaitingFetch. Once that state is
        // visible, an immediate UI demand owns this flag; the worker must never
        // clear it afterward. EOF keeps admission closed through native cleanup.
        if !eof {
            self.shared.fetch_pending.store(false, Ordering::Release);
        }
        self.publish(
            if eof {
                LiveState::Streaming
            } else {
                LiveState::AwaitingFetch
            },
            None,
        )
        .map_err(|error| error.message)?;
        after_publication();
        Ok(())
    }

    fn finish(mut self, summary: QuerySummary) -> Result<(), ExportError> {
        self.check()?;
        if !self.columns {
            return Err(execution("source omitted columns"));
        }
        if summary.rows != self.rows {
            return Err(execution("source and stored row counts differ"));
        }
        if self.pages == 0 {
            self.indices.push(PageIndex {
                row_start: 0,
                count: 0,
                offset: self.encoded_bytes,
                len: 0,
            });
            self.pages = 1;
        }
        self.file.sync_all().map_err(execution)?;
        self.check()?;
        self.publish(LiveState::Complete, Some(summary))
    }

    fn publish(
        &mut self,
        state: LiveState,
        summary: Option<QuerySummary>,
    ) -> Result<(), ExportError> {
        let version = self.shared.snapshot.load().version + 1;
        let mut metadata = self
            .shared
            .metadata
            .lock()
            .map_err(|_| execution("result metadata poisoned"))?;
        for index in metadata.pages.len().saturating_sub(1)..self.indices.len() {
            if let Some(existing) = metadata.pages.get_mut(index) {
                *existing = self.indices[index];
            } else {
                metadata.pages.push(self.indices[index]);
            }
        }
        metadata.encoded_bytes = self.encoded_bytes;
        metadata.version = version;
        self.shared.snapshot.store(Arc::new(LiveSnapshot {
            state,
            rows: self.rows,
            pages: self.pages,
            version,
            error: None,
            summary,
        }));
        Ok(())
    }
}

fn run(
    mut request: QueryRequest,
    mode: LiveMode,
    shared: Arc<Shared>,
    demand: mpsc::Receiver<u64>,
    started: Instant,
) -> Result<(), ExportError> {
    let active_timeout = request.options.timeout.min(Duration::from_secs(30));
    let capture_timeout = if mode == LiveMode::PostgresCursor {
        Duration::from_secs(300)
    } else {
        active_timeout
    };
    let mut builder = LiveBuilder::new(shared.clone(), capture_timeout, started)?;
    let result = match mode {
        LiveMode::Stream => {
            request.options.timeout = active_timeout.saturating_sub(started.elapsed());
            dbide_export::stream_query(
                request,
                shared.cancel.clone(),
                Arc::new(ExportProgress::default()),
                |event| builder.push(event),
            )
        }
        LiveMode::PostgresCursor => {
            run_cursor(request, shared, demand, active_timeout, &mut builder)
        }
    };
    match result {
        Ok(summary) => {
            builder.publish(LiveState::Streaming, None)?;
            builder.finish(summary)
        }
        Err(error) => {
            builder.publish(LiveState::Streaming, None)?;
            Err(error)
        }
    }
}

fn run_cursor(
    request: QueryRequest,
    shared: Arc<Shared>,
    demand: mpsc::Receiver<u64>,
    active_timeout: Duration,
    builder: &mut LiveBuilder,
) -> Result<QuerySummary, ExportError> {
    let profile = &request.connection.profile;
    let target = dbide_profiles::StoredExecutionTarget::capture(profile, &profile.database)
        .map_err(configuration)?;
    let sql_sha256 = dbide_project::digest_sql(&request.sql);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(execution)?;
    let limits = dbide_driver_api::stream::StreamLimits {
        max_rows: request.options.max_rows,
        max_row_bytes: request.options.max_row_bytes,
        max_payload_bytes: request.options.max_bytes,
        timeout: active_timeout.saturating_sub(builder.started.elapsed()),
    };
    if limits.timeout.is_zero() {
        return Err(execution(
            "live query active deadline exceeded before connection",
        ));
    }
    let options = dbide_pg_spike::cursor::CursorOptions {
        lifetime_timeout: builder.timeout.saturating_sub(builder.started.elapsed()),
        ..dbide_pg_spike::cursor::CursorOptions::default()
    };
    if options.lifetime_timeout.is_zero() {
        return Err(execution("live query lifetime exceeded before connection"));
    }
    let mut sink_error = None;
    let result = runtime.block_on(dbide_pg_spike::cursor::query_cursor(
        request.connection,
        &request.sql,
        limits,
        options,
        shared.cancel.clone(),
        demand,
        |event| {
            let result = match event {
                dbide_pg_spike::cursor::CursorEvent::Data(event) => builder.push(event),
                dbide_pg_spike::cursor::CursorEvent::BatchEnd {
                    eof,
                    fetch_sequence,
                } => builder.batch_end(eof, fetch_sequence),
            };
            if let Err(reason) = &result {
                // These diagnostics originate only in our storage/control
                // callbacks. Never substitute arbitrary source diagnostics
                // or format SQL, credentials, column names, or row values.
                sink_error = Some(bounded_sink_error(reason));
            }
            result
        },
    ));
    let summary = result.map_err(|error| cursor_error(error, sink_error))?;
    Ok(QuerySummary {
        rows: summary.rows,
        payload_bytes: summary.payload_bytes,
        execution_target: target,
        sql_sha256,
        execution_id: dbide_driver_api::next_session_id(),
    })
}

fn bounded_sink_error(reason: &str) -> String {
    const MAX_LOCAL_ERROR_BYTES: usize = 1024;
    let mut end = reason.len().min(MAX_LOCAL_ERROR_BYTES);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_owned()
}

fn cursor_error(
    error: dbide_driver_api::stream::StreamError,
    sink_error: Option<String>,
) -> ExportError {
    use dbide_driver_api::stream::StreamErrorKind;
    ExportError {
        kind: if error.kind == StreamErrorKind::Cancelled {
            ExportErrorKind::Cancelled
        } else {
            ExportErrorKind::Execution
        },
        message: if error.kind == StreamErrorKind::Sink {
            sink_error.unwrap_or(error.message)
        } else {
            error.message
        },
    }
}

#[cfg(test)]
mod tests;
