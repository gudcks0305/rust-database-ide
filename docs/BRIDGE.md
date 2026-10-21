# Native database UI bridge

2026-10-21. `crates/dbide-bridge` has no GPUI dependency. `DatabaseBridge` owns a
private Tokio worker thread. UI calls enqueue commands and poll `try_next()`.
`FixtureBridge` remains a compatibility wrapper for the original loopback fixture
and its explicit-reconnect contract; product UI uses `DatabaseBridge`.

## Configuration and events

`ConnectionProfile.engine` selects PostgreSQL, SQLite or MySQL. A private adapter enum
dispatches connection, execution, controls, cancellation and state; all engines share
the existing actor admission, exact cancellation keys and event budgets. PostgreSQL
shared DTO re-exports remain source compatible.

`new()` starts unconfigured. `configure(ConnectionRequest)` validates the profile,
closes the previous idle physical connection, replaces the in-memory request,
and emits `Configured { operation }`. Changing configuration while executing is
rejected; use explicit disconnect/cancel first. An idle Active/Failed transaction
also prevents reconfiguration until Commit/Rollback or an explicit Disconnect.
Rejected profiles never replace the existing session or configured request. Invalid configuration cannot
replace a valid session. Passwords are session memory only; requests cannot be
serialized and their Debug representation is redacted by `dbide-profiles`.

`connect()` and a disconnected `execute(sql)` emit `Connecting { operation,
attempt: 1, target }`, then `Connected` or `ConnectionFailed { diagnostic }`.
The diagnostic includes safe host/port/database, a typed failure kind, fixed
readable message and actionable hint, and available SQLSTATE/OS code. Raw driver
and server connection error strings never reach events. Malformed host values
containing a credential URI are replaced by an invalid-host marker. Network,
refused, DNS, timeout, authentication, missing database, TLS and configuration
failures have distinct PostgreSQL kinds. SQLite/MySQL expose the coarser typed
adapter classifications mapped to existing diagnostic kinds. No credential URI is constructed.

Each Run makes at most one connection attempt before SQL submission. A known
closed idle socket reconnects before dispatch. A failed attempt leaves the
request configured so a later Run tries again. **SQL is submitted exactly once;
there is no retry after dispatch**, including failed or outcome-unknown writes.
PostgreSQL and MySQL accepted cancellation retire their physical connection, according
to the actual terminal report. SQLite interrupts its current statement and normally
preserves its connection; its terminal report determines retirement and transaction
state. A subsequent Run reconnects only if necessary, losing temp tables and
transaction state when a physical connection was retired.

`test_connection(request)` uses a separate temporary connection and emits
`ConnectionTested`; it does not replace the configured request or active idle
session. `inspect_schema()` also uses a separate temporary connection and emits
`SchemaLoaded { objects, truncated }`. It reads schema/table/view/column/type/
nullability candidates from the current engine (`pg_catalog`, MySQL
`information_schema` filtered to `DATABASE()`, or SQLite `main.sqlite_schema` joined
to `pragma_table_xinfo`), retains at most 2,048 column rows and
the driver's result budget, and has a four-second query deadline. Both operations
are rejected during an active user query. They cannot modify its temp tables or
transaction. This initial catalog snapshot is not a complete semantic model,
object DDL browser, or incremental schema cache. SQLite catalog names are bound by
the table-valued pragma expression, never interpolated as SQL identifiers. This
snapshot covers only the selected database (SQLite main), excludes session temp
objects and attached SQLite databases, and uses a separate connection. A SQLite
`:memory:` probe therefore sees its own empty database.

Each `SchemaColumn` exposes `name`, `data_type`, `nullable`,
`primary_key_ordinal: Option<u16>`, `foreign_key: bool`, `default: Option<String>`,
and `extra: Option<String>`. PK order comes from PostgreSQL constraint `conkey`
(included index columns are excluded), MySQL PRIMARY KEY column usage, and
SQLite's native `pk` ordinal. Defaults retain the engine's expression/text form;
extra identifies generated/identity/auto-increment properties where exposed.
SQLite labels INTEGER rowid aliases explicitly; nullable TEXT/composite rowid-table
primary keys and `INTEGER PRIMARY KEY DESC` stay nullable. WITHOUT ROWID/STRICT
nullability comes from native `notnull`. Foreign-key flags identify participation,
not referenced-object details. Object `kind` stays normalized as `table`/`view`
(with existing PostgreSQL materialized/foreign variants). The unchanged 512 KiB
result budget can truncate snapshots before the 2,048-column hard cap.


`list_databases()` and its explicit `refresh_databases()` alias return an operation
ID and emit `DatabasesLoaded { operation, databases, truncated }` after Connecting.
`DatabaseInfo { name, accessible, current }` is a typed catalog entry. Discovery
opens only the currently configured database on one separate metadata connection;
it does not fetch every schema, connect to every advertised database, change the
configured request, or retarget a console. Active user execution rejects discovery;
an idle user transaction, temporary objects and SQL diagnostics remain untouched.

PostgreSQL lists non-template databases with `datallowconn` and advertises CONNECT
privilege through `accessible`. MySQL uses SHOW DATABASES visibility and its actual
probe DATABASE() for `current`. MySQL `accessible=true` means catalog-visible;
a global SHOW DATABASES privilege can expose a database without permission to USE
it, so this is not a successful-connection assertion. Neither engine is probed
per database. SQLite advertises only `main`, representing the configured file.
Attachments belong to the original physical session and are deliberately absent
from this separate probe's list.

Discovery retains at most 256 names, 256 UTF-8 bytes per name and 64 KiB of name
bytes, additionally subject to the existing driver result budget. Unsupported
control-character names and over-budget rows mark `truncated`. PostgreSQL's query
requests at most 257 rows to detect truncation; SHOW DATABASES is drained within
the result/time budgets. Query time is capped at four seconds, connection setup
uses min(profile timeout, four seconds), and exact operation Cancel/Disconnect/Drop interrupt the
probe. Permission/query failures produce a terminal Error with the operation ID
and a fixed catalog/list-permission hint. No raw server connection errors escape.

Lazy browsing is additive: `list_schemas()` emits `SchemasLoaded`,
`list_objects(schema)` emits `ObjectsLoaded`, and `list_columns(RelationRef {
schema, name })` emits `ColumnsLoaded`. Each event carries the exact operation ID,
requested scope where applicable, and `truncated`. `SchemaInfo` contains only a
name; `SchemaObjectInfo` contains schema/name/normalized kind and no columns.
Expanding a schema queries only object names/kinds; expanding one relation queries
only that relation's columns. The compatibility `inspect_schema()` / `SchemaLoaded`
snapshot remains unchanged and is not required for these expansion paths.

Each lazy operation uses its own connection to the configured database and a
four-second query deadline, honors exact cancellation, and rejects active user
execution. PostgreSQL lists visible user schemas in the selected database. MySQL
returns only its selected database as the schema; object/column filters cannot
escape into another database. SQLite exposes only `main`, preserving the original
session's attachments. Changing the configured profile is always explicit.

Schema lists retain at most 256 entries; object/column lists at most 2,048, with an
extra sentinel row requested to detect truncation. Every query also retains the
existing 512 KiB / 4,096-event driver budget. Scope identifiers are bounded to
4,096 UTF-8 bytes each, and NUL/empty scopes fail with operation-targeted feedback.
Filters use PostgreSQL E-literal escaping, MySQL UTF-8 hex literals, or SQLite
literal escaping; identifiers are never concatenated unquoted as SQL syntax.
An absent relation produces a refresh-required error. Oversized result streams
emit partial data with `truncated=true`; an empty partial result is not advertised
as complete. Cache ownership, endpoint/database/scope keys, invalidation and stale
operation-generation checks remain with the caller; these methods perform no
hidden eager loads or cache mutation.

PostgreSQL TLS uses native-tls system trust and hostname verification. `Require` and
`VerifyFull` both require verified TLS; `Require` deliberately offers no insecure
certificate-only policy. Explicit `root_cert_path` adds one PEM root (regular
file, at most 128 KiB) for this connector only. File opening/reading runs on a
blocking-pool task outside the actor; Unix O_NONBLOCK prevents a FIFO open from
blocking before regular-file validation. Symlinks to regular files are allowed.
No OS trust-store mutation and no
plaintext fallback. `Disable` is an explicit profile choice for isolated local
servers. `Session::connect(url)` is retained for existing fixture tests only.

SQLite metadata operations (`inspect_schema`, database discovery and every lazy
catalog query) normalize only a probe-local profile clone to `read_only=true` and
`sqlite_create=false`. Missing files produce visible connection-failure events;
metadata expansion cannot create a database file. The original configured request,
cache-key fingerprint and live console connection remain unchanged. Explicit
Connect, Run and TestConnection keep their original authorized creation policy.

SQLite opens only the selected `sqlite_path`, honoring explicit `sqlite_create` and
`read_only` file flags for those explicit operations; passwords and network fields never enter SQLite. Its event
target uses host `SQLite`, port `0`, and the bounded validated file path as database.
MySQL connection failures map typed adapter errors to fixed credential-safe messages
and existing diagnostic kinds; raw driver strings are never forwarded. Adapter
connection setup is bounded by the profile timeout and existing cancellation epoch;
metadata probes additionally clamp that connection timeout to at most four seconds.

Read-only is engine-specific: SQLite uses a read-only file handle and rejects
write-capable statements; PostgreSQL sets `default_transaction_read_only`; MySQL
sets the session transaction default to READ ONLY. Server-side defaults can be
changed by SQL-capable users, so these settings are convenience safeguards, not
authorization boundaries. SQLite has no server TLS capability; MySQL TLS policy
and adapter limitations are documented in MYSQL-ADAPTER.md.

## Lifecycle and bounded queues

Commands: configure, test_connection, inspect_schema, connect, execute, cancel,
disconnect, apply_batch, list_databases. Every accepted command gets `OperationId`; execution additionally
gets `ExecutionKey { session, generation, execution }`. Cancel requires an exact
execution key or operation ID. Stale cancels never affect a later execution.
Only one user execution is admitted. `set_commit_policy(CommitPolicy)` emits
`CommitPolicyChanged { operation, policy, transaction_state }` and sends no SQL.
It is rejected during execution. Auto/Manual policy is independent of wire state.
Normal Manual Run begins a transaction only from wire Idle. Dedicated `commit()`
and `rollback()` emit the same Executing/Finished IDs as Run, use only the existing
session, and never reconnect, retry, or auto-BEGIN. Failed transactions require
Rollback; Commit is rejected. `ExecutionReport::transaction_state` reports actual
engine state Idle/Active/Failed, or Unknown after retirement/lost readiness. PostgreSQL
uses actual ReadyForQuery via the locally patched API in PG-SPIKE.md. SQLite uses
autocommit state and does not emulate PostgreSQL Failed semantics. MySQL uses server
status and engine transaction behavior.

`apply_batch(Vec<ParameterizedStatement>)` is an exclusive PostgreSQL operation on
the existing live physical connection only. It never auto-connects or retries,
uses current commit policy, and emits the normal Executing/Finished pair. Empty or
invalid batches receive terminal feedback; SQL, statement, parameter-count and
parameter-byte budgets are bounded before/at driver admission. Disconnected apply
requires reconnecting and reloading rows. SQLite/MySQL reject this initial editing
batch API without dispatching SQL. PostgreSQL validates DML and numbered parameters,
uses a transaction or savepoint, checks expected affected rows, and rolls back its
scope on failure/conflict. Manual successful apply remains Active.

`apply_guarded_batch(TableIdentityGuard { schema, table, oid,
metadata_fingerprint }, statements)` forwards the guard unchanged through the same
admission/cancel/transaction path. `metadata_fingerprint: None` retains the legacy
OID-only check: ROW EXCLUSIVE table lock plus catalog OID comparison. The row-editing
UI must pass `Some(captured_sha256)` from its metadata capture. That mode takes a
SHARE UPDATE EXCLUSIVE lock and checks both OID and the captured table-local
metadata fingerprint atomically before preparing or executing DML. This prevents
applying a row plan after the target was dropped/recreated or its table metadata
changed. A mismatch fails and rolls back only the batch scope, preserving prior
work in an existing transaction and its physical session. The fingerprint does
not claim to cover transitive changes to referenced external objects or function
bodies. The bridge re-exports guard/statement types; unguarded `apply_batch`
remains available for compatible callers. Neither apply method opens a connection.

An independent operation-targeted cancellation signal and disconnect epoch also
interrupt connection and metadata futures. Admission captures the epoch, so a
Disconnect or Cancel issued before the worker starts a queued connection still
prevents SQL dispatch. Pending Run emits a terminal Error explaining that SQL
was not submitted. Cancel targets are bounded by admitted commands and removed
when their matching cancel command is processed; a stale target cannot cancel a
new attempt. Disconnect then emits its normal Disconnected acknowledgement.

The select loop polls query completion and commands together. Immediate cancel
waits for the native actor's Running transition or terminal completion. Explicit
disconnect cancels/drains an active query, emits its terminal event, then
Disconnected. Drop signals shutdown independently, returns without joining the
worker, and allows up to eight seconds for cancel/drain before runtime teardown.
Accepted cancel has an independent five-second retirement deadline. Dispatched
writes with missing acknowledgement remain outcome unknown.

The command queue has 16 slots and SQL is capped at 64 KiB. Events have 64 slots;
maximum event reservations per operation are admitted under a 32-slot budget
(four for a disconnected Run, two for connect/test/apply, three for schema/database metadata, one for
configure/cancel/disconnect). Unused reservations are released on every path.
Terminal execution reserves its own slot until polled. Worker event sends never
wait for UI consumption. Unpolled UI eventually receives `QueueFull`.
`SqlTooLarge` and `WorkerStopped` are the other synchronous submission errors.
The driver retains at most an estimated 512 KiB / 4,096 events; this is not an
entire process or transient wire-buffer memory bound.

Ordinary connection establishment uses the configured connection timeout. Every
metadata operation clones its request and clamps only the probe connection phase
to min(configured timeout, four seconds), followed by a separate four-second query
budget. These are separate phases: the whole metadata operation is not promised
to finish within four seconds (connection plus query can take up to about eight).
The original configured timeout and SQL session remain unchanged. Both phases are
immediately interruptible by Drop, Disconnect, or exact operation Cancel. The
connection phase does not dispatch user SQL; there is no retry loop.

## Verification

```sh
cargo test -p dbide-bridge -p dbide-pg-spike -p dbide-sqlite -p dbide-mysql
cargo clippy -p dbide-bridge --all-targets -- -D warnings
# Explicitly provision only the owned local fixtures before ignored tests:
DBIDE_BRIDGE_FIXTURES=owned-loopback \
  cargo test -p dbide-bridge --test engines_contract -- --ignored --test-threads=1
cargo test -p dbide-bridge --test fixture_contract -- --ignored
cargo test -p dbide-bridge --test transactions_contract -- --ignored
DBIDE_TEST_TLS_CA="$PWD/tmp/tls-fixture/ca.pem" \
  cargo test -p dbide-bridge --test connections_contract -- --ignored
```

Live contracts cover original immediate cancel/disconnect/stale-key/drop cleanup,
custom profile auto-connect, repeated failed-port feedback, sanitized auth error,
missing database, test-connection session isolation, separate metadata, refused
active configuration, profile-switch invalidation, idle socket loss and
post-dispatch outcome unknown. TLS fixtures prove untrusted certificates rejected,
explicit CA accepted, `Require` still verifies, and plaintext servers cannot
satisfy `VerifyFull`. Normal tests visibly ignore live contracts. No real user
profiles or credentials are discovered or tested.

The transaction contract also independently observes uncommitted visibility,
Commit/Rollback, Manual beginning the next transaction, direct Auto BEGIN,
failed-transaction recovery, preserving temp tables/uncommitted work on rejected
profile changes, cancellation retirement, and refusing Commit after connection
loss without reconnecting. These are fixture results, not a completed production
adapter or full DataGrip transaction UX claim.

Normal engine contracts use real isolated temporary SQLite files to verify auto-open,
idle-only file switching, transaction-preserving rejected profile changes, exact and
pending cancellation, temp/session preservation, safe errors, read-only writes,
2,048-row metadata truncation and quoted object names. Live engine contracts run
only against owned PostgreSQL 55439/MySQL 55441 fixtures, verifying engine dispatch,
metadata, controls, profile switching and PostgreSQL apply policy. Normal tests
ignore these network contracts explicitly.
