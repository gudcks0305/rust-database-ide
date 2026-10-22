//! Dedicated, one-shot read-only native prepared execution for textual export.
//! No pool, retries, SQL replay, or UI result buffer. CSV is not a typed interchange.
use crate::{build_opts, connect_error};
use dbide_driver_api::stream::{
    StreamError, StreamErrorKind, StreamEvent, StreamLimits, StreamSummary,
};
use dbide_profiles::ConnectionRequest;
use mysql_async::{Conn, OptsBuilder, TxOpts, Value, consts::ColumnType, prelude::Queryable};
use sqlparser::{
    ast::{Expr, Query, Select, SetExpr, Statement, Visit, Visitor},
    dialect::MySqlDialect,
    parser::Parser,
    tokenizer::{Token, Tokenizer, Whitespace},
};
use std::{
    ops::ControlFlow,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const RETIRE_TIMEOUT: Duration = Duration::from_secs(3);
const SQL_BYTES: usize = 64 * 1024;

/// Execute one SELECT/CTE on an independent READ ONLY transaction.
/// The callback must return promptly; synchronous sink work cannot be preempted.
/// Native packets are capped separately; row limits apply after the driver decodes
/// one packet, and do not claim a process RSS bound or typed CSV fidelity.
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
            fail(
                StreamErrorKind::Configuration,
                "MySQL export timeout is too large",
            )
        })?;
    validate(sql, limits)?;
    check_cancel(&cancelled)?;
    let opts = build_opts(&request).map_err(|error| fail(StreamErrorKind::Configuration, error))?;
    // Codec rejects oversized wire packets before unrestricted row conversion.
    // mysql_async clamps its packet option to its supported protocol range.
    let opts: mysql_async::Opts = OptsBuilder::from_opts(opts)
        .max_allowed_packet(Some(limits.max_row_bytes.clamp(1024, 16 * 1024 * 1024)))
        .into();
    let mut conn = controlled(deadline, &cancelled, async {
        Conn::new(opts.clone())
            .await
            .map_err(|error| fail(StreamErrorKind::Connection, connect_error(error)))
    })
    .await?;
    // A separate control socket exists before executing user SQL. Its KILL commands
    // target only this exclusively owned connection, which is never reused.
    let mut control = match controlled(deadline, &cancelled, async {
        Conn::new(opts)
            .await
            .map_err(|error| fail(StreamErrorKind::Connection, connect_error(error)))
    })
    .await
    {
        Ok(control) => control,
        Err(error) => {
            disconnect(conn, tokio::time::Instant::now() + RETIRE_TIMEOUT).await?;
            return Err(error);
        }
    };
    let id = conn.id();
    let outcome = controlled(deadline, &cancelled, async {
        let mut tx_opts = TxOpts::default();
        tx_opts.with_readonly(true);
        let mut transaction = conn.start_transaction(tx_opts).await.map_err(execution)?;
        let statement = transaction.prep(sql).await.map_err(execution)?;
        if statement.num_params() != 0 || statement.columns().is_empty() {
            return Err(fail(
                StreamErrorKind::Configuration,
                "Export requires one parameter-free SELECT with columns",
            ));
        }
        let mut result = transaction
            .exec_iter(&statement, ())
            .await
            .map_err(execution)?;
        // An empty native result has already consumed its terminal packet and
        // mysql_async then exposes no active columns; prepare still has them.
        let metadata = result.columns().unwrap_or_else(|| statement.columns());
        let mut budget = Budget {
            limits,
            summary: StreamSummary {
                rows: 0,
                payload_bytes: 0,
            },
        };
        let mut header_size = 0usize;
        for column in metadata.iter() {
            header_size = add_size(
                header_size,
                column.name_ref().len().max(1),
                limits.max_row_bytes,
            )?;
        }
        budget.payload(header_size)?;
        let names = metadata
            .iter()
            .map(|column| {
                std::str::from_utf8(column.name_ref())
                    .map(str::to_owned)
                    .map_err(|_| fail(StreamErrorKind::Execution, "MySQL column name is not UTF-8"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        sink(StreamEvent::Columns(&names)).map_err(|error| fail(StreamErrorKind::Sink, error))?;
        while let Some(row) = result.next().await.map_err(execution)? {
            check_cancel(&cancelled)?;
            check_deadline(deadline)?;
            if budget.summary.rows >= limits.max_rows {
                return Err(fail(
                    StreamErrorKind::Limit,
                    "MySQL export row limit exceeded",
                ));
            }
            let mut minimum_size = 0usize;
            for index in 0..row.len() {
                let value = row
                    .as_ref(index)
                    .ok_or_else(|| fail(StreamErrorKind::Execution, "Missing MySQL value"))?;
                // Reject large native byte payloads before UTF-8 validation/copy.
                minimum_size = add_size(
                    minimum_size,
                    match value {
                        Value::Bytes(bytes) => bytes.len().max(1),
                        _ => 1,
                    },
                    limits.max_row_bytes,
                )?;
            }
            if minimum_size as u64
                > limits
                    .max_payload_bytes
                    .saturating_sub(budget.summary.payload_bytes)
            {
                return Err(fail(
                    StreamErrorKind::Limit,
                    "MySQL export payload limit exceeded",
                ));
            }
            let values = row
                .unwrap()
                .into_iter()
                .zip(metadata.iter())
                .map(|(value, column)| text(value, column.column_type(), column.character_set()))
                .collect::<Result<Vec<_>, _>>()?;
            let size = values.iter().try_fold(0usize, |size, value| {
                add_size(
                    size,
                    value.as_ref().map_or(1, |value| value.len().max(1)),
                    limits.max_row_bytes,
                )
            })?;
            budget.payload(size)?;
            sink(StreamEvent::Row(&values)).map_err(|error| fail(StreamErrorKind::Sink, error))?;
            budget.summary.rows += 1;
        }
        // Consume terminal protocol status before declaring completion.
        result.drop_result().await.map_err(execution)?;
        transaction.close(statement).await.map_err(execution)?;
        transaction.rollback().await.map_err(execution)?;
        check_cancel(&cancelled)?;
        check_deadline(deadline)?;
        Ok(budget.summary)
    })
    .await;
    let cleanup_deadline = tokio::time::Instant::now() + RETIRE_TIMEOUT;
    let cleanup_deadline = if outcome.is_ok() {
        cleanup_deadline.min(deadline)
    } else {
        cleanup_deadline
    };
    let retire = if outcome.is_err() {
        // Dropping Conn alone can spawn an unbounded drain in mysql_async. Kill
        // the backend first, then explicitly disconnect (which bypasses drain).
        tokio::time::timeout_at(cleanup_deadline, async {
            let query_kill = control.query_drop(format!("KILL QUERY {id}")).await;
            let connection_kill = control.query_drop(format!("KILL CONNECTION {id}")).await;
            if let Err(error) = connection_kill
                && !matches!(&error, mysql_async::Error::Server(error) if error.code == 1094)
            {
                return Err(fail(
                    StreamErrorKind::Connection,
                    "MySQL export backend retirement failed",
                ));
            }
            // KILL CONNECTION also covers an already-completed KILL QUERY.
            let _ = query_kill;
            loop {
                let alive: Option<u64> = control
                    .exec_first(
                        "SELECT COUNT(*) FROM information_schema.processlist WHERE ID = ?",
                        (id,),
                    )
                    .await
                    .map_err(|_| {
                        fail(
                            StreamErrorKind::Connection,
                            "MySQL export backend retirement verification failed",
                        )
                    })?;
                if alive == Some(0) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(())
        })
        .await
        .unwrap_or_else(|_| {
            Err(fail(
                StreamErrorKind::Connection,
                "MySQL export backend retirement timed out",
            ))
        })
    } else {
        Ok(())
    };
    // Conn::disconnect marks disconnected before its first await, so timeout
    // drops its socket without the Drop cleanup_for_pool/drain path.
    let data_close = disconnect(conn, cleanup_deadline).await;
    let control_close = disconnect(control, cleanup_deadline).await;
    retire?;
    // A server-killed data socket may reject COM_QUIT; it has already retired.
    if outcome.is_ok() {
        check_cancel(&cancelled)?;
        check_deadline(deadline)?;
        data_close?;
    }
    control_close?;
    outcome
}

async fn controlled<T>(
    deadline: tokio::time::Instant,
    cancelled: &AtomicBool,
    future: impl std::future::Future<Output = Result<T, StreamError>>,
) -> Result<T, StreamError> {
    tokio::select! {
        biased;
        _ = wait_cancel(cancelled) => Err(fail(StreamErrorKind::Cancelled, "MySQL export cancelled")),
        _ = tokio::time::sleep_until(deadline) => Err(fail(StreamErrorKind::Timeout, "MySQL export deadline exceeded")),
        result = future => result,
    }
}
async fn wait_cancel(cancelled: &AtomicBool) {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn disconnect(conn: Conn, deadline: tokio::time::Instant) -> Result<(), StreamError> {
    // Pinned tokio polls the inner future before its timeout, even when expired.
    // Conn::disconnect therefore marks disconnected before a timeout can drop it.
    tokio::time::timeout_at(deadline, conn.disconnect())
        .await
        .map_err(|_| {
            fail(
                StreamErrorKind::Connection,
                "MySQL export disconnect timed out",
            )
        })?
        .map_err(|_| {
            fail(
                StreamErrorKind::Connection,
                "MySQL export disconnect failed",
            )
        })
}
fn check_cancel(cancelled: &AtomicBool) -> Result<(), StreamError> {
    if cancelled.load(Ordering::Acquire) {
        Err(fail(StreamErrorKind::Cancelled, "MySQL export cancelled"))
    } else {
        Ok(())
    }
}
fn check_deadline(deadline: tokio::time::Instant) -> Result<(), StreamError> {
    if tokio::time::Instant::now() >= deadline {
        Err(fail(
            StreamErrorKind::Timeout,
            "MySQL export deadline exceeded",
        ))
    } else {
        Ok(())
    }
}
fn fail(kind: StreamErrorKind, message: impl ToString) -> StreamError {
    StreamError::new(kind, message)
}
fn execution(_error: mysql_async::Error) -> StreamError {
    // Native server/transport text can contain SQL, credentials, and remote paths.
    fail(StreamErrorKind::Execution, "MySQL export execution failed")
}
fn add_size(size: usize, more: usize, limit: usize) -> Result<usize, StreamError> {
    size.checked_add(more)
        .filter(|size| *size <= limit)
        .ok_or_else(|| {
            fail(
                StreamErrorKind::Limit,
                "MySQL export row/header byte limit exceeded",
            )
        })
}
struct Budget {
    limits: StreamLimits,
    summary: StreamSummary,
}
impl Budget {
    fn payload(&mut self, bytes: usize) -> Result<(), StreamError> {
        self.summary.payload_bytes = self
            .summary
            .payload_bytes
            .checked_add(bytes as u64)
            .filter(|bytes| *bytes <= self.limits.max_payload_bytes)
            .ok_or_else(|| {
                fail(
                    StreamErrorKind::Limit,
                    "MySQL export payload limit exceeded",
                )
            })?;
        Ok(())
    }
}
fn text(value: Value, kind: ColumnType, charset: u16) -> Result<Option<String>, StreamError> {
    Ok(Some(match value {
        Value::NULL => return Ok(None),
        Value::Bytes(bytes) => {
            if (charset == 63
                && !matches!(
                    kind,
                    ColumnType::MYSQL_TYPE_DECIMAL
                        | ColumnType::MYSQL_TYPE_NEWDECIMAL
                        | ColumnType::MYSQL_TYPE_JSON
                ))
                || matches!(
                    kind,
                    ColumnType::MYSQL_TYPE_BIT
                        | ColumnType::MYSQL_TYPE_GEOMETRY
                        | ColumnType::MYSQL_TYPE_VECTOR
                )
            {
                return Err(fail(
                    StreamErrorKind::Execution,
                    "Binary MySQL values require an explicit text cast for export",
                ));
            }
            String::from_utf8(bytes).map_err(|_| {
                fail(
                    StreamErrorKind::Execution,
                    "MySQL value is not UTF-8; cast explicitly to UTF-8 text",
                )
            })?
        }
        Value::Int(value) => value.to_string(),
        Value::UInt(value) => value.to_string(),
        Value::Float(value) if value.is_finite() => value.to_string(),
        Value::Double(value) if value.is_finite() => value.to_string(),
        Value::Float(_) | Value::Double(_) => {
            return Err(fail(
                StreamErrorKind::Execution,
                "Non-finite MySQL floats are unsupported",
            ));
        }
        Value::Date(year, month, day, hour, minute, second, micros) => {
            if kind == ColumnType::MYSQL_TYPE_DATE {
                format!("{year:04}-{month:02}-{day:02}")
            } else if micros == 0 {
                format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
            } else {
                format!(
                    "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{micros:06}"
                )
            }
        }
        Value::Time(negative, days, hours, minutes, seconds, micros) => {
            let hours = u64::from(days) * 24 + u64::from(hours);
            let sign = if negative { "-" } else { "" };
            if micros == 0 {
                format!("{sign}{hours:02}:{minutes:02}:{seconds:02}")
            } else {
                format!("{sign}{hours:02}:{minutes:02}:{seconds:02}.{micros:06}")
            }
        }
    }))
}

fn validate(sql: &str, limits: StreamLimits) -> Result<(), StreamError> {
    let invalid = || {
        fail(
            StreamErrorKind::Configuration,
            "Export requires a bounded, parameter-free read-only SELECT/CTE",
        )
    };
    // Avoid interpretation differences when a server enables
    // NO_BACKSLASH_ESCAPES; this intentionally narrow route requires doubled
    // quote escaping rather than backslashes, including inside string literals.
    if sql.contains('\\')
        || sql.len() > SQL_BYTES
        || limits.max_rows == 0
        || limits.max_row_bytes == 0
        || limits.max_payload_bytes == 0
        || limits.timeout.is_zero()
    {
        return Err(invalid());
    }
    let tokens = Tokenizer::new(&MySqlDialect {}, sql)
        .tokenize()
        .map_err(|_| invalid())?;
    for token in &tokens {
        if matches!(token, Token::AtSign | Token::AtAt | Token::Assignment | Token::Placeholder(_))
            || matches!(token, Token::Word(word) if word.value.starts_with('@'))
            // MySQL executable comments/hints can change semantics outside AST.
            || matches!(token, Token::Whitespace(Whitespace::MultiLineComment(comment)) if comment.starts_with('!') || comment.starts_with('+'))
            || matches!(token, Token::Word(word) if word.quote_style.is_none() && ["OUTFILE", "DUMPFILE", "SQL_CALC_FOUND_ROWS", "PROCEDURE"].iter().any(|keyword| word.value.eq_ignore_ascii_case(keyword)))
        {
            return Err(invalid());
        }
    }
    let statements = Parser::parse_sql(&MySqlDialect {}, sql).map_err(|_| invalid())?;
    if statements.len() != 1 || !matches!(&statements[0], Statement::Query(_)) {
        return Err(invalid());
    }
    if statements.visit(&mut ReadOnly).is_break() {
        return Err(invalid());
    }
    Ok(())
}
struct ReadOnly;
impl Visitor for ReadOnly {
    type Break = ();
    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<()> {
        if matches!(statement, Statement::Query(_)) {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        }
    }
    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        if !read_body(&query.body)
            || !query.locks.is_empty()
            || query.settings.is_some()
            || query.for_clause.is_some()
            || query.format_clause.is_some()
            || !query.pipe_operators.is_empty()
        {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
    fn pre_visit_select(&mut self, select: &Select) -> ControlFlow<()> {
        if select.into.is_some() || !select.optimizer_hints.is_empty() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        if let Expr::Function(function) = expr {
            // Keep stored functions/UDFs and session-mutating functions outside
            // this narrow route; server READ ONLY is an additional boundary.
            let name = function.name.to_string().to_ascii_uppercase();
            if ![
                "COUNT",
                "SUM",
                "MIN",
                "MAX",
                "AVG",
                "COALESCE",
                "IF",
                "IFNULL",
                "NULLIF",
                "CONCAT",
                "CONCAT_WS",
                "REPEAT",
                "LENGTH",
                "CHAR_LENGTH",
                "CHARACTER_LENGTH",
                "LOWER",
                "UPPER",
                "TRIM",
                "LTRIM",
                "RTRIM",
                "SUBSTRING",
                "SUBSTR",
                "REPLACE",
                "ROUND",
                "ABS",
                "CEIL",
                "CEILING",
                "FLOOR",
                "MOD",
                "POW",
                "POWER",
                "SQRT",
                "DATE",
                "YEAR",
                "MONTH",
                "DAY",
                "DATE_FORMAT",
                "TIME_FORMAT",
                "NOW",
                "CURRENT_TIMESTAMP",
                "CONNECTION_ID",
                "SLEEP",
                "JSON_EXTRACT",
                "JSON_UNQUOTE",
                "JSON_ARRAY",
                "JSON_OBJECT",
                "HEX",
                "UNHEX",
                "CONVERT",
                "ROW_NUMBER",
                "RANK",
                "DENSE_RANK",
                "LEAD",
                "LAG",
            ]
            .contains(&name.as_str())
            {
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    }
}
fn read_body(body: &SetExpr) -> bool {
    match body {
        SetExpr::Select(_) | SetExpr::Query(_) => true,
        SetExpr::SetOperation { left, right, .. } => read_body(left) && read_body(right),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn select_gate_rejects_mutation_and_mysql_escape_hatches() {
        for sql in [
            "SELECT 1; SELECT 2",
            "UPDATE t SET x=1",
            "SET TRANSACTION READ WRITE",
            "SELECT 1 INTO OUTFILE '/tmp/dbide'",
            "SELECT 1 INTO DUMPFILE '/tmp/dbide'",
            "SELECT @a:=1",
            "SELECT @@transaction_read_only",
            "SELECT * FROM t FOR UPDATE",
            "SELECT * FROM t LOCK IN SHARE MODE",
            "WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x",
            "SELECT GET_LOCK('x',1)",
            "SELECT app.custom_write()",
            "SELECT 1 /*! INTO OUTFILE '/tmp/dbide' */",
            "SELECT /*+ SET_VAR(sort_buffer_size=100000) */ 1",
            "SELECT ?",
        ] {
            assert!(
                validate(sql, StreamLimits::default()).is_err(),
                "accepted {sql}"
            );
        }
        for sql in [
            "SELECT 1",
            "SELECT '한글', NULL, ''",
            "WITH x AS (SELECT 1 AS a) SELECT a FROM x",
            "SELECT COUNT(*) FROM t",
            "SELECT 1 UNION ALL SELECT 2",
            "SELECT '/* ! @ ? */' AS txt",
        ] {
            assert!(
                validate(sql, StreamLimits::default()).is_ok(),
                "rejected {sql}"
            );
        }
    }
    #[test]
    fn native_scalar_text_preserves_precision_null_and_temporal_values() {
        use ColumnType::*;
        assert_eq!(
            text(Value::UInt(u64::MAX), MYSQL_TYPE_LONGLONG, 63).unwrap(),
            Some(u64::MAX.to_string())
        );
        assert_eq!(
            text(
                Value::Bytes(b"12345678901234567890.123456789".to_vec()),
                MYSQL_TYPE_NEWDECIMAL,
                8
            )
            .unwrap(),
            Some("12345678901234567890.123456789".into())
        );
        assert_eq!(text(Value::NULL, MYSQL_TYPE_NULL, 63).unwrap(), None);
        assert_eq!(
            text(Value::Bytes(vec![]), MYSQL_TYPE_VAR_STRING, 45).unwrap(),
            Some(String::new())
        );
        assert!(text(Value::Bytes(vec![255]), MYSQL_TYPE_VAR_STRING, 45).is_err());
        assert!(text(Value::Bytes(b"abc".to_vec()), MYSQL_TYPE_BLOB, 63).is_err());
        assert!(text(Value::Double(f64::NAN), MYSQL_TYPE_DOUBLE, 63).is_err());
        assert_eq!(
            text(
                Value::Date(2026, 10, 22, 1, 2, 3, 456789),
                MYSQL_TYPE_DATETIME,
                63
            )
            .unwrap(),
            Some("2026-10-22 01:02:03.456789".into())
        );
        assert_eq!(
            text(Value::Time(true, 1, 1, 2, 3, 456789), MYSQL_TYPE_TIME, 63).unwrap(),
            Some("-25:02:03.456789".into())
        );
    }
}
