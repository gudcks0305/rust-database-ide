# MySQL native adapter

`crates/dbide-mysql` implements a dedicated MySQL text-protocol connection behind
an async actor. It uses `mysql_async = "=0.37.1"` with `minimal-rust` and
`native-tls-tls`; it has no JVM, pool, automatic reconnect, or SQL replay.
Version and API contract were checked against `cargo info mysql_async` and the
published 0.37.1 documentation on 2026-10-21.

## Contract

- `Session::connect_request(&ConnectionRequest, Limits)` validates a typed MySQL
  profile. No interpolated credential URL, saved-profile discovery, or logging of
  authentication errors. Password remains separate from the persisted profile.
- `submit(sql, policy)` immediately returns an `Execution` with an `ExecutionKey`;
  `finish().await` returns the shared `ExecutionReport`. `execute` combines both.
- `submit_commit`, `submit_rollback`, `commit`, `rollback`, `transaction_state`,
  `is_running`, `is_closed`, `cancel`, and `close` match the PG adapter's surface.
- One actor owns one physical `Conn`. Temporary tables, user variables, session
  settings and transaction context survive executions. Actor requests are FIFO;
  queue capacity is 16. Network/protocol failure retires the connection and rejects
  queued work. A later connection requires an explicit new connect operation.
- Raw SQL is sent once through `COM_QUERY`. MySQL parses multiple statements;
  semicolons inside literals are never split by the adapter. Results retain set
  boundaries, column names, rows and each command's affected-row count. Earlier
  rows remain in the report if a later statement fails. Each successful SELECT
  ends with its own `CommandComplete` carrying the observed wire row count,
  including rows discarded after the retention limit. A following DML update
  count is a separate event. Failed result sets do not get a success terminal.
  `drop_result()` must be
  called even when `is_empty()` is true: version 0.37.1 can cache a later-result
  server error that `is_empty()` alone does not expose.
- Text values preserve SQL NULL versus empty strings, Unicode and decimal digits
  without converting through floating point. Binary string/blob/BIT/geometry
  types use `0x` hex; invalid UTF-8 also uses hex. Numeric text is not mistaken
  for binary merely because its column has binary charset 63. This is a safe
  display representation, not a typed editing/export API.

## Transactions and cancellation

The adapter reads the native OK/EOF status already delivered by successful
user commands through public `Conn::last_ok_packet().status_flags()`, testing
`SERVER_STATUS_IN_TRANS`. It sends no diagnostic SQL or ping after executions:
both `DO 0` and `COM_PING` overwrite MySQL `ROW_COUNT()`. Keeping the wire passive
also preserves `SHOW WARNINGS` and error diagnostics across separate executions.
No arbitrary submitted SQL text or UI policy is used to infer transaction state.

MySQL ERR packets contain no transaction flags, and the driver clears its last
OK packet on ERR. The adapter therefore reports Unknown after SQL errors rather
than guessing whether a transaction survived (a deadlock can roll it back).
Subsequent Manual ordinary SQL is rejected without touching diagnostics until
an explicit Commit/Rollback returns fresh native status. An explicitly submitted
Auto diagnostic query can also return actual status through its own native EOF;
Auto policy itself does not commit an already active transaction.

Manual ordinary execution sends `START TRANSACTION` only when the preceding
observed state is Idle. Existing Active transactions are preserved. Explicit
Commit/Rollback controls never start a new transaction. Unknown state blocks
manual dispatch. MySQL statement failures do not imply PostgreSQL's Failed
transaction state; ERR reports Unknown until a later user command supplies a
fresh wire flag. MySQL
implicit DDL commits are reported as Idle even when policy remains Manual.

Cancellation matches the complete active execution key, then marks that target
for permanent retirement. A separate connection issues `KILL QUERY` only for
the actor's captured, owned `Conn::id()`. The active lease keeps the physical
target alive until that control attempt terminates, preventing thread-ID reuse
or a late cancel from hitting execution N+1. Caller cancellation cannot drop the
control task or clear its retirement decision. Accepted cancellation interrupts
actor I/O, rejects queued work and retires the target even if the control attempt
fails. Because cancellation can race completion/write acknowledgement, its
report can be `OutcomeUnknown`; `RequestedConnectionWillRetire` confirms the
control statement succeeded, not that every write was rolled back.

An execution deadline is 300 seconds including draining.
The cancellation control path has a separate five-second deadline. Deadline or
transport failure after dispatch reports `OutcomeUnknown`, retains any earlier
rows, and retires the connection. No retry or write replay occurs.

## Security and limits

`VerifyFull` (profile default) and `Require` both require TLS with native platform
trust plus hostname verification; neither mode downgrades to plaintext or enables
invalid-certificate/domain bypass flags. `root_cert_path` adds an explicitly
selected PEM/DER root bundle (regular file, at most 1 MiB at initial validation).
`Disable` is an explicit plaintext choice used by isolated loopback fixtures.
TCP is forced with `prefer_socket(false)` so a local socket cannot silently
replace the selected host/port or TLS transport.

`read_only` applies `SET SESSION TRANSACTION READ ONLY`. It is a server session
default, **not an authorization boundary**: SQL-capable users can change session
settings, MySQL allows some temporary-table operations, and account privileges
remain the actual security boundary. Use a restricted server account for an
access guarantee.

Profile timeout bounds connection establishment plus setup; it is distinct from
the execution deadline. `Limits.sql_bytes` rejects oversized input before
queueing. Event count and retained payload byte estimates are bounded; extra rows
are drained without retention and a bounded terminal error survives a full row
budget. Driver packet buffers and one in-flight server row are outside the
retained-report budget. The adapter does not claim a process-memory cap, prepared
parameter editing, streaming LOBs, metadata browsing, warning/notice delivery,
SSH tunneling, or complete MySQL/MariaDB compatibility.

## Reproducible fixture verification

Never use real user databases or profiles. Parent fixture tooling owns the
MySQL 8.4 Docker server on `127.0.0.1:55441`, database `dbide_fixture`, username
`root`, password `dbide_fixture_only`. All live tests are visibly ignored in a
normal test run and require an explicit fixture opt-in:

```sh
cargo test -p dbide-mysql
DBIDE_MYSQL_FIXTURE=owned-loopback DBIDE_MYSQL_FIXTURE_CA="$PWD/tmp/tls-fixture/ca.pem" cargo test -p dbide-mysql --test live -- --ignored --test-threads=1
cargo clippy -p dbide-mysql --all-targets -- -D warnings
```

Measured on 2026-10-21 against the owned MySQL 8.4.11 image
`sha256:0f51e63e537a1f176423fa03ef559aabe6fa4372a53160a44bc7e75da7d15364`:
three unit tests and thirteen explicitly invoked live tests passed. Live gates cover
fixed connection identity, temp table/user variables, separate-call ROW_COUNT,
warning/error diagnostics, SELECT/DML event boundaries, cross-session commit and
rollback visibility, actual state after implicit DDL commit, raw multiple results,
affected counts, NULL/empty/decimal/Unicode/binary values, rows before later error,
bounded retention plus drain/reuse, stale cancel, accepted cancel retirement,
queued rejection, independent-session safety, dead connection without reconnect,
read-only session default (and a privileged SQL user's ability to change that
default), and rejection of the fixture's untrusted TLS cert. TLS follow-up uses
the owned CA-signed server certificate with SAN `localhost` and `127.0.0.1`:
both VerifyFull and Require successfully connected using the explicit CA and
reported a nonempty `Ssl_cipher` for DNS and IP destinations. An unrelated valid
CA was rejected. A test-only driver TLS hostname override to `wrong.invalid`
was rejected with the correct CA and physical TCP destination still loopback.
Production options have no TLS hostname override and keep both certificate and
hostname validation enabled. The public unrelated-CA PEM under the crate's test
fixtures contains no private key and serves only this negative trust test.

Primary sources:

- [mysql_async 0.37.1](https://docs.rs/mysql_async/0.37.1/mysql_async/)
- [Conn public API](https://docs.rs/mysql_async/0.37.1/mysql_async/struct.Conn.html)
- [SslOpts](https://docs.rs/mysql_async/0.37.1/mysql_async/struct.SslOpts.html)
- [QueryResult](https://docs.rs/mysql_async/0.37.1/mysql_async/struct.QueryResult.html)
- [MySQL KILL](https://dev.mysql.com/doc/refman/8.4/en/kill.html)
- [MySQL transaction characteristics](https://dev.mysql.com/doc/refman/8.4/en/set-transaction.html)
