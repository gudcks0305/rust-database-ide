# SQLite native adapter

`crates/dbide-sqlite` is the product-facing SQLite session adapter, based on the
repository-owned `dbide-sqlite-spike` native Batch, progress gate, interrupt lease,
and prepare-time authorization experiments. It does not copy reference IDE code.
The pinned dependency is `rusqlite = 0.40.2` with bundled SQLite and hooks.

## Connection and public API

`Session::connect_request(&ConnectionRequest, Limits).await` validates a SQLite
profile and opens the physical connection on a named background thread. Password,
network, TLS, and certificate fields are unused. Connection diagnostics expose
only `Configuration`, `NotFound`, `Permission`, `Busy`, or `Io` plus fixed message
and hint strings; they never include a path, SQL, or password.

A file must exist by default. Only explicit `sqlite_create=true` enables SQLite's
create flag. A read-only profile never enables create. `:memory:` explicitly
selects a private in-memory database and does not require the create checkbox.
URI interpretation, loadable extensions, and shared connection caches are not
enabled. An existing file and a directory with the same name are not treated as
interchangeable; SQLite validates its own file format when used.

A cloneable `Session` owns a bounded sender; a single worker exclusively owns one
native connection. `submit(sql, policy)`, `submit_commit(policy)`, and
`submit_rollback(policy)` return a ticket with a shared `ExecutionKey` and async
`finish()`. Convenience `execute`, `commit`, and `rollback` await that ticket.
`transaction_state`, `is_closed`, and `close` are synchronous; `is_running(key)`
and `cancel(key)` match the bridge's asynchronous driver interface. Submitting,
opening, executing, and dropping never wait for native SQL on the UI thread.
Session IDs use the shared driver API counter, preventing cross-engine collisions.

## Transaction, result, and error semantics

Manual policy begins a transaction only before ordinary SQL when native
`Connection::is_autocommit()` reports Idle. The commit/rollback controls bypass
that auto-BEGIN. Direct SQL transaction/savepoint commands remain native SQL.
Each terminal report and cached session state derives from `is_autocommit()`:
SQLite returns Idle or Active, without inventing PostgreSQL's Failed state.
For example a uniqueness violation commonly leaves Active, while `INSERT OR
ROLLBACK` can return Idle. Switching policy never implicitly commits existing work.

Native `Batch` prepares statement boundaries, including quoted semicolons,
comments, and entire trigger bodies. Commands and rows retain statement order;
a real statement error stops the remainder and preserves earlier events.
Each successfully drained result stream ends with `CommandComplete` carrying its
native produced-row count, including zero for an empty SELECT. A mid-stream error
preserves earlier rows without emitting a false completion. This keeps SELECT,
DDL, and later SELECT pages distinct. `CommandCompleteUnknown` is intentional for
all zero-column statements. We do not label DDL, control statements, or trigger
work with stale `sqlite3_changes`.

Shared display values represent NULL as `None`, text as UTF-8 with replacement
for invalid byte sequences, integer/real as decimal text, and blobs as `x'hex'`.
This display contract is not a lossless typed value or editable-LOB API.
A bounded terminal native error is retained separately even after truncation.
SQLite's error code appears in `sqlstate` as an engine-specific `SQLITE_*` string;
it is not a PostgreSQL SQLSTATE.

## Budgets and script behavior

The queue accepts at most eight pending executions. SQL is bounded by `Limits`
and a hard 1 MiB cap (default 64 KiB). Retained report bytes and events respect
`Limits`, capped at 8 MiB and 65,536 events; defaults come from the shared API.
Per-event accounting includes conservative string/container overhead. Large
text/blob output is measured before creating its display allocation.
The byte count bounds retained report accounting, not total SQLite/native RAM.

When retention fills, the adapter sets `truncated=true` and drains native rows;
subsequent writes in that script still execute. Truncation is therefore not a
script cancellation or rollback. Native errors still stop subsequent statements.
There is also an independent 256-statement script cap. Exceeding that cap is an
explicit error stating the remaining script was not executed. Before preparing
an excess statement, a deny-all native authorizer prevents prepare-time PRAGMA
side effects. Native empty/comment-only trailing SQL remains accepted. The
previous authorizer policy is restored before the next execution.

Native lock contention has a 250 ms busy timeout. SQL changes to `PRAGMA
busy_timeout` are denied on every connection, keeping that worker policy bounded.
Filesystem stalls and arbitrary SQLite computation are not real-time guarantees.

## Read-only enforcement

Read-only uses native `SQLITE_OPEN_READ_ONLY`, `PRAGMA query_only=ON`, and a
persistent fail-closed authorizer. SQL cannot turn query_only off, attach a second
file, write temporary objects, mutate schema/data, or run writable PRAGMAs.
Select/read/function/transaction/savepoint operations and a narrow allowlist of
metadata/read PRAGMAs remain available. Extension loading is denied.
The authorizer remains installed after both normal execution and statement-cap
errors. An unknown future authorization action is denied in read-only mode.
This is the adapter's SQL policy, not an operating-system sandbox or protection
against a separate process writing the same file.

## Cancel, abandonment, and shutdown

Cancellation is out of band. `cancel(key)` returns `Requested` only while the
exact session/generation/execution lease is active. It sets a cancellation flag
and invokes SQLite's native interrupt while holding the same mutex that guards
worker handoff. The flag also covers statement gaps where no native step is
running. Late/stale/cross-session keys return `NotRunning` and cannot interrupt
the next execution. Native interrupt is observed as a Cancelled terminal report;
the connection remains reusable and reports its actual transaction state.
Earlier committed statements are not rolled back by cancelling a later statement.

Dropping an unfinished ticket marks it abandoned: a queued ticket never starts,
and an active ticket stops at its next progress callback or statement boundary.
`close()` and last Session drop mark the shared lease closed and interrupt active
work. The worker rejects all pending SQL, drops the connection itself, and exits.
There is no caller-side join and no restart of queued long queries during close.
A closed session reports cached transaction state as Unknown; native terminal
reports still contain the observed state before physical connection disposal.

## Verification

Run `cargo test -p dbide-sqlite` and
`cargo clippy -p dbide-sqlite --all-targets -- -D warnings`.
Tests use only `:memory:` or generated files under `tempfile::tempdir`; they never
read saved profiles or open user databases. Native fixture tests cover ordered
trigger scripts and partial errors, display values, dedicated TEMP state,
manual commit/rollback and external observer visibility, native rollback state,
explicit file creation and safe diagnostics, read-only SQL bypass attempts,
competing-writer timeout/recovery, bounded retain-and-drain writes, prepare-time
PRAGMA cap enforcement, progress-gated native interrupt with stale-key rejection
and connection reuse, queue/input bounds, abandoned tickets, and close/drop while
a native query and a queued query are pending.
