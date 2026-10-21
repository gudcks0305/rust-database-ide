# PostgreSQL native driver M0 spike

2026-10-21. Source: `crates/dbide-pg-spike`. This is an executable driver-contract
experiment, not a production adapter or a parity/performance claim.

## Run

Use a caller-provisioned **disposable local PostgreSQL database**, never a saved
user profile. The fixture role needs CREATE TABLE and permission to terminate its
own backend. The suite creates one uniquely named regular table and temporary
tables; it drops the regular table on success. A failed run can leave its generated
`dbide_contract_*` table in the disposable database.

```sh
# Set DBIDE_TEST_PG_URL explicitly to the disposable database (loopback only).
cargo run -p dbide-pg-spike
cargo test -p dbide-pg-spike --test postgres_contract -- --ignored --nocapture
```

The CLI prints JSON evidence only after every assertion passes, exits 2 when the
URL is absent, and exits 1 on contract failure or its 90 second deadline. The
integration test is visibly `ignored` in normal workspace tests. Running the
explicit ignored test without the variable fails; no database means no pass.
Do not store a credential-bearing URL in evidence. The fixture URL is supplied
only via `DBIDE_TEST_PG_URL`; this crate never discovers profiles or credentials.
NoTls is for this isolated fixture verifier only. The typed-profile TLS/auth extension is documented below; Keychain/SSH remain untested.

## Implemented contract

- One actor owns one physical `tokio-postgres` client. No pool checkout or replay.
  Each request carries session ID, connection generation, execution ID. Session
  IDs are process-unique counters; generation is 1 because reconnect is absent.
- The command queue has 16 slots and SQL defaults to at most 64 KiB per request.
  A dropped reply receiver does not abort or replay server execution.
- Raw SQL goes unmodified to PostgreSQL simple protocol; no client semicolon
  splitting. Result schemas, rows, and command counts retain wire-result order.
  NULL, empty string, and Unicode remain distinguishable as text values.
- SQL error preserves already collected results and SQLSTATE. A terminal error
  slot (message capped to 1024 Unicode scalar values) survives row truncation.
  Before another
  user command starts, an empty simple-query FIFO barrier confirms readiness
  after a stream error. Barrier failure/5s timeout retires the connection and
  yields OutcomeUnknown. I/O/protocol failure yields OutcomeUnknown too.
- Cancel uses the driver's independent CancelToken path, guarded by exact
  session/generation/execution equality. Stale/queued/wrong-session/wrong-
  generation requests never send CancelRequest. Actor completion and cancel
  share a lease, so a late cancel cannot acquire a new query's lease.
- **Every accepted cancel retires the physical connection**, even when the query
  finishes normally or the cancel request fails. CancelRequest has no success
  acknowledgement and can arrive after query completion; retirement prevents a
  late signal from hitting the next query. This deliberately loses temp tables
  and transaction state. A later explicit bridge Run may establish a new session; queued native Session requests
  receive Rejected. Server SQLSTATE 57014, not token-send success, is Cancelled.
  An independent 5s retirement timer closes the backend even if cancel delivery
  fails or the caller drops its cancel future; it never depends on the busy actor
  returning first. The 3s token-send timeout is separate from that timer.
- Rows are drained after the retained-result cap is reached. Default retained
  payload estimate is 512 KiB and 4096 events; no complete-result accumulation.
  Notices have a separate cap of 16, message length 1024 Unicode scalar values.

## Deliberate production blockers

`CommitPolicy` is independent of the actual `ServerTransactionState`.
`Auto` never inserts transaction control. `Manual` automatically begins the next
ordinary execution only when the last synchronized wire state is Idle. It does
not commit an existing transaction when the client policy changes. Dedicated
`Session::{submit_commit,submit_rollback,commit,rollback}` bypass automatic BEGIN.
A dedicated Commit against a failed transaction is rejected; explicit Rollback
is required. Direct SQL BEGIN/COMMIT/ROLLBACK is still sent unchanged and its
actual resulting state is read from the protocol, never inferred from SQL text.

`ServerTransactionState::{Idle,Active,Failed,Unknown}` maps ReadyForQuery I/T/E
from the workspace's narrow **local tokio-postgres patch**. An initial empty
query obtains actual state before normal requests. A bounded empty FIFO barrier
after user SQL also consumes ReadyForQuery after an error. Missing terminal
readiness or I/O failure retires the connection, reports Unknown, and never
replays SQL or Commit. Every accepted cancel still retires the connection and
resets state to Unknown, even if a final server status was observed. The public
`Session::transaction_state()` getter is the last synchronized status; it is not
a live mid-query transaction inspector. This evidence depends on the vendored
patch, not an unmodified upstream driver release.

Retention limits cover the spike's accounted event scalar/container payload,
not total process RSS, spare Vec capacity, driver buffers, async notices, or one
transient oversized field/row. A giant server value is decoded before it can be
rejected. There is no UI event stream/backpressure, spill, server cursor, bounded
wire frame, query deadline outside the verifier, or lazy LOB support. Truncation
also stops retaining later events (including an error event), while report
outcome and a separate bounded terminal-error slot retain failure/SQLSTATE.
Production still needs event streaming independent of row retention.

Notices arrive through `Connection::poll_message` separately from query results;
relative row/notice ordering and dropped-notice counts are unproven. Text simple
protocol does not provide typed/OID lossless roundtrip evidence. COPY,
general parameterized query/result APIs, typed arrays/JSON/decimal/bytes, pooling, ambiguous COMMIT delivery, edit conflict handling, client certificate identity, and
fault injection around write acknowledgement remain unverified. The disconnect
scenario commits a fixture insert, observes its visibility, then kills the same
backend during a subsequent sleep; it proves retirement/no replay/no later write dispatch;
it does **not** prove an actual ambiguous COMMIT server outcome.

## Assertions emitted in JSON

1. Pinned temp-table continuity and isolation from a second session.
2. Explicit manual uncommitted invisibility, COMMIT visibility, ROLLBACK.
3. Raw BEGIN with Auto policy; external proof of active/failed transaction;
   SQL error drain and ROLLBACK recovery.
4. Two result sets, ordered counts, DO dollar quote / literal semicolon, notice,
   NULL versus empty/Unicode, middle error preserving partial rows.
5. Small byte/event retention cap plus full drain before subsequent query.
6. Cancel query N while N+1 is already running; wrong generation/session denial.
7. Out-of-band cancellation of a server-observed active sleep, SQLSTATE 57014,
   terminal response, retirement, and subsequent query rejection.
8. A committed insert before backend termination during a sleep, OutcomeUnknown,
   exactly one persisted insert, and no automatic replay/later write dispatch.

## API evidence

Primary driver source/API checked for the resolved `tokio-postgres 0.7.18`:
[CancelToken](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.CancelToken.html),
[SimpleQueryStream implementation](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/simple_query.rs.html),
[Client](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.Client.html).
Exact dependencies are in workspace Cargo.lock. tokio-postgres is MIT/Apache-2.0.
The driver exposes no cancel-success acknowledgement. Upstream 0.7.18 consumes
ReadyForQuery without exposing its status byte. The workspace patch in
`vendor/tokio-postgres/src/simple_query.rs` adds `TransactionStatus` and
`SimpleQueryStream::transaction_status()`; state is available after terminal drain.
Its protocol wire tests are maintained alongside the vendored source.

## Observed validation

2026-10-21: PostgreSQL **18.6**, `aarch64-unknown-linux-musl` (Alpine), isolated
loopback Docker fixture provisioned by the coordinator. Rust **1.99.1**.
The JSON CLI passed all eight scenarios; the explicit integration test passed
`1 passed; 0 failed; 0 ignored`. `cargo clippy -p dbide-pg-spike --all-targets --
-D warnings` passed. Missing URL CLI exited **2**, as intended. These are narrow
functional contract measurements; no performance comparison or production
transaction-status/TLS capability is inferred.

## Typed profile connections and TLS extension (2026-10-21)

`Session::connect_request(&ConnectionRequest, Limits)` now builds
`tokio_postgres::Config` from validated fields and supplies the password separately.
A whole-connect timeout includes resolution, transport, TLS and authentication.
`is_closed()` exposes known idle transport loss before SQL dispatch; `close()`
explicitly retires the driver. Existing URL-based `connect` remains a NoTls fixture
compatibility API. Neither API retries SQL.

`VerifyFull` (profile default) and `Require` both select driver `SslMode::Require`
with native-tls certificate-chain and hostname verification. There is no Prefer
mode or insecure verification override. An optional bounded PEM root augments
this connector's system roots only. Cancel uses the same TLS policy as the query
connection. The fixed connection diagnostics expose only safe targets and
SQLSTATE/OS codes, never raw connection errors or credential URIs.

Implementation checked against official [MakeTlsConnector API](https://docs.rs/postgres-native-tls/0.5.3/postgres_native_tls/struct.MakeTlsConnector.html),
[native-tls verification defaults](https://docs.rs/native-tls/0.2.18/native_tls/struct.TlsConnectorBuilder.html),
and [tokio-postgres SSL modes](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/config/enum.SslMode.html).
The separate ignored bridge TLS contract uses only the owned loopback port55440
fixture; rejects its untrusted CA, accepts an explicitly supplied CA, and rejects
plaintext port55439 without fallback. This extends connectivity evidence;
production typed values, COPY, SSH, client cert
identity, and ambiguous commit fault injection remain unimplemented.

## Exclusive parameterized CRUD batches (2026-10-21)

`Session::submit_batch(Vec<ParameterizedStatement>, CommitPolicy)` returns the
same keyed `Execution` as ordinary SQL; `execute_batch` awaits its report.
`ParameterizedStatement` contains `sql`, `parameters: Vec<Option<String>>`, and
`expected_affected_rows: Option<u64>`. Values bind through `ToSql` as wire TEXT;
SQL templates may explicitly cast `$n::text` to their destination types. NULL,
empty text, Unicode, decimal lexemes and SQL-looking text remain values. Both
statement Debug and the driver's parameter Debug view redact values.

One actor request owns the whole batch, so queued ordinary SQL cannot interleave.
Before admission, local PostgreSQL AST validation requires exactly one INSERT,
UPDATE or DELETE per statement, with complete numbered placeholder coverage,
valid text parameters and bounded payload. Transaction control, SET, SELECT,
multiple statements, NUL, missing/unreferenced parameters and excessive budgets
are rejected before any transaction begins. Defaults: at most 256 statements,
4,096 total parameters, 4 MiB total parameter bytes and `Limits::sql_bytes`
(64 KiB by default) aggregate SQL. This is a CRUD executor, not an arbitrary SQL
script transaction sandbox.

Server `prepare_typed` for **all** statements happens inside a new BEGIN or a
unique savepoint before the first mutation. A missing relation/column during
prepare therefore cannot abort unrelated work in an existing transaction.

| Initial wire state | Success | Failure/conflict |
|---|---|---|
| Idle, Auto | BEGIN, statements, acknowledged COMMIT → Idle | Confirmed ROLLBACK → Idle |
| Idle, Manual | BEGIN, statements → Active; caller later commits | Confirmed ROLLBACK → Idle |
| Active, either policy | SAVEPOINT, statements, RELEASE → Active | ROLLBACK TO + RELEASE → Active; earlier work preserved |
| Failed | Rejected; explicit Rollback required | No batch SQL dispatched |

Every statement retains its actual CommandComplete count within result budgets.
Those counts describe attempted statements, including changes subsequently
rolled back. An affected-row mismatch stops the batch and rolls back its scope;
`Outcome::Failed` carries a terminal `Conflict:` application message and no fake
PostgreSQL SQLSTATE. Database failures carry their actual SQLSTATE with a safe
message, excluding raw server text and parameter values. Deferred constraint
failure at COMMIT is also drained and rolled back when readiness can be confirmed.
Rollback failure, transport loss, or missing COMMIT acknowledgement yields
OutcomeUnknown/Unknown and retires the connection. SQL is never replayed.
Cancellation uses the same exact execution lease and always retires the backend.
These guarantees concern PostgreSQL transactional row changes; sequence advances
and external side effects of caller-selected functions/triggers are not undone
by SQL rollback.

Opt-in fixture tests:

```sh
DBIDE_TEST_TLS_CA="$PWD/tmp/tls-fixture/ca.pem" \
  cargo test -p dbide-pg-spike --test batch_contract -- --ignored
```

Five live tests passed against the owned local fixtures: verified TLS bound
CRUD (server TLS independently observed), and conflict and savepoint
recovery/exclusivity/value binding/native cancellation; read-only startup default;
deferred COMMIT failure; and a loopback protocol relay that drops COMMIT's
acknowledgement **after** observing server CommandComplete/ReadyForQuery. In the
last case an independent observer proves exactly one committed row, while the
client reports OutcomeUnknown and refuses subsequent requests on the retired
session. This is actual commit-ack fault injection, not a successful-write claim
inferred from a pre-commit disconnect. The relay handles only the disposable
NoTls fixture and logs no wire data.

## Engine and read-only profile policy

`connect_request` rejects non-PostgreSQL engines before network I/O.
`profile.read_only=true` sets the server startup parameter
`default_transaction_read_only=on`. This is a server session default, **not a
permission boundary**: explicit SQL can turn it off, and PostgreSQL permits some
operations on temporary objects in read-only transactions. Applications requiring
strict write prevention still need read-only database roles and the workflow's
explicit read-only transaction guards. Fixture tests prove ordinary persistent
DML is rejected with SQLSTATE 25006, and that an explicit SET override changes the
default. No user database or stored production credential is used.

## Captured table identity guard

`TableIdentityGuard { schema: String, table: String, oid: u32,
metadata_fingerprint: Option<String> }` is a separate
metadata witness. `Session::submit_guarded_batch(guard, statements, policy)` and
`execute_guarded_batch` preserve the existing unguarded batch API. Callers must
retain the OID captured with their editable table metadata; looking up a fresh OID
only when applying would defeat the protection. The UI must also keep the capture
associated with its original connection/profile.

Guard admission validates positive OID and 1–63 UTF-8 byte identifiers without
NUL. Local AST checks require every mutation target, including writable CTEs,
to match the guard's explicitly schema-qualified relation. Quoted identifier
case/embedded quotes are preserved; unquoted PostgreSQL names fold ASCII case.
The SQL lock identifier is separately escaped with double-quote doubling, while
catalog lookup schema/table names are bound parameters.

Inside the same BEGIN/savepoint as the batch, legacy guards with no metadata
fingerprint acquire ROW EXCLUSIVE. Fingerprinted guards acquire SHARE UPDATE
EXCLUSIVE, which blocks conflicting table metadata DDL/maintenance while allowing
ordinary DML; mutation dispatch subsequently takes its normal ROW EXCLUSIVE lock.
The actor then reads catalog OID and
requires `relkind='r'` plus exact captured OID before preparing or dispatching any
mutation. A same-name/same-columns DROP/CREATE therefore fails closed. An OID or
kind mismatch reports OutcomeFailed with application message
`TableIdentityChanged:` and no fabricated SQLSTATE, and rolls back the local
batch scope. Lock/catalog errors retain their actual SQLSTATE. Success keeps the
lock through batch COMMIT or the caller's outer transaction; savepoint rollback
releases locks acquired by the failed batch while preserving earlier work.
No reconnect or SQL replay is introduced.

A missing metadata fingerprint preserves the legacy OID-only contract. The row
editor must provide Some(fingerprint) from the same repeatable-read capture as
its table metadata/OID. Existing optimistic row predicates and affected-row
validation remain necessary; the metadata guard is not a row-version check.
Views and partitioned parents are not editable through this ordinary-table guard.

The explicitly ignored `guarded_batch_contract` runs only against the owned
loopback fixture. It creates one uniquely named schema, recreates its own target
with identical columns, proves stale-OID rejection without DML, proves valid-OID
success, checks savepoint lock cleanup and outer-transaction lock retention via
`pg_locks`, preserves/commits unrelated earlier work, rejects an updatable view,
and applies safely to a quoted SQL-looking table name. Its final DROP removes
only that generated test schema.


### Shared table-local metadata fingerprint and bounded locking

`table_metadata_query(oid: u32) -> String` and
`table_metadata_fingerprint(canonical_json: &str) -> String` are shared by the
read-only workflow capture and guarded apply, avoiding two drifting catalog
representations. The query is a single SELECT (under 16 KiB) with numeric-only OID
substitution. It returns exactly one TEXT-or-NULL `canonical_json` cell. PostgreSQL
JSONB text supplies canonical object keys; metadata arrays are explicitly ordered.
Raw catalog node trees avoid search_path-dependent `pg_get_*` deparsing. Dynamic
row statistics such as relpages/reltuples/visibility/frozen-XID are excluded.

The witness includes ordered column type/typmod/null/default/generated/identity/
collation metadata, outgoing **and incoming** constraints/actions/deferral,
primary/unique/index validity/expressions/predicates, triggers, rules, RLS,
partition/inheritance, comments, structural dependencies, publication membership
and extended-statistics definitions. Referenced global objects are not frozen.
After its relation lock and OID/kind check, apply executes this same query and
compares SHA-256 before server prepare or mutation. Missing/over-budget metadata
or mismatch yields `TableMetadataChanged:` / Failed and rolls back only the batch
scope. Fingerprints must be exactly 64 lowercase hexadecimal characters.

The SQL returns NULL when canonical metadata exceeds 1 MiB, avoiding transmission
and decoding of an oversized cell. Capture and apply also check the client byte
length. This is a wire/client result bound, not a PostgreSQL JSON construction
memory limit. Capture keeps its read-only repeatable-read transaction open through
catalog/OID/hash and confirmed rollback.

Fingerprinted guards save caller lock_timeout/statement_timeout, temporarily cap
each at the shorter positive existing value or 4 seconds (zero/unlimited becomes
4 seconds), and restore both with bound `set_config` before mutation. Scope
rollback restores settings on failure. The outer client guard deadline is 5
seconds. Either PostgreSQL lock timeout (55P03) or statement timeout (57014) may
fire first at equal caps; both are acknowledged server failures. Shorter caller
limits and unrelated outer-transaction work are preserved. Native cancellation
remains available while waiting for the stronger lock. Ordinary batch DML has no
new automatic deadline. An internal transaction-control client deadline now also
requests native cancellation using the original TLS policy before retiring the
backend, rather than relying on socket close alone.

Owned live tests prove normal INSERT/UPDATE/ANALYZE leave the witness unchanged;
same-OID column, trigger and incoming cascading-FK changes reject before mutation;
fresh metadata succeeds; prior transaction work survives rejection; COMMENT,
CREATE INDEX CONCURRENTLY and VALIDATE metadata work blocks while ordinary DML
continues; oversized metadata produces a tiny NULL response; caller timeout
settings survive success/failure; and server lock waits terminate before the
client returns. A separate delayed deferred-COMMIT test proves the internal client
deadline sends cancellation and the fixture backend disappears instead of
continuing its sleep.

This is a guard for the selected table's captured metadata, not global catalog
immutability or a server-side effect sandbox. ALTER FUNCTION, global collation or
role changes are outside its table locks. DEFAULT is evaluated by PostgreSQL and
can involve functions or sequence state; its expression fingerprint does not
freeze all referenced behavior. The row editor retains its existing read-only
rules for unsupported triggers/cascades/metadata and requires explicit submission.

### Read Committed gate and response-loss deadlines

Every guarded batch, including the legacy OID-only variant, checks actual
`transaction_isolation` inside its BEGIN/savepoint. Only Read Committed is
accepted. Repeatable Read/Serializable can retain an **old catalog snapshot**
even after acquiring the stronger relation lock, while execution uses a newer
relcache definition. Such batches return Rejected / `UnsupportedGuardIsolation:`
before mutation. Savepoint rollback preserves caller work; a newly created scope
rolls back to Idle. Session isolation defaults are never silently changed.
Read-only metadata capture intentionally still uses Repeatable Read to obtain
one consistent capture; apply requires a fresh Read Committed view.

Owned tests establish an RR/Serializable snapshot, install a new trigger from a
second session, prove the old snapshot still produces the old metadata hash,
then prove guarded apply rejects without firing that trigger. Existing caller
work/isolation remains usable; default-RR/Serializable new scopes also reject.

Isolation, timeout setup, LOCK, identity query, metadata query and timeout restore
all use bounded client waits with native cancellation before retirement on lost
responses. An owned protocol relay forwards LOCK's CommandComplete and
ReadyForQuery, then withholds the following identity Parse response. The client
returns OutcomeUnknown/Unknown after its five-second read deadline and bounded
(up to three-second) native cancellation attempt, retires the connection
and dispatches no mutation. This specifically covers the formerly unbounded
identity-query await, rather than only server-side lock_timeout.

### BLOCKED M2 acceptance: namespace rebinding

**The guarded write executor is experimental and is not production row-editor
Apply acceptance. Apply must remain disabled until the remaining bound-portal
contract is validated. M0/M1 connection/explorer/query work does not depend on
this advanced write feature.**

An owned-fixture probe verified a remaining name-binding race: a relation lock
protects the old table, but does not prevent another session renaming its schema
and creating a replacement at the original qualified name. Preparing afterward
can target the new table. Even a statement prepared beforehand can rebind after
DISCARD PLANS or search_path invalidation, while client Statement column metadata
continues reporting the old OID. Thus relation lock + catalog hash + ordinary
prepared-statement metadata does **not** establish an atomic namespace-binding
guarantee.

Local probe evidence is preserved under `tmp/schema-binding-probe/`, including
`results.jsonl` and `results-portal-describe.jsonl` (ignored scratch evidence,
not published source or production driver changes). The bound-portal probe found
that Describe **Portal** reflected the actual bound target when stale Statement
metadata did not, and executing the same already-bound portal retained its
original target across the tested namespace changes. This is a candidate design,
not an implemented production guarantee.

Smallest next acceptance work: Bind a named portal, obtain fresh Describe Portal
metadata without Execute, validate actual target identity, then execute that exact
portal without rebinding. Non-returning DML needs a suitable validated projection.
Test schema rename/replacement before and after Bind, plan invalidation, view/rule/
INSTEAD-trigger cases, origin-versus-modification-target ambiguity, cancellation,
rollback and absence of unintended trigger effects before enabling Apply.
No global namespace/catalog lock or broad portal patch was added in this stage.

## M0 COPY risk experiment — 2026-10-22

`tests/copy_contract.rs` adds two explicitly ignored, fixture-only tests. They
were run against the owned loopback PostgreSQL 18.6 fixture; **2 passed, 0 failed,
0 ignored** (0.21s). No Session/bridge/UI transfer implementation was added.

The ordinary SQL console's `Session` path does **not support COPY transfer**:

- `COPY (SELECT generate_series(1,3)) TO STDOUT` terminates within three seconds
  with the existing generic terminal error, OutcomeUnknown, connection retirement
  and Unknown transaction state. No COPY row is exposed as a normal result row.
- `COPY copy_m0_input(v) FROM STDIN`, against a session-owned temporary table,
  similarly terminates/retire instead of waiting indefinitely for stdin bytes.
- An independent observer confirms each old backend disappears within two seconds;
  the retired Session rejects another query. A new connection executes SELECT42
  normally. No COPY statement is automatically replayed.

OutcomeUnknown is the current conservative protocol-failure classification,
not evidence that arbitrary COPY SQL has no server-side effects. Retirement loses
that connection's temporary objects/transaction state. This is bounded unsupported
console behavior, not successful import/export support.

A separate direct `tokio-postgres::Client::copy_out` candidate probe drains exactly
`1\n2\n3\n` (six bytes, with a 1 KiB cap and three-second deadline) and successfully
runs SELECT42 on the same native connection afterward. That demonstrates a driver
candidate API capability only. It provides no GUI COPY flow, COPY FROM upload,
file handling, progress, cancellation contract, binary format, large-transfer
backpressure or end-to-end product acceptance.

Reproduce only after starting the owned disposable fixture:

```sh
cargo test -p dbide-pg-spike --test copy_contract -- --ignored
cargo clippy -p dbide-pg-spike --test copy_contract -- -D warnings
```

Default workspace tests visibly ignore both database experiments. Full COPY
support remains an advanced feature; its absence does not block completion of
M0/M1 connection, catalog browsing and ordinary SQL query work.
