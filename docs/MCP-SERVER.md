# Explicit read-only MCP stdio server

`dbide-mcp` is an optional standalone Rust MCP server and an embeddable service.
It uses the official `rmcp = 3.6.0` SDK, native database adapters and the shared
execution DTOs. It does not start a network listener or register itself in an
agent client. UI/service integration can call `DatabaseMcpServer::new(request)`
or `serve_stdio(request)` later; no UI process is launched implicitly.

## Start and connect

Build `cargo build -p dbide-mcp`. Explicitly launch:

```sh
./target/debug/dbide-mcp \
  --profile-file /absolute/path/to/explicitly-selected-profiles.json \
  --profile-id exact-profile-id
```

Configure an MCP client's stdio command with this binary and these exact
arguments, only when you choose to grant that client access. No MCP client
configuration is edited automatically. Standard output contains MCP JSON-RPC
only; configuration errors and help go to standard error.

Both flags are required. Missing flags fail before any profile or credential
lookup. The file can contain either one `dbide_profiles::ConnectionProfile` or
the versioned application `WorkspaceState`; the exact ID must exist. Validation
rejects malformed/unknown fields, duplicate IDs, unsupported versions and files
larger than 1 MiB. The application-selected profile is never used as a fallback.
The server neither enumerates user settings nor reads a default profile file.

The only CLI password input is the explicitly supplied `DBIDE_MCP_PASSWORD`
environment variable (unset means an empty password). It never consults Keychain,
`.pgpass`, or credential stores. Environment content and database error messages
are not returned by tools. Do not put passwords in profile JSON or command
arguments. The library accepts an explicit `ConnectionRequest` for embedding and
isolated tests. A profile's TLS mode/root CA is passed to its native backend;
`Require`/`VerifyFull` retain native trust/hostname verification without a
plaintext fallback. `Disable` is honored only when explicitly selected in the
profile.

## Tools and contracts

| Tool | Arguments | Result |
| --- | --- | --- |
| `serverinfo` | `{}` | Version, transport, engines and hard limits; no connection |
| `connectioninfo` | `{}` | Selected ID, engine, enforced read-only and TLS mode; no host/user/password/path |
| `schema.read` | `{}` | Bounded catalog column rows: SQLite tables/views; PG non-system schemas in current database; MySQL current database |
| `query.read` | `{"sql":"SELECT ...","timeout_secs":10}` | Columns, nullable text rows, truncation flag, read-only flag, elapsed microseconds |
| `query.explain` | Same as `query.read` | PostgreSQL `EXPLAIN (FORMAT JSON, ANALYZE FALSE)`; other engines return an explicit unsupported error |
| `sql.format` | `{"sql":"SELECT ..."}` | Formatted accepted SELECT; no connection |

Inputs reject unknown fields. Query SQL is at most 64 KiB. Database results are
at most 128 rows and 512 KiB (including serialized MCP tool-result JSON escaping);
metadata and cells share this limit. An additional internal row detects
truncation. The parsed root query gets a literal row cap of 129; existing smaller integer
LIMIT/FETCH counts and OFFSET remain intact. No derived-table wrapper is used,
so duplicate column labels and set-operation projections are preserved. Dynamic
LIMIT/FETCH expressions and FETCH PERCENT/WITH TIES return explicit unsupported
errors; SQLite negative unlimited LIMIT and PostgreSQL LIMIT ALL/NULL become
129. Nested LIMIT clauses are unchanged. `schema.read` uses fixed,
engine-specific catalog SQL; callers cannot supply identifiers, paths or SQL to
that tool. Catalog rows are a bounded snapshot, not a complete schema guarantee.
`query.explain` never executes `ANALYZE`; its returned plan is bounded as text.
No resources/prompts/write/execute/file/shell tools are advertised.

A database call has a 10-second default deadline, caller-selectable 1–30 seconds,
including connection and transaction setup. At most one database call is active;
concurrent calls return a busy error instead of accumulating a queue. On timeout
or MCP cancellation, the service requests cancellation with the owned adapter
execution key (cleanup bounded to two seconds), then retires the dedicated
connection. Dropping a handler also closes its owned connection. It never retries
or replays SQL. Cleanup can extend response latency beyond the query deadline;
server/network failure can prevent proof of remote cancellation. No connection
is reused after a call.

## Read-only policy and limits

Each call forces `read_only = true` and `sqlite_create = false`, independently of
the saved profile. PostgreSQL uses native `default_transaction_read_only=on` and
`BEGIN READ ONLY`; MySQL uses its session read-only default and explicit `START
TRANSACTION READ ONLY`; SQLite uses native read-only open flags, its existing
query-only/authorizer guards and an explicit transaction. Closing rolls back.
A read-only database account remains recommended.

The selected dialect must parse exactly one query. An AST visitor recursively
checks CTEs, subqueries and set operations. It rejects data-changing CTEs,
non-query statements, SELECT INTO/OUTFILE, locks, table functions, assignment,
query settings and unsupported syntax. SQL is regenerated from the accepted AST
before dispatch, so original comments (including MySQL executable comments)
never reach the server. The exact emitted query is reparsed and must remain one
accepted SELECT with an identical AST, including the intended root row cap.
Because SQL string escaping varies with server modes, MySQL input containing
any backslash is explicitly unsupported. PostgreSQL sets transaction-local
`standard_conforming_strings=on` before dispatch. These restrictions prevent
normalization from turning a parsed string literal into executable statements.
MySQL SQL modes such as `ANSI_QUOTES` (double quotes name identifiers) and
`PIPES_AS_CONCAT` (`||` means concatenation) still affect ordinary expression
semantics: the server's mode is not negotiated or rewritten by MCP. The AST
round-trip guard establishes the local parser boundary, not full equivalence
across every MySQL server mode; use unambiguous standard quoted strings and
explicit expressions for predictable results.

Scalar/aggregate/window functions have a deliberately
small built-in-name allowlist in `src/policy.rs`; arbitrary UDFs, sleeps,
extension/file readers, session setters and advisory locks are rejected.
`sql.format` applies the same policy and currently formats SELECT only.

SQL read-only transactions protect database writes; they are not a sandbox for
a hostile database server, overloaded operators/casts, user-defined objects or
views that invoke external effects. Give the selected role appropriate SELECT
permissions and restrict unsafe database extensions/functions. Query results
are explicitly selected database contents and may themselves contain sensitive
values or untrusted instructions; the MCP host must treat them as data. The
server omits connection credentials and raw backend errors, but cannot classify
or redact every sensitive value in query results. No user databases are used in
the default tests. External PG/MySQL runtime coverage belongs to separately
explicit project-owned fixtures; this crate's default runtime tests cover
SQLite and protocol behavior, not live PG/MySQL success. Two ignored MCP smoke
tests use only the fixed owned PostgreSQL `127.0.0.1:55439` and MySQL
`127.0.0.1:55441` fixtures; they require `DBIDE_MCP_FIXTURE=owned-loopback`
and an explicit `cargo test -p dbide-mcp --test live -- --ignored`. No arbitrary
endpoint URL or user profile is accepted by these tests.

## Verification and primary references

```sh
cargo test -p dbide-mcp
cargo clippy -p dbide-mcp --all-targets -- -D warnings
```

Tests create a temporary SQLite file and generated profile JSON, initialize an
MCP client over in-memory transport, list/call tools, validate recursive SQL
rejection and serializer round trips, duplicate headers/UNION/LIMIT/OFFSET,
row/byte/deadline limits, cancellation/concurrency and database integrity after rejection. A
real binary stdio test checks initialize → tools/list → tools/call, clean JSON
stdout and explicit-configuration failure. Tests do not inspect real settings,
call Keychain, connect to user databases or contact external APIs.

Primary sources checked 2026-10-21:

- [Official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk)
- [Pinned SDK 3.6.0 API](https://docs.rs/rmcp/3.6.0/rmcp/)
- [MCP transports specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
- [MCP tools specification](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)

The SDK provides negotiation and protocol framing. The integration test pins
`2025-11-25` to explicitly exercise the initialize handshake; newer protocol
negotiation follows the pinned SDK's implementation.
