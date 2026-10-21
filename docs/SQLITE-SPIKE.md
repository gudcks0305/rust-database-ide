# M0 SQLite contract verifier

`dbide-sqlite-spike` is an executable fixture verifier and small reusable library.
It is not the production adapter, a UI, or a DataGrip performance comparison.
It creates only in-memory databases and an isolated automatically deleted temporary
file. No profile discovery, user database access, credentials, or network DB access.

```sh
cargo test -p dbide-sqlite-spike
cargo run --release -p dbide-sqlite-spike
# Optional durable report, from repository root:
mkdir -p tmp
cargo run --release -p dbide-sqlite-spike > tmp/sqlite-contract.json
```

JSON includes schema version, wall-clock timestamp, actual `sqlite3_libversion`,
pinned rusqlite version/features, per-case pass/failure, and measured microseconds.
Exit status is nonzero on failed assertions. Each timing is one instrumented
fixture observation, not a percentile, throughput result, or UI latency metric.
Use Cargo.lock and `rustc -Vv` with a saved report to reproduce build provenance.

## Implemented contract

- A dedicated named OS worker creates and owns each physical connection. The
  bounded command queue uses `try_send`; it does not execute SQLite on a UI thread.
  `Ticket::wait` and session initialization/destruction are blocking fixture APIs;
  product UI integration must move lifecycle/collection off its event thread.
- `rusqlite::Batch` delegates boundaries to SQLite's native preparation parser.
  Ordered command completions and result sets survive a later statement error, including
  its zero-based statement index. The fixture covers trigger bodies, quoted text,
  comments containing semicolons, and trailing statements after an error.
  `Event::Command(Option<usize>)` reports `None` for affected rows: this spike does
  not classify zero-column statements. `sqlite3_changes` can retain a prior DML
  count through DDL or transaction commands, so its value is not presented as
  the current command's count.
- One persistent worker connection retains temporary tables across requests;
  a separate session cannot see them. Direct `BEGIN`, `COMMIT`, and `ROLLBACK`
  are verified using `Connection::is_autocommit`, including actual rollback data.
  No invented CommitPolicy is reported as engine transaction state.
- SQLite storage classes survive table insert/select: NULL, empty text, i64 max,
  an exactly representable real, binary including NUL/0xff, and Korean text.
  This does not establish lossless arbitrary decimal or vendor types.
- Native read-only file flags reject writes. Competing writers use two worker
  connections on one isolated file; acknowledged `BEGIN IMMEDIATE` establishes
  the lock before the contender. A configured 50 ms busy timeout must return
  SQLITE_BUSY within broad 40 ms–5 s fixture bounds, then recover after rollback.
- Cancellation uses a separate native `InterruptHandle`. An execution id is
  checked under the same lock used to mark terminal/next execution boundaries.
  A progress-handler handshake proves the recursive aggregate has entered native
  execution before interruption; no sleep-based query-start race. The terminal
  SQLITE_INTERRUPT is collected, late cancellation is rejected, and SELECT 42
  succeeds on the same connection afterward.
- Shutdown marks a shared stopping flag under the command-admission lock, closes
  the sender, and interrupts native execution. The worker rejects queued commands
  with `SessionClosing` instead of executing them. Its progress handler also checks
  the flag, covering shutdown just before native execution starts. A synchronized
  test gates the running query, queues another billion-row aggregate, then proves
  shutdown joins and the queued query receives no execution events.

## Deliberate limits

This is a bounded **small-contract fixture**, not the planned streaming result
store. Defaults cap stored rows across a script at 1,024, raw value payload at
1 MiB, statements at 64, SQL input at 64 KiB, and queued commands at 8. A zero
statement budget rejects even an empty script before native preparation. Native
empty statements/comments do not consume a positive statement budget.

At a positive statement cap, the next native parse runs under SQLite's deny-all
authorizer; an empty/comment-only tail succeeds, while another statement is
rejected before its authorization actions. This matters because preparation
itself can apply some PRAGMAs. A regression reproduced `SELECT 1; PRAGMA
foreign_keys=ON` changing the connection despite the former post-prepare budget
check, then verifies the authorizer prevents the change and is removed before
the next request. The worker owns this authorizer; it is not a composable product
authorization policy.

Value byte limits are checked before copying a borrowed SQLite text/blob. Numeric/NULL slots
are charged 8 bytes; vector/string allocator overhead and JSON encoding expansion
are not included. There is no process RSS guarantee or limit on SQLite's internal
query engine allocations. A budget error stops the script without auto rollback;
prior successful writes may already have taken effect. The current result set is
not returned if its collection fails; earlier completed events are preserved.

The executable runs a fixed corpus only. A 10-second response timeout is a fixture
failure guard, not a production deadline/cancel implementation. The cancellation
fixture pauses once inside a progress callback; it tests native interruption and
stale-id isolation, not production cancellation latency under arbitrary SQL,
connection generation/reconnection, or cancel between script
statements. Generation handling, nonblocking lifecycle, async event delivery,
partial current-result batches, explicit cancellation outcome state, query policy,
spill, LOB handles, disconnect outcome uncertainty, and production error taxonomy
remain M1+ work. No production readiness or feature parity claim.

## Dependency evidence

As checked 2026-10-21, official [rusqlite documentation](https://docs.rs/rusqlite/0.40.2/rusqlite/)
reports 0.40.2; the manifest pins `=0.40.2` with `bundled` and `hooks`.
[Connection docs](https://docs.rs/rusqlite/0.40.2/rusqlite/struct.Connection.html)
define `is_autocommit`, `busy_timeout`, `progress_handler`, and
`get_interrupt_handle`. The bundled SQLite C engine means this is Rust product
code with a native C dependency, not transitive pure Rust.

## Initial local observation

2026-10-21, macOS ARM64, Rust 1.99.1: the seven crate tests passed and all six
JSON verifier cases passed against bundled SQLite **3.53.2**. Build contention
with other M0 spikes was avoided using `--target-dir tmp/sqlite-target`.
The debug run observed the 50 ms competing-writer timeout at about 69 ms;
this is a single fixture timing, not a performance claim. Raw regenerated output
belongs in local `tmp/sqlite-contract.json`; it is not checked in as a baseline.
