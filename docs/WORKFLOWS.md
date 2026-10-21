# Read-only PostgreSQL workflows

2026-10-21. `dbide-workflows` connects existing profile/driver/schema crates for
actual catalog capture and plan-only EXPLAIN. It is a service library, not a
migration executor or a claim of complete M4 support.

```rust
use dbide_workflows::{capture_schema, compare_snapshots, explain_plan, CaptureLimits};
# async fn example(request: &dbide_profiles::ConnectionRequest) -> Result<(), dbide_workflows::WorkflowError> {
let scope = vec!["app".to_owned()];
let actual = capture_schema(request, &scope, CaptureLimits::default()).await?;
let unchanged = compare_snapshots(&actual, &actual)?;
let explain = explain_plan(request, "SELECT 1", false, CaptureLimits::default()).await?;
# let _ = (unchanged, explain);
# Ok(())
# }
```

## Session, safety and completeness

Every operation creates a dedicated `dbide_pg_spike::Session` using the supplied
`ConnectionRequest`. TLS/authentication remain the profile/driver's verified
connection contract. No caller console session, open transaction, temp object or
credentials are reused in catalog data. The connection is retired by a drop guard
on success, error, timeout or cancellation of the workflow future. Connection
errors use the driver's credential-safe diagnostics; SQL/server error text is
not returned. Query failures expose SQLSTATE only.

Catalog statements execute in one `REPEATABLE READ READ ONLY` transaction, then
ROLLBACK. Server `statement_timeout`, `lock_timeout` and a client timeout bound
work. Names in predicates use escaped PostgreSQL `E''` literals; backslash and
quote escaping remains correct independently of initial
`standard_conforming_strings`. No schema identifier or caller SQL is interpolated
into executable catalog statements. Built-in catalogs and functions are qualified
or resolved under local `search_path=pg_catalog`.

Scope must explicitly contain 1–32 distinct user schemas, each 1–63 UTF-8 bytes,
with no NUL. System schemas (`pg_*`, `information_schema`) are rejected. An absent
schema remains an authoritative absent entry in scope, which can support a
reviewed create-schema preview. Only PostgreSQL 14+ is accepted; live evidence
currently covers PostgreSQL 18.6. Older versions are unsupported rather than
assumed compatible. Catalog permissions are required; permission errors return
no snapshot. Credentials are not part of the stable database fingerprint.

Default limits: 20,000 metadata records, 500 relations, 10,000 columns, 8 MB retained
result data, 15 seconds. Hard maxima: 50,000 records, 2,000 relations, 30,000
columns, 32 MB, 120 seconds. Each catalog query fetches at most record-cap+1, and
the complete script also has a shared driver event/byte cap. Any cap, truncated
report, failed statement, missing/invalid identity or malformed reference returns
an error. A partial snapshot is never returned as `complete=true`.

Catalog reads never run DDL/DML or the optional fixture setup script. The explicit
transaction/SET/ROLLBACK commands only configure this dedicated connection.

## Captured schema semantics

The adapter builds the reusable `SchemaSnapshot` model with exact qualified
identities, database/version, schema scope and live provenance. It captures:

- Ordinary/partitioned/foreign table identities, views/materialized definitions.
- Columns in catalog ordinal order, type, NULL/default and identity/generated markers.
- Named composite PK/FK column order, qualified references, actions and deferral.
- Index name, uniqueness, access method, key columns, expressions, predicate,
  INCLUDE, constraint ownership and unsupported option markers.

Types and defaults are canonicalized with the schema crate parser, keeping its
SQL generation comparison consistent. Unsupported type/expression syntax rejects
the snapshot; it is not coerced to text or dropped.

The snapshot preserves unsupported markers for partitions/inheritance, foreign or
nonpersistent tables, RLS, triggers, storage/access/tablespace options, replica
identity, explicit privileges/ownership/comments/collation, CHECK/UNIQUE/exclusion
and advanced/unvalidated constraints, identity/generated columns, advanced PK
backing indexes, index sort/null/opclass/storage/collation/NULLS NOT DISTINCT,
incoming out-of-scope FKs, dependent out-of-scope views/routines, publication
membership/statistics, custom column types and extension-owned objects. Sequences,
routines, user types/extensions, schema/default privileges/comments, collations,
conversions/operators/classes/families and text-search objects remain explicit
schema-level unsupported objects. They are metadata, not silently supported DDL.

Catalog markers are exposed as stable constants in
`dbide_workflows::metadata_features` for consumers that need an explicit CRUD
allowlist. The former combined identity/comment/collation and constraint marker
strings are no longer emitted. Refresh older snapshots; unknown/legacy markers
must not be assumed safe. Exact groups:

| Metadata | Constants | Meaning |
|---|---|---|
| Generated values | `COLUMN_IDENTITY`, `COLUMN_GENERATED` | Separate presence flags; `Column.generated` retains `identity:a`/`identity:d` or `generated:s`/`generated:v` plus expression. |
| Other column attributes | `COLUMN_PRIVILEGES`, `COLUMN_COMMENT` | Attribute presence, independent of generation or equality semantics. |
| Column equality | `COLUMN_CUSTOM_COLLATION`, `COLUMN_NONDETERMINISTIC_COLLATION` | Collation differs from type default, and/or the actual collation is nondeterministic. These are independent checks. A deterministic custom collation emits only the first flag. |
| Constraint kind | `CONSTRAINT_CHECK`, `CONSTRAINT_UNIQUE`, `CONSTRAINT_EXCLUSION`, `CONSTRAINT_NOT_NULL`, `CONSTRAINT_UNKNOWN_KIND` | Kind is preserved separately from validation/enforcement/deferral. PostgreSQL 18 NOT NULL catalog constraints are identified explicitly. |
| Constraint state | `CONSTRAINT_UNVALIDATED`, `CONSTRAINT_NOT_ENFORCED`, `CONSTRAINT_DEFERRABLE` | State is not conflated with an ordinary CHECK/UNIQUE. The deferrable marker covers non-PK/non-FK constraints; FK deferral already has typed fields. |
| Primary key semantics | `PRIMARY_KEY_DEFERRABLE`, `PRIMARY_KEY_INCLUDE_COLUMNS` | PK deferral can affect uniqueness/row-identity assumptions. INCLUDE is a distinct DDL feature. |
| Other constraint options | `FOREIGN_KEY_NON_SIMPLE_MATCH`, `FOREIGN_KEY_DELETE_COLUMN_SUBSET`, `CONSTRAINT_TEMPORAL_PERIOD` | Special MATCH, action-column subsets and temporal/period semantics. |

A marker describes metadata the migration generator cannot fully reproduce; it is
not blanket permission to modify a table. A CHECK or comment can affect validation
or DDL reproduction without implying a different row-equality rule. A CRUD caller
must separately check a stable nondeferrable PK, column generation, collation,
triggers/RLS and other identity assumptions, and reject unknown features. The
capture library intentionally does not make that UI policy decision.

This remains a bounded, conservative representation. It does not preserve every
PostgreSQL object attribute, ownership policy, extension semantic or future server
catalog addition. It does not certify arbitrary-schema migration safety or provide
pg_dump parity. Unsupported markers can deliberately block otherwise simple plans;
a future dependency/DDL adapter must model those features before relaxing them.
External/global dependencies and concurrent DDL still need executor-side checks.

## Ordinary-table rewrite rules: fail closed

`metadata_features::USER_REWRITE_RULES` has the exact value `user rewrite rules`.
Both full and targeted capture emit it for an ordinary `relkind='r'` table with
any `pg_rewrite` entry, including ALSO and INSTEAD rules and conservatively disabled
rules. Normal views' `_RETURN` rules do not produce this ordinary-table marker;
views remain separately noneditable. The existing generic unsupported-feature
policy yields `Read-only metadata: user rewrite rules`. The actionable path is to
inspect the rule definitions and keep the verified row editor read-only, rather
than assume a direct target table implies one direct DML effect.

The shared PG canonical fingerprint already contains ordered raw `pg_rewrite`
rows, including rule qualification/action node trees. No fingerprint query/hash
API change is needed. This captures a rule-definition change even when the table
OID, typed column/PK metadata and rule-presence marker remain unchanged. The hash
is a TOCTOU witness, not permission to apply edits to a table whose rules were
already present at capture; the unsupported marker must still be enforced.

Verification uses an explicitly ignored, exact-owned-URL test:

```sh
DBIDE_WORKFLOW_TEST_URL='postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture' \
  cargo test -p dbide-workflows --test postgres_rewrite_rules -- --ignored
```

It creates a unique confirmed-owned `dbide_workflow_rules_<pid>_<time>` namespace,
loads the definition templates in `fixtures/rewrite_rules_base.sql` and
`fixtures/rewrite_rules.sql`, captures ordinary-table ALSO INSERT/UPDATE/DELETE and
INSTEAD NOTHING variants, and verifies the view `_RETURN` control. Replacing an
ALSO action changes the canonical guard hash while the table OID and typed schema
semantic hash remain stable. The test never executes rule-firing DML; original,
audit and INSTEAD tables remain empty. Fallible checks return through cleanup,
which drops only the unique schema whose creation this test confirmed. The
explicit PostgreSQL 18.6 run passed; no fixture objects remain afterward.

This prerequisite does not enable UI Apply or establish universal server-effects
safety. Generated/default/constraint expressions and user functions remain an
explicit limitation; absence of rewrite rules is not proof that every server-side
operation has only a direct row effect. Later execution/portal guards have their
own separate acceptance boundary.

## Compare and fresh validation

`compare_snapshots(desired, actual)` delegates to the offline directional diff.
Both snapshots must be complete and have the same explicit scope. It executes
nothing. Plans use the schema crate's **semantic** `content_fingerprint()` for
fresh validation, including stable database/profile endpoint identity, database
name/version, scope, completeness and all represented/unsupported schema content.
Capture time/generation/offline label are excluded. A new unchanged live capture
therefore has the same semantic hash and can pass `verify_target`; a changed
column, database or scope invalidates it. Original capture hashes remain separate
evidence in plan version 2. A content hash alone cannot close the race between
recapture and DDL, grant execution authority or replace transaction/lock policy.

## Atomic table identity for row editing

`capture_table_for_edit(request, schema, table, limits)` returns
`CapturedTableForEdit { snapshot, relation_oid, metadata_fingerprint }`. The
ordinary catalog transaction captures the exact table's positive `pg_class.oid`
and typed metadata, then stays open while executing the PG driver's shared
`table_metadata_query(oid)` and `table_metadata_fingerprint(canonical_json)` helper.
There is no second connection/name-to-OID lookup and no reliance on a result row's
`tableoid`, so this works for an empty table before its first insert. Active
transaction state is checked after both query phases, and terminal ROLLBACK must
confirm Idle before the result is returned. A single client timeout covers all
three phases; errors/cancellation retire the dedicated connection. The wrapper rejects
missing names, views/materialized views, foreign/partitioned/inherited tables,
duplicate/zero OIDs and incomplete metadata. It captures the selected schema within
the same standard limits; a large schema can fail its cap rather than returning a
narrow but falsely complete snapshot.

The OID is database-local row-edit identity evidence, not a schema semantic. It is
kept outside `SchemaSnapshot` and both migration schema fingerprints: dropping and
recreating an equivalent table can preserve its semantic fingerprint while changing
this wrapper's `relation_oid`. The canonical metadata fingerprint is a separate required row-edit guard,
not a migration hash: same-OID ALTER operations can change columns, constraints,
collation, triggers or rules while preserving the relation OID. It uses the exact
PG helper shared with execution, avoiding duplicated SQL or search-path-dependent
deparsing. The query returns one canonical JSON cell, capped at 1 MiB. Null,
malformed, multiple-cell/row, oversized or truncated output fails closed and yields
no edit capture. A fresh unchanged capture has the same canonical metadata hash.

The execution layer must compare both the captured OID and
`Some(captured.metadata_fingerprint)` against current metadata atomically after
its relation lock and before DML, under its connection/transaction policy. The
lock and canonical guard cover the explicitly modeled metadata; they do not claim
all catalog objects are immutable. Capturing these values neither authorizes
mutation nor closes the race on its own, and it does not replace PK, column,
collation, trigger/RLS and optimistic-conflict checks. `capture_schema` and offline migration APIs retain their
existing contracts.

## Additive targeted edit capture (opt-in)

`capture_target_table_for_edit(request, schema, table, limits)` returns the distinct
`CapturedTargetTableForEdit { snapshot, relation_oid, metadata_fingerprint }` type.
It validates complete metadata **for the exact selected table** under the same
OID/canonical-hash transaction contract. Its embedded schema snapshot always has
`complete=false`, contains only the selected relation, and deliberately omits
unrelated views and schema-object enumeration. The absence of those objects is
unknown, not authoritative. JSON preserves the incomplete flag, and migration
`diff` rejects the snapshot. This does not change `capture_schema` or the existing
whole-schema `capture_table_for_edit` behavior.

The targeted path applies the exact escaped table-name predicate to relation,
column, PK, outgoing-FK and index catalog queries. Shared relation feature checks
remain intact. Its additional `metadata_features::INCOMING_MUTATING_FOREIGN_KEY`
marker checks **all namespaces** for incoming ON DELETE/UPDATE CASCADE, SET NULL
or SET DEFAULT; it does not rely on unrelated source-table DTOs being returned.
The existing outside-scope incoming-FK safeguard is also retained. Consumers must
reject these mutating incoming effects until a broader effect preview exists.
The canonical guard still covers all incoming/outgoing constraints, so targeted
and full capture return the same guard hash for the same unchanged table.

Target mode runs seven catalog SELECTs plus the shared canonical metadata SELECT,
rather than the full mode's eight plus one. It uses the same three driver requests,
dedicated connection, overall timeout and confirmed rollback. Record, relation and
column limits apply to the returned target metadata rather than unrelated tables;
the canonical 1 MiB dependency bound remains in force. A selected table with too
many columns/indexes/constraints or dependency metadata still fails closed.

This API is additive. The current CRUD checkpoint continues using full capture;
no pagination caching or UI transition is part of this change. A future UI entry
point must explicitly accept `CapturedTargetTableForEdit` as a complete-target
contract, retain profile/provenance/identity/feature checks, and reject incoming
mutating effects. Passing its partial snapshot to the existing full-schema
`VerifiedTable::capture` constructor correctly fails; do not change the partial
snapshot to `complete=true` to bypass that check.

The owned read-only test uses the existing `dbide_workflow_features` schema:
whole-schema capture fails with `max_relations=1`, while the targeted empty-table
capture succeeds with one relation, two columns and eight records as caps. It
verifies incomplete JSON/migration rejection, full/target canonical-hash equality,
composite outgoing FK fidelity and same-schema incoming CASCADE detection. The
five explicit owned read-only integration tests and thirteen offline workflow
tests passed on PostgreSQL 18.6 after this addition; no new fixture DDL was needed.

## Exact view definitions (2026-10-22)

`capture_view(request, schema, name, limits)` returns
`CapturedView { schema, view: dbide_schema::View }` for exactly one actual view or
materialized view. This is a partial object-level result, not a schema snapshot
and not evidence of whole-schema completeness. A missing object or ordinary table
is rejected. `View.materialized` comes from the actual catalog relation kind.

One metadata SELECT filters exact schema/name and `relkind IN ('v','m')`, then reads
`pg_get_viewdef`. It does not enumerate other relations, columns, constraints,
indexes or schema objects. The dedicated session uses a READ ONLY/repeatable-read
transaction and confirms rollback. The current Session API has no parameterized
SELECT entry point: schema/name are **data values encoded with the existing
PostgreSQL E-literal encoder**, not identifiers and not protocol Bind parameters.
Quotes/backslashes are escaped; NUL, empty/overlong names and system schemas are
rejected. The static format substitutes each value once, so template-like text in
a valid name is preserved. No PREPARE/EXECUTE lifecycle or driver changes are used.

A private request copy caps connection timeout at four seconds. A separate limits
copy caps client/server query timeout at four seconds; shorter caller limits stay
shorter. Other profile fields and the original profile/limits remain unchanged.
These are distinct connection and query budgets. Server output is capped by encoded
payload octet length; driver bytes/events are also bounded. Null-over-budget,
truncated, duplicate/malformed, wrong-kind/identity or nonterminal transaction
reports yield no partial definition. The returned definition is display metadata,
never an executed view query.

Offline tests cover quote/backslash/injection-shaped names, NUL and UTF-8 byte
bounds, object-only query shape, limit validation, private timeout copies, normal
and materialized kind decoding, identity and report/cap failures. The explicit owned
PostgreSQL 18.6 `parent_view` test passed with `max_records=1`, `max_relations=1`
and `max_columns=1`, where whole-schema capture fails from unrelated metadata.
The test also rejects ordinary/missing objects and an injection-shaped name. There
is no materialized-view fixture in the existing owned seed: its kind is covered by
query/typed decoding tests, with live materialized verification still unclaimed.
No new DDL, rule/fault experiment or database write was performed for this feature.

```sh
DBIDE_WORKFLOW_TEST_URL='postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture' \
  cargo test -p dbide-workflows --test postgres_readonly live_view_capture_ignores_unrelated_schema_caps_and_rejects_nonviews -- --ignored
```

## Plan-only EXPLAIN

`explain_plan(request, sql, analyze, limits)` currently requires `analyze=false`.
True is rejected before any connection. SQL must parse as exactly one query,
INSERT, UPDATE, DELETE or MERGE AST. Scripts, nested EXPLAIN, CALL, EXECUTE, COPY,
DDL and transaction commands are rejected. The service serializes the validated
AST, then executes `EXPLAIN (FORMAT JSON, COSTS TRUE, VERBOSE TRUE)` inside its own
READ ONLY transaction. This prevents raw suffix/semicolon/comment injection.

The result is the typed EXPLAIN tree with `ExecutionMode::PlanOnly`; no ANALYZE
option or command execution path exists. Caller query input is capped at 64 KB.
The dedicated planning session uses `search_path=pg_catalog,public`; use qualified
relation names when planning queries for another schema. Console temp objects and
session variables are intentionally unavailable in this separate service session.
Planning can acquire locks and PostgreSQL can evaluate immutable expressions or
user-declared immutable routines during planning. Plan-only is not an isolation
boundary against a hostile database server or routine's external side effects.
Server transaction read-only mode prevents ordinary database writes, and timeouts
bound connection work. Supporting ANALYZE requires a separate explicit execution
confirmation/policy workflow.

## Verification and optional owned fixture

```sh
cargo test -p dbide-workflows
cargo clippy -p dbide-workflows --all-targets -- -D warnings
export DBIDE_WORKFLOW_TEST_URL='postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture'
cargo test -p dbide-workflows --test postgres_readonly live_capture_recapture_and_explain_without_execution -- --ignored
```

The live test refuses every endpoint except the exact owned loopback fixture URL.
It reads existing `dbide_benchmark.customers`, verifies repeated semantic hashes
and no-op diff, and obtains a typed EXPLAIN without actual execution metrics. The
baseline fixture must already be loaded by the integration runner. Default tests
ignore live tests; an ignored test is not a passing live check.

A second ignored test reads a richer fixture. Its setup SQL is a **separate
explicit database mutation** for an authorized parent integration runner:
`crates/dbide-workflows/fixtures/catalog_features.sql`. The workflow never loads
it. After authorized setup, run:

```sh
cargo test -p dbide-workflows --test postgres_readonly live_composite_fk_and_unsupported_metadata_are_preserved -- --ignored
```

Offline tests cover typed records, composite identities, capture-versus-semantic
hashes, unsupported metadata, caps/partial failures, scope/literal injection,
single-statement canonical EXPLAIN and pre-connection ANALYZE rejection. The
baseline live capture/recapture/EXPLAIN and rich composite-FK/generated/index
capture tests were explicitly run against PostgreSQL 18.6 and passed on
2026-10-21. Their fixture setup was performed separately by the authorized parent
integration runner.

The metadata-fidelity fixture is another separate authorized, idempotent setup:
`crates/dbide-workflows/fixtures/catalog_fidelity.sql`. It adds comments, ordinary
CHECK, deterministic `C` and ICU nondeterministic collations, deferred PK/UNIQUE
and an unvalidated CHECK. ICU is required for that optional fixture. After the
parent integration runner loads it, the read-only check is:

```sh
cargo test -p dbide-workflows --test postgres_readonly live_precise_collation_comment_and_constraint_metadata -- --ignored
```

The parent explicitly loaded the metadata-fidelity fixture into the owned
PostgreSQL 18.6 container, confirmed ICU availability, and ran all three metadata/planning ignored
read-only integration tests successfully on 2026-10-21. This verifies the distinct
comment/CHECK, deterministic custom collation, nondeterministic collation, deferred
PK/UNIQUE and unvalidated-CHECK markers against actual catalogs. The workflow
service itself still performs no fixture setup or database mutation.

The idempotent fidelity seed also creates `fidelity_empty_edit` without inserting
rows. Its constraint setup uses an exact relation/name catalog guard and performs
no DROP/recreate or data manipulation. After the fixture runner explicitly seeded
it, all four ignored read-only tests passed on PostgreSQL 18.6, including positive
OID/metadata capture for the empty table, stable recapture, and view/missing-target
rejection. Offline tests separately verify that changing only the captured OID
changes row-edit identity while leaving the semantic schema fingerprint unchanged.

The same-transaction canonical guard extension passed 11 offline workflow tests
and all four explicit PostgreSQL 18.6 read-only integration tests. The empty-table
edit capture verifies a stable 64-character lowercase metadata hash across fresh
captures. Mock reports verify shared PG hashing and rejection of null, malformed,
multi-row, oversized and truncated canonical metadata. The existing mock check
still proves OID changes do not alter semantic migration fingerprints.
