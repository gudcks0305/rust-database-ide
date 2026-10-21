# Offline schema, migration, ER and EXPLAIN tools

2026-10-21. `dbide-schema` is a reusable Rust library and offline CLI. These are
implemented tools for parts of F27/F28/F30/F31/F33, not a declaration of M4 or
DataGrip parity. It contains no driver, connection, migration executor or
EXPLAIN ANALYZE execution path. Live catalog integration belongs to the adapter.

## Snapshot contract

`SchemaSnapshot` is a versioned serde DTO with database identity, capture
provenance, explicit authoritative schema scope, completeness, schemas, tables,
columns, PK/FK/index metadata, views and unsupported-object markers. Column
metadata includes type, NULL, default and generated/identity markers. FK metadata
includes ordered column pairs, qualified target, actions and deferral. Index
metadata preserves expressions, predicate, INCLUDE and constraint ownership even
when SQL generation does not support them. JSON input is capped at 32 MB. JSON import/export preserves these
fields and rejects unknown version/fields and invalid identities/duplicates.

`database.identity` must be a stable credential-free connection/database ID.
`Provenance::Live` carries a capture timestamp and generation. Offline/imported
captures are explicitly distinct. `complete` means the adapter captured all
represented and unsupported objects inside `scope`, rather than merely fetching
some rows successfully. Do not set it for a filtered, cancelled or capped catalog.
An authoritative scope includes intentionally absent schemas, enabling an explicit
schema-create/drop comparison. Source and target scopes must match.

Adapters must mark partitions, inheritance, check/unique/exclusion constraints,
identity/generated details, RLS, grants, triggers, routines, sequences, domains,
custom options and other unrepresented semantics in `unsupported_features` or
`unsupported_objects`. Dropping such metadata silently is a contract violation.
Names are exact case-sensitive strings. Ordinary parser identifiers are folded
using PostgreSQL ASCII lowercasing; quoted Unicode identifiers retain case.
Identifiers over the standard PostgreSQL 63-byte limit fail instead of silently
producing truncated/colliding migration names. Custom server identifier limits
are not negotiated.

## Direction and migration review

```rust
use dbide_schema::diff;
# fn example(desired: &dbide_schema::SchemaSnapshot, actual: &dbide_schema::SchemaSnapshot) -> Result<(), dbide_schema::SchemaError> {
let plan = diff(desired, actual)?;
// source_desired -> target_actual: statements change ACTUAL to match DESIRED.
let sql_preview = plan.preview_sql();
plan.verify_against(desired, actual)?;
# let _ = sql_preview;
# Ok(())
# }
```

Plans (format version 2) preserve both database identities and semantic SHA-256
fingerprints of schema content, scope and completeness. `content_fingerprint()`
excludes capture time/generation/offline labels so a fresh unchanged capture can
validate a preview. `fingerprint()` includes provenance as a separate capture
evidence hash; the plan retains original source/target capture hashes as well.
`verify_against` checks both semantic fingerprints and recomputes the complete
plan, rejecting modified SQL/operations and unsupported changes. `verify_target`
is only a stale-target content check, not approval. Fresh recapture with the same
schema/database/scope passes; structural or database-identity changes fail.
Fingerprints are not signatures, and a local snapshot does not prove remote
freshness. A future executor must obtain fresh authoritative metadata immediately
before execution, keep the target connection bound, and handle the race between
validation and DDL with an explicit locking/transaction policy.

Supported preview operations: ordinary schema/table create/drop; append-column,
drop-column; SET/DROP NOT NULL; SET/DROP DEFAULT; basic PK/FK changes; plain
btree column indexes. Type changes, column reorder/insertion before an existing
column, view modification, partial/expression/include indexes, and unmodeled
features are explicit unsupported operations with no SQL. Drops and tightened
NULL constraints carry destructive flags; data checks and unique/FK additions
carry `requires_data_validation`. No rename is guessed. A renamed object appears
as drop/create and must be reviewed as possible data loss. An absent desired
constraint name means the server-generated target name is acceptable; an explicit
desired name participates in comparison.

Ordering is deterministic across equivalent catalog collection orders (column/key
column order remains semantic): schemas, detach target FKs,
drop affected indexes/tables/PKs/columns, create tables/append columns/alter
columns, add PKs/indexes, restore desired FKs, drop empty schemas. All tables and
keys exist before FK additions, so mutual/self-referential cycles work without
CASCADE. All actual FKs are detached/restored when any table shape, FK or non-PK index changes;
this is deliberately conservative and can take more locks than a minimal plan.
An actual constraint must have a stable name before it can be detached. FKs with
out-of-scope/unresolved/non-unique target keys are blocked. View metadata blocks
table shape changes until a dependency-aware view planner exists. Unmodeled
schema-object removal, table/index changes with unmodeled schema dependencies,
and unique/exclusion constraint indexes also block the plan.

SQL always quotes PostgreSQL identifiers and uses schema-qualified table/drop
index names. Types/defaults are parsed as a single type/expression before preview;
additional statements/column clauses are rejected. This does not certify that a
function/default is safe or exists. Preview SQL never grants implicit permission
to run it. `implicit_commit=false` describes ordinary PostgreSQL DDL, not a claim
of all-or-nothing execution. Locks, casts/backfills, default volatility, external
dependencies, permissions, transaction failure, concurrent changes and rollback
remain executor responsibilities. No CASCADE, inferred USING casts or concurrent
index DDL are generated. Unsupported plans must never be partially applied by
filtering out unsupported operations.

## Offline DDL sources

`ddl::import_postgres_ddl(sql, database, default_schema, label)` accepts ordinary
CREATE SCHEMA, CREATE TABLE, simple column PK/REFERENCES and table PK/FK,
plain btree CREATE INDEX, and ALTER TABLE ADD FOREIGN KEY. FK actions/deferral,
NULL/default expressions, qualified/quoted names and Unicode survive round trips.
An explicit default schema replaces ambient `search_path` assumptions. CREATE
TABLE/INDEX references in ALTER/INDEX must already be defined. Import is atomic:
one unsupported statement/clause rejects the entire snapshot, including CHECK,
UNIQUE, generated/identity/SERIAL, view, partition, table options, schema
authorization/IF NOT EXISTS, SET search_path, expression/ordered index and unknown
DDL. SQL comments are accepted by the parser and are not catalog metadata.

`export_postgres_ddl` emits schemas/tables, basic indexes, then ALTER FK statements
so cyclic references are valid. It rejects metadata it cannot reproduce; JSON is
the complete interchange format. It is a reproducible subset, not a PostgreSQL
pg_dump replacement. DDL input is capped at 16 MB. Export does not connect or run.

## ER and EXPLAIN

`er::graph` builds deterministic nodes/column IDs and FK edges with paired column
IDs. Graph edges are actual FKs, not guessed semantic relationships. Names use
length-delimited qualified identities, avoiding dotted-name collisions. Missing
or capped targets/column endpoints are explicit. `er::export_svg` produces a
standalone escaped SVG with a deterministic grid layout and visible truncation
notice; self references have loop paths. Default caps: 200 tables, 40 columns per
table, 1,000 edges, 2 MB SVG. Hard maxima are 2,000/500/10,000/20 MB. Overflow
returns an error or explicit truncation rather than unbounded layout. This is
basic export, without interactive layout, manual relations, cancellation or
large-schema routing quality guarantees.

`explain::parse_postgres_explain(json, ExecutionMode)` parses PostgreSQL FORMAT
JSON into a typed plan tree, costs, row estimates, actual metrics, planning and
execution timings. Raw statement/node properties preserve unknown fields,
triggers, JIT, buffers and future server metadata. Input is capped at 16 MB,
100 statements, 10,000 nodes and 64 typed tree levels (serde also has its default
recursion guard). Invalid field shapes fail rather than being coerced.

Caller mode is `PlanOnly`, `AnalyzeExecuted` or `Unknown`. Actual metrics are
tracked separately. Conflicting plan-only metadata produces a warning; absent
metrics never prove ANALYZE was not executed. ANALYZE mode explicitly warns that
the statement was executed and could have side effects. The parser cannot run
ANALYZE or infer execution authorization. Other engines and explain graph UI are
not implemented by this crate.

## CLI and verification

Every output path must be new; the CLI refuses to overwrite an existing file.
No command accepts connection credentials or performs database writes.

```sh
cargo run -p dbide-schema -- ddl-import crates/dbide-schema/fixtures/shop.sql output.json public
cargo run -p dbide-schema -- ddl-export output.json roundtrip.sql
cargo run -p dbide-schema -- diff desired.json actual.json plan.json
cargo run -p dbide-schema -- er-svg output.json schema.svg
cargo run -p dbide-schema -- explain crates/dbide-schema/fixtures/explain.json plan-tree.json plan
cargo test -p dbide-schema
cargo clippy -p dbide-schema --all-targets -- -D warnings
```

The contract suite covers exact quoted/injection-shaped SQL, Unicode/schema
identity, parser statement-smuggling rejection, cyclic dependency order,
idempotence, stale/tampered plan rejection, destructive/additive changes,
unsupported conversion/metadata, strict DDL/JSON round trips, SVG escaping and
budgets, EXPLAIN trees/unknown fields and caller execution mode. These unit tests
are offline. Live PostgreSQL application/recapture tests are separate evidence;
no live DB support claim follows from this suite alone.
