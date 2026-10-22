//! Explicit, independent read-only CSV source. Never resumes a retained result,
//! shares a SQL session, buffers a result set, or retries SQL.
use super::open_connection;
use rusqlite::{Batch, fallible_iterator::FallibleIterator, types::ValueRef};
use std::{
    fmt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct ExportLimits {
    pub max_rows: u64,
    /// Bounds application-owned conversion of a single row, including headers.
    pub max_row_bytes: usize,
    /// Cumulative source payload, independently of the sink's encoded byte cap.
    pub max_payload_bytes: u64,
    pub timeout: Duration,
}
impl Default for ExportLimits {
    fn default() -> Self {
        Self {
            max_rows: 1_000_000,
            max_row_bytes: 1024 * 1024,
            max_payload_bytes: 64 * 1024 * 1024,
            timeout: Duration::from_secs(30),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportErrorKind {
    Configuration,
    Connection,
    Query,
    Limit,
    Cancelled,
    Timeout,
    Sink,
}
#[derive(Debug)]
pub struct ExportError {
    pub kind: ExportErrorKind,
    pub message: String,
}
impl ExportError {
    fn new(kind: ExportErrorKind, message: impl ToString) -> Self {
        Self {
            kind,
            message: message.to_string(),
        }
    }
}
impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for ExportError {}

/// CSV represents these scalars textually; it does not retain SQLite types.
/// Invalid UTF-8, BLOBs and non-finite REALs are rejected without substitution.
#[derive(Clone, Copy, Debug)]
pub enum ExportValue<'a> {
    Null,
    Integer(i64),
    Real(f64),
    Text(&'a str),
}
pub enum ExportEvent<'a> {
    Columns(&'a [String]),
    Row(&'a [ExportValue<'a>]),
}
#[derive(Debug, PartialEq, Eq)]
pub struct ExportSummary {
    pub rows: u64,
    pub payload_bytes: u64,
}

fn check_control(cancelled: &AtomicBool, deadline: Instant) -> Result<(), ExportError> {
    if cancelled.load(Ordering::Acquire) {
        Err(ExportError::new(
            ExportErrorKind::Cancelled,
            "export cancelled",
        ))
    } else if Instant::now() >= deadline {
        Err(ExportError::new(
            ExportErrorKind::Timeout,
            "export deadline exceeded",
        ))
    } else {
        Ok(())
    }
}

/// Opens an existing absolute file using native READ_ONLY/query_only/authorizer
/// before preparing any supplied SQL. Requires exactly one column-producing
/// native readonly statement. The caller owns its provisional output and must
/// discard it on every error. Only the current borrowed row reaches the sink.
///
/// This bounds application row retention, not SQLite's own query working set or
/// the duration of a blocked operating-system output write.
pub fn export_read_only<F>(
    path: &Path,
    sql: &str,
    limits: ExportLimits,
    cancelled: Arc<AtomicBool>,
    mut sink: F,
) -> Result<ExportSummary, ExportError>
where
    F: for<'a> FnMut(ExportEvent<'a>) -> Result<(), String>,
{
    if !path.is_absolute()
        || !path.is_file()
        || path.to_str().is_none()
        || sql.len() > 64 * 1024
        || limits.max_rows == 0
        || limits.max_rows == u64::MAX
        || limits.max_row_bytes == 0
        || limits.max_row_bytes == usize::MAX
        || limits.max_payload_bytes == 0
        || limits.max_payload_bytes == u64::MAX
        || limits.timeout.is_zero()
    {
        return Err(ExportError::new(
            ExportErrorKind::Configuration,
            "export requires an existing absolute SQLite file, SQL <=64 KiB and finite positive limits",
        ));
    }
    let deadline = Instant::now().checked_add(limits.timeout).ok_or_else(|| {
        ExportError::new(ExportErrorKind::Configuration, "invalid export deadline")
    })?;
    check_control(&cancelled, deadline)?;
    let connection = open_connection(path.to_str().expect("UTF-8 checked"), true, false)
        .map_err(|error| ExportError::new(ExportErrorKind::Connection, error))?;
    let progress_cancelled = cancelled.clone();
    connection
        .progress_handler(
            1000,
            Some(move || progress_cancelled.load(Ordering::Acquire) || Instant::now() >= deadline),
        )
        .map_err(|error| ExportError::new(ExportErrorKind::Query, error))?;
    let native = |error: rusqlite::Error| {
        check_control(&cancelled, deadline)
            .err()
            .unwrap_or_else(|| ExportError::new(ExportErrorKind::Query, error))
    };
    check_control(&cancelled, deadline)?;
    let mut batch = Batch::new(&connection, sql);
    let mut statement = batch.next().map_err(native)?.ok_or_else(|| {
        ExportError::new(
            ExportErrorKind::Configuration,
            "export requires one SQL statement",
        )
    })?;
    if !statement.readonly() || statement.column_count() == 0 {
        return Err(ExportError::new(
            ExportErrorKind::Configuration,
            "export requires one column-producing read-only statement",
        ));
    }
    // Validate the complete native tail before the first row is stepped. The
    // authorizer is already installed, including for prepare-time PRAGMAs.
    if batch.next().map_err(native)?.is_some() {
        return Err(ExportError::new(
            ExportErrorKind::Configuration,
            "export accepts exactly one statement; no SQL was executed",
        ));
    }
    let names = statement.column_names();
    let header_bytes = names
        .iter()
        .try_fold(0usize, |total, name| total.checked_add(name.len().max(1)))
        .ok_or_else(|| ExportError::new(ExportErrorKind::Limit, "export header size overflow"))?;
    if header_bytes > limits.max_row_bytes || header_bytes as u64 > limits.max_payload_bytes {
        return Err(ExportError::new(
            ExportErrorKind::Limit,
            "export header byte budget exceeded",
        ));
    }
    let columns: Vec<String> = names.into_iter().map(str::to_owned).collect();
    check_control(&cancelled, deadline)?;
    sink(ExportEvent::Columns(&columns)).map_err(|e| ExportError::new(ExportErrorKind::Sink, e))?;
    let mut summary = ExportSummary {
        rows: 0,
        payload_bytes: header_bytes as u64,
    };
    let mut rows = statement.query([]).map_err(native)?;
    loop {
        check_control(&cancelled, deadline)?;
        let Some(row) = rows.next().map_err(native)? else {
            break;
        };
        check_control(&cancelled, deadline)?;
        if summary.rows >= limits.max_rows {
            return Err(ExportError::new(
                ExportErrorKind::Limit,
                "export row budget exceeded",
            ));
        }
        let mut row_bytes = 0usize;
        for column in 0..columns.len() {
            let bytes = match row.get_ref(column).map_err(native)? {
                ValueRef::Null => 1,
                ValueRef::Integer(_) | ValueRef::Real(_) => 8,
                ValueRef::Text(bytes) => bytes.len().max(1),
                ValueRef::Blob(_) => {
                    return Err(ExportError::new(
                        ExportErrorKind::Query,
                        "BLOB CSV export is unsupported; explicitly select a textual encoding",
                    ));
                }
            };
            row_bytes = row_bytes.checked_add(bytes).ok_or_else(|| {
                ExportError::new(ExportErrorKind::Limit, "export row size overflow")
            })?;
            if row_bytes > limits.max_row_bytes {
                return Err(ExportError::new(
                    ExportErrorKind::Limit,
                    "export single-row byte budget exceeded",
                ));
            }
        }
        let payload = summary
            .payload_bytes
            .checked_add(row_bytes as u64)
            .filter(|bytes| *bytes <= limits.max_payload_bytes)
            .ok_or_else(|| {
                ExportError::new(
                    ExportErrorKind::Limit,
                    "export payload byte budget exceeded",
                )
            })?;
        let values: Vec<ExportValue<'_>> = (0..columns.len())
            .map(|column| {
                Ok(match row.get_ref(column).map_err(native)? {
                    ValueRef::Null => ExportValue::Null,
                    ValueRef::Integer(value) => ExportValue::Integer(value),
                    ValueRef::Real(value) if value.is_finite() => ExportValue::Real(value),
                    ValueRef::Real(_) => {
                        return Err(ExportError::new(
                            ExportErrorKind::Query,
                            "non-finite REAL CSV export is unsupported",
                        ));
                    }
                    ValueRef::Text(bytes) => {
                        ExportValue::Text(std::str::from_utf8(bytes).map_err(|_| {
                            ExportError::new(
                                ExportErrorKind::Query,
                                "invalid UTF-8 in SQLite text; export refused",
                            )
                        })?)
                    }
                    ValueRef::Blob(_) => unreachable!("BLOB rejected before conversion"),
                })
            })
            .collect::<Result<_, ExportError>>()?;
        check_control(&cancelled, deadline)?;
        sink(ExportEvent::Row(&values)).map_err(|e| ExportError::new(ExportErrorKind::Sink, e))?;
        summary.rows += 1;
        summary.payload_bytes = payload;
    }
    check_control(&cancelled, deadline)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owned-export.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE original(v); INSERT INTO original VALUES(7)")
            .unwrap();
        drop(connection);
        (directory, path)
    }
    fn cancellation() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    #[test]
    fn exports_beyond_retained_byte_and_event_caps_one_row_at_a_time() {
        let (_directory, path) = fixture();
        let before = std::fs::read(&path).unwrap();
        let mut count = 0;
        let mut last = 0;
        let summary = export_read_only(&path,
            "WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<6000) SELECT v, printf('%0120d',v) AS payload FROM n",
            ExportLimits::default(), cancellation(), |event| {
                if let ExportEvent::Row(values) = event {
                    count += 1;
                    if let ExportValue::Integer(value) = values[0] { last = value; }
                    assert_eq!(values.len(), 2);
                }
                Ok(())
            }).unwrap();
        assert_eq!(count, 6000);
        assert_eq!(last, 6000);
        assert_eq!(summary.rows, 6000);
        assert!(summary.payload_bytes > 512 * 1024);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn authorizer_and_single_statement_checks_run_before_any_sink_callback() {
        let (_directory, path) = fixture();
        let before = std::fs::read(&path).unwrap();
        for sql in [
            "INSERT INTO original VALUES(9)",
            "CREATE TEMP TABLE bypass(v)",
            "PRAGMA query_only=OFF",
            "PRAGMA user_version=42",
            "ATTACH ':memory:' AS bypass",
            "SELECT 1; SELECT 2",
            "SELECT 1; PRAGMA user_version=42",
            "BEGIN",
            "-- empty",
        ] {
            let mut calls = 0;
            let result =
                export_read_only(&path, sql, ExportLimits::default(), cancellation(), |_| {
                    calls += 1;
                    Ok(())
                });
            assert!(result.is_err(), "{sql}");
            assert_eq!(calls, 0, "{sql}");
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            export_read_only(
                &path,
                "SELECT 1; /* comment-only tail */",
                ExportLimits::default(),
                cancellation(),
                |_| Ok(())
            )
            .unwrap()
            .rows,
            1
        );
    }

    #[test]
    fn midstream_native_sink_and_cancel_failures_stop_the_source() {
        let (_directory, path) = fixture();
        let mut rows = 0;
        let error = export_read_only(
            &path,
            "SELECT 'first' UNION ALL SELECT abs(-9223372036854775808)",
            ExportLimits::default(),
            cancellation(),
            |event| {
                if matches!(event, ExportEvent::Row(_)) {
                    rows += 1;
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ExportErrorKind::Query);
        assert_eq!(rows, 1);
        let mut rows = 0;
        let error = export_read_only(
            &path,
            "SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3",
            ExportLimits::default(),
            cancellation(),
            |event| {
                if matches!(event, ExportEvent::Row(_)) {
                    rows += 1;
                    if rows == 2 {
                        return Err("owned sink failure".into());
                    }
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ExportErrorKind::Sink);
        assert_eq!(rows, 2);
        let cancelled = cancellation();
        let signal = cancelled.clone();
        let mut rows = 0;
        let error = export_read_only(
            &path,
            "SELECT 1 UNION ALL SELECT 2",
            ExportLimits::default(),
            cancelled,
            |event| {
                if matches!(event, ExportEvent::Row(_)) {
                    rows += 1;
                    signal.store(true, Ordering::Release);
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ExportErrorKind::Cancelled);
        assert_eq!(rows, 1);
    }

    #[test]
    fn rejects_unrepresentable_cells_and_finite_budgets() {
        let (_directory, path) = fixture();
        for (sql, message) in [
            ("SELECT x'ff'", "BLOB"),
            ("SELECT CAST(x'80' AS TEXT)", "UTF-8"),
            ("SELECT 1e999", "non-finite"),
        ] {
            let error =
                export_read_only(&path, sql, ExportLimits::default(), cancellation(), |_| {
                    Ok(())
                })
                .unwrap_err();
            assert!(error.message.contains(message), "{error}");
        }
        for (sql, limits) in [
            (
                "SELECT 1 UNION ALL SELECT 2",
                ExportLimits {
                    max_rows: 1,
                    ..ExportLimits::default()
                },
            ),
            (
                "SELECT printf('%0100d',0) AS v",
                ExportLimits {
                    max_row_bytes: 32,
                    ..ExportLimits::default()
                },
            ),
            (
                "SELECT '1234567890' AS v UNION ALL SELECT '1234567890'",
                ExportLimits {
                    max_payload_bytes: 16,
                    ..ExportLimits::default()
                },
            ),
        ] {
            assert_eq!(
                export_read_only(&path, sql, limits, cancellation(), |_| Ok(()))
                    .unwrap_err()
                    .kind,
                ExportErrorKind::Limit
            );
        }
        assert_eq!(
            export_read_only(
                Path::new(":memory:"),
                "SELECT 1",
                ExportLimits::default(),
                cancellation(),
                |_| Ok(())
            )
            .unwrap_err()
            .kind,
            ExportErrorKind::Configuration
        );
    }

    #[test]
    fn native_progress_deadline_stops_an_aggregate_before_its_first_row() {
        let (_directory, path) = fixture();
        let start = Instant::now();
        let error = export_read_only(&path,
            "WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<100000000) SELECT sum(v) FROM n",
            ExportLimits { timeout: Duration::from_millis(10), ..ExportLimits::default() },
            cancellation(), |_| Ok(())).unwrap_err();
        assert_eq!(error.kind, ExportErrorKind::Timeout);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
