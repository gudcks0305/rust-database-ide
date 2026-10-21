# Data transfer, comparison and edit plans

Implemented in `crates/dbide-data` (2026-10-21). This is a reusable Rust domain
surface with fixture tests. It does not read user profiles, connect to a database,
or execute SQL. Native UI actions must call these APIs to claim an integrated
workflow; a model API alone does not establish M2/M4 completion.

## Public model

All public models derive `Serialize`/`Deserialize` and exact equality where
appropriate. `Cell` uses a tagged representation (`kind` and optional `value`):

- `Null`, `Text(String)`, `Bool(bool)`, `Integer(i64)`, `Decimal(String)`,
  `Binary(Vec<u8>)`.
- `Decimal` retains its complete input lexeme and scale. Validation accepts a
  signed decimal with optional fraction/exponent; no `f64` conversion, `NaN`,
  infinities or inferred number types. It does not canonicalize numerically
  equal spellings (`1.0` and `1.00` compare different).
- `TabularData { columns: Vec<Column>, rows: Vec<Vec<Cell>> }`,
  `Column { name: String }`; every row must match the schema arity.
  Result labels are positional: repeated `?column?`/`id` and empty labels remain
  unchanged through CSV/tagged JSON transfer and index-keyed diff. No automatic
  suffixes or replacement labels are introduced. Destination mapping/edit names
  remain independently validated unique identifiers.
- `Limits { max_rows, max_bytes }`; both must be positive, byte budget finite.
  Default: 10,000 rows and 16 MiB. These are budgets, not throughput measurements.

Every public operation validates its input. A logical payload byte budget counts
UTF-8 names/text/decimal bytes, binary bytes, eight bytes per integer, and at least
one byte per other cell/name. This is not an exact allocator/RSS budget. CSV/JSON
also limit encoded bytes, so JSON structure or CSV quoting can exceed a budget
even when logical values fit. Diff inputs each have their own budget; its cloned
result and indexes are bounded by those inputs, not a single combined RSS cap.

## CSV/TSV transfer

```rust,ignore
read_csv(reader, &options, columns, limits, ReadMode::Preview | ReadMode::Import)
    -> Result<DataPreview>;
write_csv(writer, &columns, rows, &options, limits) -> Result<()>;
```

Both functions accept normal `Read`/`Write`; CSV reads one record at a time and
writes rows incrementally. An input decoder may buffer/read ahead within its raw
byte budget. The output model contains at most `max_rows` rows. Preview observes
one extra row to report `DataPreview.truncated`; import rejects excess rows.
A preview is not proof that the remainder of a file is valid or complete.
Invalid UTF-8, unequal row width, unsupported delimiter and exceeded budgets fail.

`CsvOptions`:

| Field | Default | Contract |
|---|---|---|
| `delimiter` | `b','` | `b'\t'` for TSV; ASCII except NUL/CR/LF/quote |
| `header` | `true` | Header names come from file; with false caller supplies `Some(Vec<Column>)` |
| `null_marker` | `Some("\\N")` | Nonempty explicit file convention; `None` forbids NULL export |
| `formula_mode` | `FormulaMode::Preserve` | Exact text; `Escape` explicitly adds apostrophes to spreadsheet formula candidates |
| `scalar_mode` | `CsvScalarMode::TextOnly` | Reject typed export; `TextualValues` explicitly renders scalars as text |

CSV import returns `Null`/`Text` only. It never guesses boolean/integer/decimal
from text. Empty text remains empty. A literal text value equal to the configured
NULL marker causes export to fail; quoting does not resolve this semantic
collision. Choose a different marker, or tagged JSON. An externally authored
CSV must use the declared NULL convention; otherwise matching literal text
cannot be distinguished from NULL. Spreadsheet escaping is deliberately a
visible, non-reversible output mode and applies to header names too. Preserve
mode remains exact; escaping is not silently applied on export or undone on
import. Escape mode rejects a formula-like NULL marker and any post-escape marker
collision. Binary CSV export is unsupported.

Write failures may leave a partial stream. File callers should write a new
scratch file and publish it atomically only after successful completion. Transfer
functions themselves never open or overwrite files.

## Tagged JSON transfer

```rust,ignore
write_json(writer, &data, limits) -> Result<()>;
read_json(reader, limits) -> Result<TabularData>;
```

Version-1 envelope: `{ "version": 1, "data": { "columns": ..., "rows": ... } }`.
JSON streams to `Write`; import reads at most the finite raw byte budget plus one
probe byte before deserialization. Schema, NULL, empty strings, Unicode, binary,
exact decimals and type tags round-trip. Unknown versions and arbitrary untyped
JSON objects are rejected. Numbers are not guessed/coerced. Decimal data remains
JSON string content, retaining precision beyond IEEE-754/i64 limits.

## Import mapping preview

```rust,ignore
preview_import(&data, &mapping, limits) -> Result<ImportPreview>;
```

`ColumnMapping { source_index, target_name, expected_kind, nullable }` covers every
source column exactly once. Source/target duplication, missing mapping and width
mismatches fail. `ImportPreview` returns mapped rows and zero-based
`MappingIssue { row, column, actual }`. `ready()` is false until all type/nullability
issues are resolved. No coercion occurs: CSV `"42"` is Text, not Integer. Missing
DB defaults/generated columns, vendor type conversion and executing a bulk
import are separate adapter/UI work; this API does not claim them.

## Explicit-key data comparison

```rust,ignore
diff_by_key(&before, &after, &key_columns, limits) -> Result<DataDiff>;
```

Both complete inputs must share the exact ordered schema and meet budgets. Key
columns must be nonempty, distinct and in range. NULL keys and duplicate keys on
either side fail as ambiguous identity. No positional matching or inferred key.
`RowDifference::{Insert, Delete, Change}` retains key and original/replacement
rows; changes include changed column indices. `DataDiff` reports input counts
and unchanged count. Deletions/changes follow original order, insertions follow
replacement order. Comparison uses exact kinds/values, not SQL collation or
numeric equivalence. Truncated transfer previews are not full-table comparisons;
callers must disclose the actual comparison range and completeness.

## PostgreSQL edit preview

```rust,ignore
plan_edits(&table, &edits, limits) -> Result<TransactionPlan>;
```

`TableSpec { schema, name, columns, identity }`,
`EditColumn { name, pg_type, nullable }`,
`RowIdentity { kind: PrimaryKey | UniqueConstraint, columns }`.
Identity must come from an enforced catalog constraint; an arbitrary selected
column is not enough. `identity: None` makes this initial API read-only, including
insert planning. NULL identity values, mismatched types/arity, duplicate original
target keys and NUL SQL identifiers/text fail. Schema/table/column names must be
at most 63 UTF-8 bytes (standard PostgreSQL identifier limit); longer quoted
names are rejected rather than silently truncated to another object.
Identity normalization is separate from exact cell equality. Numeric `1`, `1.00`
and `1e0`, UUID uppercase/hyphen aliases, integer text aliases and boolean text
aliases identify the same DB row and are rejected as repeated batch targets.
Numeric keys normalize to sign/coefficient/exponent without expanding powers of
ten or converting through `f64`. Payloads and transfer/diff cell equality remain
unchanged. Numeric input is capped at 160,000 bytes, exponent magnitude 147,455,
131,072 integer digits and 16,383 fractional digits; checked arithmetic rejects
overflow before allocation proportional to an exponent. Zero normalizes without
a sign. Float/timestamp/JSON identities remain explicitly read-only until their
equivalence is normalized. The caller must verify the catalog identity uses
supported built-in types and deterministic text collation; custom/domain types
and nondeterministic collations cannot be assumed equivalent to this normalizer.

`Edit::{Insert { values }, InsertDefaulted { values },
Update { original, replacement }, Delete { original }}`
produces `PlannedStatement { kind, sql, parameters, expected_affected_rows: 1,
destructive }`. Updates emit assignments for changed columns only; no-op updates
emit no statement. Identifiers are independently double-quoted, values are never
interpolated, and every original column participates in
`IS NOT DISTINCT FROM $n::pg_catalog.text::pg_catalog.type` predicates, with composite identity
columns first. NULL and empty values remain different parameters. Update/delete
are marked destructive. Identity changes are allowed using the original identity
in the predicate; constraint violations remain server errors.
`InsertDefaulted` accepts exactly one `InsertValue::{Value(Cell), Default}` per
column. Explicit Default emits the closed SQL token `DEFAULT` without a parameter,
allowing generated identity, catalog defaults and generated columns. Value(Null)
is SQL NULL and does not request a default. Missing fields/arity fail; parameter
indices remain contiguous after any defaults. No raw expression API is exposed.
The server checks default availability and generated-column constraints inside
the explicit transaction; a preview does not promise an insert will succeed.

Binding contract is intentionally narrow:

| `PgType` | Accepted non-NULL `Cell` | Explicit SQL cast |
|---|---|---|
| Text | Text | `pg_catalog.text` |
| Boolean | Bool or Text (`t`/`f`/`true`/`false`, ASCII case insensitive) | `pg_catalog.bool` |
| SmallInt | Integer or integer Text, i16 range | `pg_catalog.int2` |
| Integer | Integer or integer Text, i32 range | `pg_catalog.int4` |
| BigInt | Integer or integer Text, i64 range | `pg_catalog.int8` |
| Numeric | Decimal, Integer or exact numeric Text | `pg_catalog.numeric` |
| Bytea | Binary | `pg_catalog.bytea` |
| Uuid | Text, 32 hex digits or standard 36-character hyphenated UUID | `pg_catalog.uuid` |
| Date | Text, valid Gregorian `YYYY-MM-DD`, years 0001–9999 | `pg_catalog.date` |
| Timestamp | Text, ISO date + space/T + `HH:MM:SS[.ffffff]` | `pg_catalog.timestamp` |
| TimestampTz | Text, timestamp plus Z or signed HH/HH:MM offset | `pg_catalog.timestamptz` |
| Json | Text, validated JSON with arbitrary-precision numbers | `pg_catalog.json` |
| Jsonb | Text, validated JSON with arbitrary-precision numbers | `pg_catalog.jsonb` |
| DoublePrecision | Text, bounded float lexeme or explicit NaN/Infinity/-Infinity | `pg_catalog.float8` |

All accept Null when nullable. Double validation is allowed to use IEEE-754 for
this explicitly approximate destination; exact Numeric/JSON never use it.
ISO validation rejects invalid dates/times, leap seconds, precision above six
fractional digits and unsupported spellings; broader PostgreSQL date/time forms
are not inferred. Server-specific range/typmod/rounding, jsonb numeric/NUL rules,
constraints and collation remain server checks. JSON parsing retains the exact
wire string; duplicate object names and whitespace are not rewritten.
PostgreSQL `json` lacks equality, so its original-value predicate compares
`column::pg_catalog.text IS NOT DISTINCT FROM $n::pg_catalog.text` (exact stored
text). `jsonb` uses typed equality. Every original column still participates.

`PgParameter.value` is an optional text payload: None is SQL NULL,
Some("") is empty text, Bytea is `\x` hex. Every placeholder first casts to
`pg_catalog.text`, then to the target type in the table above (Text needs only
one cast). The actual prepared parameter type is therefore TEXT: a future
tokio-postgres executor can bind ordinary `Option<String>` values. The `pg_type`
field records the destination type, not the wire parameter type. Do not bind
numeric/binary payloads using the destination wire type or interpolate SQL values.
Arrays/custom/domain codecs are unsupported here; Text is not a universal
binding type for arbitrary columns. PostgreSQL parameter count is capped at
65,535. Plans require a bound-parameter execution path; a simple-protocol query
path cannot execute them safely.

`TransactionPlan` requires explicit submit and one transaction. Executor contract:

1. User reviews and explicitly submits the concrete plan.
2. Begin on a known, pinned compatible session; bind/execute statements in order.
3. Call `check_affected_rows` after each statement. Zero is an optimistic conflict;
   more than one violates expected identity. Roll back the entire batch on either
   result or any statement error. Do not partially commit.
4. Commit only after every check succeeds. A disconnect or failed commit can mean
   unknown outcome; report uncertainty and never automatically retry mutations.

Value predicates detect concurrent changes to the loaded values, not ABA changes
or row-version history. Triggers/RLS/collation/custom semantics require live
adapter contracts. An affected-row check cannot undo a transaction already
committed, so it must occur inside the transaction. No write is automatically
executed by this crate, and offline SQL tests do not establish server behavior.

## Verification

`cargo test -p dbide-data`: 28 offline contract tests (generated byte fixtures),
covering quote/newline/Korean, NULL/empty/literal marker, formula modes, invalid
UTF-8, input/output/row limits, headerless TSV, exact tagged JSON, mapping errors,
explicit-key classifications and duplicates, composite identity, malicious
identifier/value SQL cases, ASCII/multibyte identifier byte boundaries,
numeric identity spelling aliases,
duplicate/empty result label round trips and target mapping ambiguity,
NULL optimistic conditions, affected-row conflicts,
exact decimal/binary parameter plans and unsupported edits, closed scalar casts,
integer/ISO/JSON/UUID validation, DEFAULT parameter numbering, JSON optimistic
predicates, normalized composite identities and numeric precision/scale/input
boundaries without expanded decimal allocation.

`cargo clippy -p dbide-data --all-targets -- -D warnings` and
`cargo fmt -p dbide-data -- --check` pass. Default crate tests also report one
ignored live contract; an ignored test is not a pass.

### Explicit owned PostgreSQL contract

`tests/postgres_edit_contract.rs` is an opt-in integration test of these generated
plans against `dbide-pg-spike::Session::execute_batch`. It requires the exact
environment value `DBIDE_DATA_FIXTURE=owned-loopback` before any connection, and
hardcodes the project's generated fixture credentials and
`127.0.0.1:55439/dbide_fixture`; it never reads profiles, a URL override or user DB
settings. The test creates one unique `dbide_data_<pid>_<nanos>` schema and drops
only that schema afterward, including when an assertion panics. Sessions are
closed on failure/timeout and no generated write is retried.

```sh
bash scripts/fixture-pg.sh start
DBIDE_DATA_FIXTURE=owned-loopback cargo test -p dbide-data --test postgres_edit_contract -- --ignored
```

Explicit run passed on PostgreSQL 18.6/aarch64, 2026-10-21. Docker fixture owner
label was checked as `rust-database-ide-m0` before running. The server had zero
remaining `dbide_data_%` schemas after cleanup. This is a separate observed live
pass, not an inference from the default 28 offline passes.

The live contract exercises all 14 closed scalar destination types, `DEFAULT`
with generated-always PK/catalog default, exact numeric/JSON text and UUID
payloads, bytea, NULL versus empty text, changed JSON with its exact original
text predicate, update and delete success. A second session changes a row between
read and submit; a two-statement update batch and a two-statement delete batch
each report affected rows `[1, 0]`, fail with application `Conflict`, return to
Idle and roll back the first mutation as observed independently. Missing or
unsupported identity and duplicate numeric/UUID spelling aliases are rejected
locally before creating a dispatchable plan.

No user file/database was used. Streaming UI integration, atomic filesystem
publication, vendor codecs, broader server versions and trigger/RLS/collation
contracts remain separate work. The fixture verifies this particular native
parameterized executor and the generated plans; it does not make arbitrary
simple-protocol SQL a safe mutation path.
