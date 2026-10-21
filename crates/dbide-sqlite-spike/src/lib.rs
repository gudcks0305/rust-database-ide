//! M0 contract probe, not the product driver. Never opens user profiles.
use rusqlite::fallible_iterator::FallibleIterator;

use rusqlite::hooks::{AuthContext, Authorization};
use rusqlite::{Batch, Connection, ErrorCode, OpenFlags, types::ValueRef};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "value")]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}
#[derive(Debug, Serialize)]
pub enum Event {
    Rows(Vec<Vec<Value>>),
    /// None means affected rows are not reliably classified for this statement.
    Command(Option<usize>),
}
#[derive(Debug, Serialize)]
pub struct Failure {
    pub statement_index: usize,
    pub code: Option<String>,
    pub message: String,
}
#[derive(Debug, Serialize)]
pub struct Execution {
    pub events: Vec<Event>,
    pub error: Option<Failure>,
    pub autocommit: bool,
    pub elapsed_us: u128,
    pub payload_bytes: usize,
}
impl Execution {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
}

pub enum Database {
    Memory,
    File { path: PathBuf, read_only: bool },
}
/// Explicit small-fixture budget: stop script with an error instead of silently truncating writes.
#[derive(Clone, Copy)]
pub struct Budget {
    pub rows: usize,
    pub bytes: usize,
    pub statements: usize,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            rows: 1024,
            bytes: 1_048_576,
            statements: 64,
        }
    }
}
struct Probe {
    entered: mpsc::SyncSender<()>,
    resume: mpsc::Receiver<()>,
}
struct Command {
    id: u64,
    sql: String,
    budget: Budget,
    probe: Option<Probe>,
    reply: mpsc::SyncSender<Execution>,
}
pub struct Ticket {
    pub id: u64,
    reply: mpsc::Receiver<Execution>,
}
impl Ticket {
    /// Blocking fixture/CLI collection; UI must poll/forward the response off its event thread.
    pub fn wait(self) -> Execution {
        self.reply
            .recv_timeout(Duration::from_secs(10))
            .expect("worker response within fixture deadline")
    }
}
/// One physical connection per worker. Bounded queue; try_send never blocks the UI.
pub struct Session {
    tx: Option<mpsc::SyncSender<Command>>,
    worker: Option<thread::JoinHandle<()>>,
    active: Arc<Mutex<Option<u64>>>,
    stopping: Arc<AtomicBool>,
    interrupt: rusqlite::InterruptHandle,
    next_id: u64,
}
impl Session {
    pub fn open(database: Database, timeout: Duration) -> rusqlite::Result<Self> {
        let (tx, rx) = mpsc::sync_channel::<Command>(8);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let active = Arc::new(Mutex::new(None));
        let state = active.clone();
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = stopping.clone();
        let worker = thread::Builder::new()
            .name("sqlite-contract-worker".into())
            .spawn(move || {
                let connection = match database {
                    Database::Memory => Connection::open_in_memory(),
                    Database::File { path, read_only } => Connection::open_with_flags(
                        path,
                        if read_only {
                            OpenFlags::SQLITE_OPEN_READ_ONLY
                        } else {
                            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
                        },
                    ),
                }
                .and_then(|c| {
                    c.busy_timeout(timeout)?;
                    Ok(c)
                });
                let c = match connection {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                if ready_tx.send(Ok(c.get_interrupt_handle())).is_err() {
                    return;
                }
                for command in rx {
                    {
                        let mut active = state.lock().unwrap();
                        if worker_stopping.load(Ordering::Acquire) {
                            let _ = command.reply.send(Execution {
                                events: vec![],
                                error: Some(Failure {
                                    statement_index: 0,
                                    code: Some("SessionClosing".into()),
                                    message: "queued execution rejected during shutdown".into(),
                                }),
                                autocommit: c.is_autocommit(),
                                elapsed_us: 0,
                                payload_bytes: 0,
                            });
                            continue;
                        }
                        *active = Some(command.id);
                    }
                    {
                        let mut probe = command.probe;
                        let stop = worker_stopping.clone();
                        c.progress_handler(
                            1000,
                            Some(move || {
                                if let Some(p) = probe.take() {
                                    let _ = p.entered.send(());
                                    // Deadline is fault containment, not a timing-based synchronization.
                                    let _ = p.resume.recv_timeout(Duration::from_secs(5));
                                }
                                stop.load(Ordering::Acquire)
                            }),
                        )
                        .expect("install fixture progress handler");
                    }
                    let result = execute(&c, &command.sql, command.budget);
                    c.progress_handler(0, None::<fn() -> bool>)
                        .expect("remove fixture progress handler");
                    *state.lock().unwrap() = None;
                    let _ = command.reply.send(result);
                }
            })
            .expect("spawn SQLite worker");
        let interrupt = ready_rx.recv().expect("worker initializes")?;
        Ok(Self {
            tx: Some(tx),
            worker: Some(worker),
            active,
            stopping,
            interrupt,
            next_id: 1,
        })
    }
    pub fn submit(&mut self, sql: &str, budget: Budget) -> Result<Ticket, String> {
        self.submit_probe(sql, budget, None)
    }
    fn submit_probe(
        &mut self,
        sql: &str,
        budget: Budget,
        probe: Option<Probe>,
    ) -> Result<Ticket, String> {
        if sql.len() > 64 * 1024 {
            return Err("fixture SQL exceeds 64 KiB".into());
        }
        let (reply, rx) = mpsc::sync_channel(1);
        let id = self.next_id;
        self.next_id += 1;
        self.tx
            .as_ref()
            .unwrap()
            .try_send(Command {
                id,
                sql: sql.into(),
                budget,
                probe,
                reply,
            })
            .map_err(|_| "worker queue full or closed".to_string())?;
        Ok(Ticket { id, reply: rx })
    }
    pub fn cancel(&self, id: u64) -> bool {
        let active = self.active.lock().unwrap();
        if *active == Some(id) {
            self.interrupt.interrupt();
            true
        } else {
            false
        }
    }
    pub fn run(&mut self, sql: &str) -> Execution {
        self.submit(sql, Budget::default()).unwrap().wait()
    }
    fn begin_shutdown(&mut self) {
        // Same lock serializes shutdown and admission of the next queued command.
        let _active = self.active.lock().unwrap();
        self.stopping.store(true, Ordering::Release);
        self.interrupt.interrupt();
        self.tx.take();
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        // Closing this fixture session interrupts outstanding native execution before joining.
        self.begin_shutdown();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn execute(c: &Connection, sql: &str, budget: Budget) -> Execution {
    let started = Instant::now();
    let mut out = Execution {
        events: vec![],
        error: None,
        autocommit: c.is_autocommit(),
        elapsed_us: 0,
        payload_bytes: 0,
    };
    let mut index = 0;
    let mut total_rows = 0;
    let result: rusqlite::Result<()> = (|| {
        if budget.statements == 0 {
            return Err(rusqlite::Error::InvalidParameterName(
                "zero statement budget; script not prepared".into(),
            ));
        }
        let mut batch = Batch::new(c, sql);
        loop {
            // Batch.next prepares natively; some PRAGMAs mutate during prepare.
            // At the cap, allow SQLite to recognize an empty/comment-only tail
            // while denying every native authorization action before it happens.
            let at_limit = index >= budget.statements;
            if at_limit {
                c.authorizer(Some(|_: AuthContext<'_>| Authorization::Deny))?;
            }
            let next = batch.next();
            if at_limit {
                c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
                if matches!(next, Ok(None)) {
                    break;
                }
                return Err(rusqlite::Error::InvalidParameterName(
                    "statement budget exceeded".into(),
                ));
            }
            let Some(mut statement) = next? else {
                break;
            };
            let columns = statement.column_count();
            if columns == 0 {
                statement.execute([])?;
                // sqlite3_changes is connection state, and DDL/transaction commands
                // leave a previous DML count untouched. Without native statement
                // classification, an unknown count is safer than stale data.
                out.events.push(Event::Command(None));
            } else {
                let mut values = Vec::new();
                let mut rows = statement.query([])?;
                while let Some(row) = rows.next()? {
                    if total_rows >= budget.rows {
                        return Err(rusqlite::Error::InvalidParameterName(
                            "row budget exceeded; script stopped".into(),
                        ));
                    }
                    let mut current = Vec::new();
                    for col in 0..columns {
                        let v = row.get_ref(col)?;
                        let bytes = match v {
                            ValueRef::Text(b) | ValueRef::Blob(b) => b.len(),
                            _ => 8,
                        };
                        if bytes > budget.bytes.saturating_sub(out.payload_bytes) {
                            return Err(rusqlite::Error::InvalidParameterName(
                                "payload byte budget exceeded; script stopped".into(),
                            ));
                        }
                        out.payload_bytes += bytes;
                        current.push(match v {
                            ValueRef::Null => Value::Null,
                            ValueRef::Integer(n) => Value::Integer(n),
                            ValueRef::Real(n) => Value::Real(n),
                            ValueRef::Text(b) => Value::Text(
                                std::str::from_utf8(b)
                                    .map_err(|e| rusqlite::Error::Utf8Error(col, e))?
                                    .into(),
                            ),
                            ValueRef::Blob(b) => Value::Blob(b.into()),
                        });
                    }
                    total_rows += 1;
                    values.push(current);
                }
                out.events.push(Event::Rows(values));
            }
            index += 1;
        }
        Ok(())
    })();
    if let Err(error) = result {
        out.error = Some(Failure {
            statement_index: index,
            code: error.sqlite_error_code().map(|c| format!("{c:?}")),
            message: error.to_string(),
        });
    }
    out.autocommit = c.is_autocommit();
    out.elapsed_us = started.elapsed().as_micros();
    out
}

#[derive(Serialize)]
pub struct Case {
    pub name: &'static str,
    pub passed: bool,
    pub elapsed_us: u128,
    pub detail: String,
}
#[derive(Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub scope: &'static str,
    pub rusqlite: &'static str,
    pub sqlite: &'static str,
    pub timestamp_unix_ms: u128,
    pub passed: bool,
    pub cases: Vec<Case>,
}
fn row(execution: &Execution, event: usize) -> &[Value] {
    match &execution.events[event] {
        Event::Rows(rows) => &rows[0],
        _ => panic!("expected rows"),
    }
}
fn code(execution: &Execution, expected: ErrorCode) -> bool {
    execution
        .error
        .as_ref()
        .and_then(|e| e.code.as_ref())
        .is_some_and(|s| *s == format!("{expected:?}"))
}
fn require(value: bool, message: &str) -> Result<(), String> {
    if value { Ok(()) } else { Err(message.into()) }
}
fn memory() -> Session {
    Session::open(Database::Memory, Duration::from_millis(50)).unwrap()
}
fn add(cases: &mut Vec<Case>, name: &'static str, f: impl FnOnce() -> Result<String, String>) {
    let start = Instant::now();
    let result = f();
    cases.push(Case {
        name,
        passed: result.is_ok(),
        elapsed_us: start.elapsed().as_micros(),
        detail: result.unwrap_or_else(|e| e),
    });
}
/// Generates isolated fixtures only. Per-case times are one observation, not benchmark percentiles.
pub fn verify() -> Report {
    let mut cases = vec![];
    add(&mut cases, "native_batch_order_partial_error", || {
        let mut s = memory();
        let r = s.run("CREATE TABLE a(v); CREATE TABLE audit(v); CREATE TRIGGER a_log AFTER INSERT ON a BEGIN INSERT INTO audit VALUES('trigger; body'); INSERT INTO audit VALUES(new.v); END; INSERT INTO a VALUES('한글; 문자열'); -- ; comment\n SELECT v FROM audit ORDER BY rowid; SELECT * FROM missing; SELECT 99;");
        require(
            r.events.len() == 5 && r.error.as_ref().is_some_and(|e| e.statement_index == 5),
            "partial events or error index lost",
        )?;
        require(
            row(&r, 4) == [Value::Text("trigger; body".into())],
            "native trigger parser failed",
        )?;
        Ok("trigger body, quoted/comment semicolons, ordered commands/results; error stops trailing SELECT".into())
    });
    add(&mut cases, "temp_session_and_direct_transaction", || {
        let mut a = memory();
        let mut b = memory();
        require(
            a.run("CREATE TEMP TABLE t(v); INSERT INTO t VALUES(1)")
                .ok(),
            "temp setup failed",
        )?;
        require(!b.run("SELECT * FROM t").ok(), "temp table leaked")?;
        require(
            !a.run("BEGIN; INSERT INTO t VALUES(2)").autocommit,
            "BEGIN state incorrect",
        )?;
        require(a.run("ROLLBACK").autocommit, "ROLLBACK state incorrect")?;
        require(
            row(&a.run("SELECT count(*) FROM t"), 0) == [Value::Integer(1)],
            "rollback did not undo write",
        )?;
        require(
            !a.run("BEGIN").autocommit && a.run("COMMIT").autocommit,
            "COMMIT state incorrect",
        )?;
        Ok(
            "worker connection persists; second session isolated; is_autocommit follows direct SQL"
                .into(),
        )
    });
    add(&mut cases, "sqlite_storage_classes_roundtrip", || {
        let mut s = memory();
        let r = s.run("CREATE TABLE types(n,e,i,r,b,k); INSERT INTO types VALUES(NULL,'',9223372036854775807,1.25,x'0001ff','한글'); SELECT * FROM types");
        require(
            r.ok()
                && row(&r, 2)
                    == [
                        Value::Null,
                        Value::Text("".into()),
                        Value::Integer(i64::MAX),
                        Value::Real(1.25),
                        Value::Blob(vec![0, 1, 255]),
                        Value::Text("한글".into()),
                    ],
            "storage classes changed",
        )?;
        Ok("NULL, empty text, i64 maximum, exact binary fraction, bytes, Korean persisted and read".into())
    });
    add(&mut cases, "readonly_and_competing_writer_timeout", || {
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        let path = dir.path().join("fixture.sqlite");
        let mut a = Session::open(
            Database::File {
                path: path.clone(),
                read_only: false,
            },
            Duration::from_millis(50),
        )
        .map_err(|e| e.to_string())?;
        require(a.run("CREATE TABLE t(v)").ok(), "file setup failed")?;
        let mut ro = Session::open(
            Database::File {
                path: path.clone(),
                read_only: true,
            },
            Duration::from_millis(50),
        )
        .map_err(|e| e.to_string())?;
        require(
            code(&ro.run("INSERT INTO t VALUES(1)"), ErrorCode::ReadOnly),
            "read-only write accepted or wrong error",
        )?;
        let mut b = Session::open(
            Database::File {
                path,
                read_only: false,
            },
            Duration::from_millis(50),
        )
        .map_err(|e| e.to_string())?;
        require(
            a.run("BEGIN IMMEDIATE; INSERT INTO t VALUES(2)").ok(),
            "writer lock not held",
        )?;
        require(
            row(&b.run("SELECT count(*) FROM t"), 0) == [Value::Integer(0)],
            "other session observed uncommitted write",
        )?;
        let start = Instant::now();
        let busy = b.run("INSERT INTO t VALUES(3)");
        let elapsed = start.elapsed();
        require(
            code(&busy, ErrorCode::DatabaseBusy),
            "second writer did not return SQLITE_BUSY",
        )?;
        require(
            elapsed >= Duration::from_millis(40) && elapsed < Duration::from_secs(5),
            "busy timeout outside broad fixture bounds",
        )?;
        require(
            a.run("ROLLBACK").ok() && b.run("INSERT INTO t VALUES(3)").ok(),
            "writer did not recover",
        )?;
        Ok(format!(
            "read-only native error; configured busy_timeout=50ms; observed {}us; writer recovers after rollback",
            elapsed.as_micros()
        ))
    });
    add(&mut cases, "interrupt_scoped_and_next_execution", || {
        let mut s = memory();
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        let ticket = s.submit_probe("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n", Budget::default(), Some(Probe { entered, resume }))?;
        let id = ticket.id;
        ready
            .recv_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        require(s.cancel(id), "running execution rejected cancel")?;
        release.send(()).map_err(|e| e.to_string())?;
        require(
            code(&ticket.wait(), ErrorCode::OperationInterrupted),
            "native interrupt not observed",
        )?;
        require(
            !s.cancel(id),
            "completed execution accepted late cancellation",
        )?;
        let next = s.run("SELECT 42");
        require(
            next.ok() && row(&next, 0) == [Value::Integer(42)],
            "next query was interrupted",
        )?;
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        let next = s.submit_probe("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) SELECT sum(x) FROM n", Budget::default(), Some(Probe { entered, resume }))?;
        ready
            .recv_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        require(
            !s.cancel(id),
            "stale id cancelled a different active execution",
        )?;
        release.send(()).map_err(|e| e.to_string())?;
        let next = next.wait();
        require(
            next.ok() && row(&next, 0) == [Value::Integer(50_005_000)],
            "late cancellation leaked into active next execution",
        )?;
        Ok("progress handshake proves native execution; native interrupt terminal response; stale id rejected both idle and while next execution active".into())
    });
    add(&mut cases, "bounded_result_and_script_stop", || {
        let mut s = memory();
        let r = s
            .submit(
                "SELECT 'oversized'; CREATE TEMP TABLE must_not_exist(v)",
                Budget {
                    rows: 1,
                    bytes: 2,
                    statements: 2,
                },
            )?
            .wait();
        require(
            !r.ok() && r.payload_bytes <= 2 && !s.run("SELECT * FROM must_not_exist").ok(),
            "byte limit failed or script continued",
        )?;
        let r = s
            .submit(
                "SELECT 1 UNION ALL SELECT 2",
                Budget {
                    rows: 1,
                    bytes: 1024,
                    statements: 2,
                },
            )?
            .wait();
        require(!r.ok(), "row limit failed")?;
        Ok("global row/payload budgets stop script; bounded SQL 64KiB and command queue 8".into())
    });
    Report {
        schema_version: 1,
        scope: "M0 isolated SQLite contract verifier, not production driver or UI benchmark",
        rusqlite: "0.40.2 (bundled, hooks)",
        sqlite: rusqlite::version(),
        timestamp_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
        passed: cases.iter().all(|c| c.passed),
        cases,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_contracts() {
        let r = verify();
        for c in &r.cases {
            assert!(c.passed, "{}: {}", c.name, c.detail);
        }
    }
    #[test]
    fn native_batch_result_order() {
        let mut s = memory();
        let r = s.run("CREATE TABLE t(v); INSERT INTO t VALUES(1),(2); SELECT v FROM t ORDER BY v; UPDATE t SET v=v+1; SELECT sum(v) FROM t");
        assert!(r.ok());
        assert!(matches!(r.events[1], Event::Command(None)));
        assert!(matches!(r.events[3], Event::Command(None)));
        assert_eq!(row(&r, 4), [Value::Integer(5)]);
    }
    #[test]
    fn statement_budget_stops_script() {
        let mut s = memory();
        let r = s
            .submit(
                "SELECT 1; CREATE TABLE omitted(v)",
                Budget {
                    rows: 10,
                    bytes: 100,
                    statements: 1,
                },
            )
            .unwrap()
            .wait();
        assert!(!r.ok());
        assert!(!s.run("SELECT * FROM omitted").ok());
    }

    #[test]
    fn command_counts_never_reuse_previous_dml() {
        let mut s = memory();
        let r = s.run("CREATE TABLE t(v); INSERT INTO t VALUES(1),(2); CREATE TABLE later(v); BEGIN; COMMIT; BEGIN; ROLLBACK; SELECT count(*) FROM t");
        assert!(r.ok());
        assert_eq!(r.events.len(), 8);
        for event in &r.events[..7] {
            assert!(matches!(event, Event::Command(None)), "{event:?}");
        }
        assert_eq!(row(&r, 7), [Value::Integer(2)]);
        assert!(r.autocommit);
    }

    #[test]
    fn statement_budget_blocks_prepare_time_pragma_side_effects() {
        let mut s = memory();
        assert!(s.run("PRAGMA foreign_keys=OFF").ok());
        let r = s
            .submit(
                "SELECT 1; PRAGMA foreign_keys=ON",
                Budget {
                    statements: 1,
                    ..Budget::default()
                },
            )
            .unwrap()
            .wait();
        assert!(!r.ok());
        assert_eq!(row(&s.run("PRAGMA foreign_keys"), 0), [Value::Integer(0)]);
        let exact = s
            .submit(
                "SELECT 1; ; /* tail only */ -- empty\n",
                Budget {
                    statements: 1,
                    ..Budget::default()
                },
            )
            .unwrap()
            .wait();
        assert!(
            exact.ok(),
            "trailing native empty statements must remain valid"
        );
        let next = s.run("PRAGMA foreign_keys=ON; PRAGMA foreign_keys; SELECT 42");
        assert!(
            next.ok(),
            "budget authorizer must be removed before next script"
        );
        assert_eq!(row(&next, 1), [Value::Integer(1)]);
        assert_eq!(row(&next, 2), [Value::Integer(42)]);
    }

    #[test]
    fn zero_statement_budget_never_prepares_even_empty_prefixes() {
        let mut s = memory();
        for sql in [
            "",
            "; ; -- empty\n",
            "; ; /* skipped */ CREATE TABLE forbidden(v)",
            "PRAGMA user_version=42",
        ] {
            let r = s
                .submit(
                    sql,
                    Budget {
                        statements: 0,
                        ..Budget::default()
                    },
                )
                .unwrap()
                .wait();
            assert!(!r.ok(), "zero statement budget must reject before prepare");
            assert!(r.events.is_empty());
        }
        assert!(!s.run("SELECT * FROM forbidden").ok());
        assert_eq!(row(&s.run("PRAGMA user_version"), 0), [Value::Integer(0)]);
        let r = s
            .submit(
                "; ; /* empty */ CREATE TABLE allowed(v); ; -- empty\n CREATE TABLE forbidden(v)",
                Budget {
                    statements: 1,
                    ..Budget::default()
                },
            )
            .unwrap()
            .wait();
        assert!(!r.ok());
        assert_eq!(r.events.len(), 1);
        assert!(s.run("SELECT * FROM allowed").ok());
        assert!(!s.run("SELECT * FROM forbidden").ok());
    }

    #[test]
    fn shutdown_interrupts_running_and_rejects_queued_long_query() {
        let mut s = memory();
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        let sql = "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n";
        let running = s
            .submit_probe(sql, Budget::default(), Some(Probe { entered, resume }))
            .unwrap();
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let queued = s.submit(sql, Budget::default()).unwrap();
        // Use the same shutdown entry point as Drop while the native query is gated.
        s.begin_shutdown();
        release.send(()).unwrap();
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let joiner = thread::spawn(move || {
            drop(s);
            done_tx.send(()).unwrap();
        });
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("shutdown must not start the queued billion-row query");
        joiner.join().unwrap();
        assert!(code(&running.wait(), ErrorCode::OperationInterrupted));
        let queued = queued.wait();
        assert_eq!(
            queued.error.unwrap().code.as_deref(),
            Some("SessionClosing")
        );
        assert!(queued.events.is_empty());
    }
}
