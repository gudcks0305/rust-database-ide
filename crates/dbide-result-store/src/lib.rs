//! One successful independent query, captured once in a private temporary file.
//! Browsing reads this completed capture; it never executes SQL or retains a
//! database connection. Call both query execution and page reads off the UI thread.
//! Progressive partial captures are provided separately by [`live`].

pub mod live;
pub use live::{LiveMode, LiveQuery, LiveSnapshot, LiveState, start_live_query};

use dbide_driver_api::stream::StreamEvent;
use dbide_export::{ExportError, ExportErrorKind, ExportProgress, QueryRequest, QuerySummary};
use serde::Serialize;
use std::{
    io::{self, Read, Seek, SeekFrom, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;

const MAX_ROWS: u64 = 1_000_000;
const MAX_PAGES: usize = 65_536;
const MAX_PAGE_ROWS: usize = 200;
const MAX_PAGE_BYTES: usize = 512 * 1024;
const MAX_SPOOL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct PageLimits {
    pub max_rows_per_page: usize,
    /// Encoded JSON row records, including their newline delimiters.
    pub max_page_bytes: usize,
    /// Actual file bytes, including the column header and all delimiters.
    pub max_spool_bytes: u64,
}

impl Default for PageLimits {
    fn default() -> Self {
        Self {
            max_rows_per_page: MAX_PAGE_ROWS,
            max_page_bytes: MAX_PAGE_BYTES,
            max_spool_bytes: MAX_SPOOL_BYTES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPage {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub row_start: u64,
}

#[derive(Clone, Copy, Debug)]
struct PageIndex {
    row_start: u64,
    count: usize,
    offset: u64,
    len: usize,
}

/// The last owner dropping this store removes its private temporary file.
/// SQL text, credentials, and source connections are not retained here.
pub struct PagedResult {
    pub summary: QuerySummary,
    columns: Vec<String>,
    pages: Vec<PageIndex>,
    file: Mutex<NamedTempFile>,
    encoded_bytes: u64,
    limits: PageLimits,
}

impl PagedResult {
    /// Empty results have one empty page so their columns remain displayable.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    pub fn encoded_bytes(&self) -> u64 {
        self.encoded_bytes
    }

    /// Reads at most one bounded page. Returns a new value only after complete
    /// validation, so callers can preserve their displayed page on failure.
    pub fn read_page(&self, index: usize) -> Result<StoredPage, String> {
        let page = self.pages.get(index).ok_or("page index out of bounds")?;
        if page.len > self.limits.max_page_bytes
            || page.count > self.limits.max_rows_per_page
            || page
                .offset
                .checked_add(page.len as u64)
                .is_none_or(|end| end > self.encoded_bytes)
            || page
                .row_start
                .checked_add(page.count as u64)
                .is_none_or(|end| end > self.summary.rows)
        {
            return Err("invalid stored page index".into());
        }
        let mut bytes = vec![0; page.len];
        {
            let mut file = self.file.lock().map_err(|_| "result store lock poisoned")?;
            file.as_file_mut()
                .seek(SeekFrom::Start(page.offset))
                .and_then(|_| file.as_file_mut().read_exact(&mut bytes))
                .map_err(|error| format!("cannot read stored page: {error}"))?;
        }
        // The encoded byte cap bounds allocation even for corrupt JSON. Parse
        // records individually and reject trailing/missing records and widths.
        let mut rows = Vec::with_capacity(page.count);
        for record in bytes.split_inclusive(|byte| *byte == b'\n') {
            if rows.len() >= page.count || record.last() != Some(&b'\n') {
                return Err("invalid stored page record count or delimiter".into());
            }
            let row: Vec<Option<String>> = serde_json::from_slice(&record[..record.len() - 1])
                .map_err(|error| format!("invalid stored page JSON: {error}"))?;
            if row.len() != self.columns.len() {
                return Err("invalid stored page row width".into());
            }
            rows.push(row);
        }
        if rows.len() != page.count {
            return Err("invalid stored page record count".into());
        }
        Ok(StoredPage {
            columns: self.columns.clone(),
            rows,
            row_start: page.row_start,
        })
    }
}

/// Executes once and publishes the store only after the source succeeds and
/// closes. Any source, encoding, limit, cancellation, or disk error drops all
/// provisional data. Source payload and encoded disk budgets are independent.
pub fn query_pages(
    request: QueryRequest,
    limits: PageLimits,
    cancel: Arc<AtomicBool>,
    progress: Arc<ExportProgress>,
) -> Result<PagedResult, ExportError> {
    let deadline = CaptureDeadline {
        started: Instant::now(),
        timeout: request.options.timeout,
    };
    validate(limits, &request)?;
    deadline.check(&cancel)?;
    let builder = StoreBuilder::new(limits).map_err(execution)?;
    capture(request, cancel, progress, builder, deadline)
}

fn capture(
    mut request: QueryRequest,
    cancel: Arc<AtomicBool>,
    progress: Arc<ExportProgress>,
    mut builder: StoreBuilder,
    deadline: CaptureDeadline,
) -> Result<PagedResult, ExportError> {
    deadline.check(&cancel)?;
    // Source preflight/execution receives only the budget left after private
    // spool creation. Its separately bounded connection cleanup may overrun
    // this deadline, but can never make an expired capture publishable.
    request.options.timeout = deadline.timeout.saturating_sub(deadline.started.elapsed());
    let result = dbide_export::stream_query(request, cancel.clone(), progress, |event| {
        deadline.check(&cancel).map_err(|error| error.message)?;
        builder.push(event)?;
        deadline.check(&cancel).map_err(|error| error.message)
    });
    deadline.check(&cancel)?;
    let summary = result?;
    if summary.rows != builder.rows {
        return Err(execution("source and stored row counts differ"));
    }
    let store = builder.finish(summary, &cancel, deadline)?;
    deadline.check(&cancel)?;
    Ok(store)
}

fn validate(limits: PageLimits, request: &QueryRequest) -> Result<(), ExportError> {
    if !(1..=MAX_PAGE_ROWS).contains(&limits.max_rows_per_page)
        || !(1..=MAX_PAGE_BYTES).contains(&limits.max_page_bytes)
        || !(1..=MAX_SPOOL_BYTES).contains(&limits.max_spool_bytes)
    {
        return Err(ExportError {
            kind: ExportErrorKind::Configuration,
            message: "page limits must be finite: 1..=200 rows/page, 1..=524288 bytes/page, 1..=67108864 spool bytes".into(),
        });
    }
    if request.options.max_rows == 0
        || request.options.max_rows > MAX_ROWS
        || request.options.max_bytes == 0
        || request.options.max_bytes > MAX_SPOOL_BYTES
    {
        return Err(ExportError {
            kind: ExportErrorKind::Configuration,
            message: "paged query source requires 1..=1000000 rows and 1..=67108864 payload bytes"
                .into(),
        });
    }
    Ok(())
}

fn execution(message: impl ToString) -> ExportError {
    ExportError {
        kind: ExportErrorKind::Execution,
        message: message.to_string(),
    }
}

#[derive(Clone, Copy)]
struct CaptureDeadline {
    started: Instant,
    timeout: Duration,
}

impl CaptureDeadline {
    fn check(self, cancel: &AtomicBool) -> Result<(), ExportError> {
        if cancel.load(Ordering::Acquire) {
            Err(ExportError {
                kind: ExportErrorKind::Cancelled,
                message: "query cancelled".into(),
            })
        } else if self.started.elapsed() >= self.timeout {
            Err(execution("query capture deadline exceeded"))
        } else {
            Ok(())
        }
    }
}

struct StoreBuilder {
    file: NamedTempFile,
    columns: Option<Vec<String>>,
    pages: Vec<PageIndex>,
    current: Option<PageIndex>,
    rows: u64,
    encoded_bytes: u64,
    limits: PageLimits,
}

impl StoreBuilder {
    fn new(limits: PageLimits) -> Result<Self, String> {
        Ok(Self {
            file: tempfile::Builder::new()
                .prefix("dbide-result-")
                .tempfile()
                .map_err(|e| e.to_string())?,
            columns: None,
            pages: Vec::new(),
            current: None,
            rows: 0,
            encoded_bytes: 0,
            limits,
        })
    }

    fn push(&mut self, event: StreamEvent<'_>) -> Result<(), String> {
        match event {
            StreamEvent::Columns(columns) => {
                if self.columns.is_some() {
                    return Err("source emitted columns more than once".into());
                }
                let encoded =
                    encode_record(columns, self.limits.max_page_bytes.min(MAX_HEADER_BYTES))?;
                self.append(&encoded)?;
                self.columns = Some(columns.to_vec());
            }
            StreamEvent::Row(row) => {
                let columns = self
                    .columns
                    .as_ref()
                    .ok_or("source emitted row before columns")?;
                if row.len() != columns.len() {
                    return Err("source row width differs from columns".into());
                }
                if self.rows >= MAX_ROWS {
                    return Err("result store row limit exceeded".into());
                }
                let encoded = encode_record(row, self.limits.max_page_bytes)?;
                if self.current.is_some_and(|page| {
                    page.count == self.limits.max_rows_per_page
                        || page.len + encoded.len() > self.limits.max_page_bytes
                }) {
                    self.finish_page()?;
                }
                if self.current.is_none() {
                    if self.pages.len() >= MAX_PAGES {
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
                let page = self.current.as_mut().expect("current page created");
                page.count += 1;
                page.len += encoded.len();
                self.rows += 1;
            }
        }
        Ok(())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        let next = self
            .encoded_bytes
            .checked_add(bytes.len() as u64)
            .filter(|next| *next <= self.limits.max_spool_bytes)
            .ok_or("encoded result spool byte limit exceeded")?;
        self.file
            .write_all(bytes)
            .map_err(|error| format!("cannot write result spool: {error}"))?;
        self.encoded_bytes = next;
        Ok(())
    }

    fn finish_page(&mut self) -> Result<(), String> {
        if let Some(page) = self.current.take() {
            if self.pages.len() >= MAX_PAGES {
                return Err("result store page limit exceeded (65536)".into());
            }
            self.pages.push(page);
        }
        Ok(())
    }

    fn finish(
        mut self,
        summary: QuerySummary,
        cancel: &AtomicBool,
        deadline: CaptureDeadline,
    ) -> Result<PagedResult, ExportError> {
        let columns = self
            .columns
            .take()
            .ok_or_else(|| execution("source omitted columns"))?;
        self.finish_page().map_err(execution)?;
        if self.pages.is_empty() {
            self.pages.push(PageIndex {
                row_start: 0,
                count: 0,
                offset: self.encoded_bytes,
                len: 0,
            });
        }
        self.file.as_file().sync_all().map_err(execution)?;
        // A blocked filesystem operation cannot be preempted. Its eventual
        // completion still must not publish a cancelled or expired capture.
        deadline.check(cancel)?;
        Ok(PagedResult {
            summary,
            columns,
            pages: self.pages,
            file: Mutex::new(self.file),
            encoded_bytes: self.encoded_bytes,
            limits: self.limits,
        })
    }
}

/// serde writes strings incrementally into this capped buffer. A single row
/// cannot allocate a JSON representation larger than the page byte limit.
struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("encoded result record exceeds byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode_record<T: Serialize + ?Sized>(record: &T, cap: usize) -> Result<Vec<u8>, String> {
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit: cap,
    };
    serde_json::to_writer(&mut writer, record).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())?;
    Ok(writer.bytes)
}

#[cfg(test)]
mod tests;
