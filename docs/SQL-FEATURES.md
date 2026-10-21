# SQL editor service contract

2026-10-21. `crates/dbide-sql` is a reusable, synchronous, database-free SQL
service. It implements a bounded part of F09–F14. This is **partial M3**, not
DataGrip parity, a full SQL validator, or a server authorization boundary.
SQL execution must remain available independently of analysis success.

## Integration API

```rust
use dbide_sql::{Catalog, CatalogTable, Dialect};
let catalog = Catalog { tables: vec![CatalogTable {
    schema: "public".into(), name: "users".into(),
    columns: vec!["id".into(), "name".into()],
}] };
let sql = "SELECT u.na FROM users u";
let suggestions = dbide_sql::completions(sql, 11, Dialect::PostgreSql, &catalog);
```

| API | Contract |
|---|---|
| `format_sql(sql, dialect)` | Parsed AST formatting with normalized round-trip verification. Commented input returns an error and the caller retains the original source, because AST formatting can discard hints/comments. No source is modified. |
| `statement_ranges(sql, dialect)` | Original UTF-8 byte ranges. Uses sqlparser tokens, not splitting raw semicolons. Includes leading inter-statement whitespace/comments in the next statement, trailing semicolon in the preceding statement. Tolerant lexical errors retain the entire unfinished tail. |
| `statement_at_cursor(sql, byte, dialect)` | Range containing the caret; EOF selects the final statement. Invalid UTF-8 byte positions, lexical failures and native procedural BEGIN bodies return `None`. |
| `analyze(sql, dialect, catalog)` | AST scopes, relation/alias/CTE binding, column resolution, symbols, distinct syntax/semantic/unsupported diagnostics. Syntax failure returns a source-wide syntax diagnostic, never a fabricated semantic success. |
| `Analysis::symbol_at(byte)` | A unique source-bound symbol, with declaration, usages and optional `(schema, table)` navigation target. Unknown locations, comments and string contents have no symbol. |
| `diagnostics(sql, dialect, catalog)` | Convenience analysis diagnostics. These describe local evidence; they are not server diagnostics. |
| `completions(sql, byte, dialect, catalog)` | Keywords, schema/table names, visible CTE names and uniquely resolved alias/column names. Includes a replacement byte range and quoted insertion text. A synthetic identifier is inserted only in a temporary analysis buffer. |
| `rename(sql, byte, name, dialect, catalog, revision)` | Preview only for resolved document-local relation aliases and CTE names. All edits come from AST/token source spans. Schema/table/column DDL renaming is separate and refused. |
| `RenamePreview::apply(sql, revision)` | Requires the original SHA-256 and exact revision, checks every edit range for UTF-8 validity/overlap, then produces a separate new string. Caller retains the old string for undo and replaces the document atomically. |
| `byte_to_utf16`, `utf16_to_byte` | UTF-8 byte / UTF-16 code-unit conversion. Invalid byte boundaries and surrogate midpoints return `None`; Korean, emoji and combining sequences are preserved. |

`Dialect::{PostgreSql, SQLite, MySql}` use the pinned sqlparser 0.63.0 dialects.
The PostgreSQL-first fixture suite covers catalog names, quoted case-sensitive
identifiers, Unicode aliases, joins, nested subqueries, alias shadowing,
sequential/nested CTE scope and independent statements. SQLite/MySQL currently
have basic formatting/tokenization contracts; server-specific semantic parity
has not been established.

A caller should perform analysis outside the UI rendering loop and attach its
own document revision and catalog generation to requests/results. This crate
has no background task scheduler or catalog cache. Rename carries an explicit
revision and hash; consumers must independently discard stale completion and
diagnostic results. The catalog contains only schema/table/column names, with
no credentials, live connections, types or database queries.

## Safety and unsupported cases

- Unqualified tables matching multiple schemas are ambiguous. There is no
  implicit `search_path`, role, temporary-object precedence or catalog generation
  model. Supply the intended catalog slice or qualify the schema explicitly.
- Supported semantic queries are SELECT/CTE expressions and ordinary joins with
  `ON`/no condition. Correlated subqueries and alias shadowing use nested query
  frames. Explicit CTE output names and direct projected column names are
  available; `*` output expansion/type inference is not implemented.
- Recursive/forward/duplicate CTEs, derived FROM tables, table functions,
  table-column alias lists, USING/NATURAL joins, vendor-specific joins, set
  operations, lock target binding, DDL/DML and dynamic SQL prevent rename.
  Unsupported statements may still be formatted or executed through the driver.
- Column types, result-expression types, overloaded functions, schema qualification
  beyond two name components, window declaration resolution, complete dialect
  name/collation rules, full SQL diagnostics and quick fixes remain unfinished.
- Rename refuses any existing analysis diagnostic, relation/CTE name collision,
  or same-spelled identifier token lacking a proven binding. Strings/comments
  are never edited. Names are quoted and embedded quotes escaped. Catalog
  renames, cross-file rename, extract refactoring and editor undo integration
  remain separate work. The renamed buffer must parse and bind successfully.
- Boundaries handle nested comments, escaped strings, dollar-quoted PostgreSQL
  functions and grouping simple native BEGIN/END routine bodies, including
  EXPLAIN-wrapped triggers, MySQL events and otherwise unknown BEGIN wrappers.
  All such native bodies return `None` for current-statement execution until
  dialect procedural grammar proves an executable boundary. Transaction
  `BEGIN;`/`BEGIN TRANSACTION` remains separate. Unsupported native
  conditional/loop control flow conservatively retains the whole routine tail;
  `statement_at_cursor` returns `None` for that tail, so callers must ask for an
  explicit selection/script action rather than executing an inferred fragment.
  MySQL client `DELIMITER`, COPY FROM STDIN payloads and client meta-commands
  need dedicated script/protocol handling before execution.
- Completion recovers syntactically incomplete identifier/dot positions by
  synthetic-identifier parsing. General incomplete parse recovery falls back to
  catalog/keyword candidates. It does not invent a successful semantic binding.
  Unqualified ambiguous columns are omitted. Completion in strings/comments is
  suppressed. Ranking, snippets and multicursor integration are unfinished.
- Formatting preserves no comments by design: commented input is refused
  explicitly. The normalized AST round-trip is a local syntax check, not proof
  that server-side SQL execution has identical behavior for every dialect.
- No incremental CST is maintained here. The separate M0 tree-sitter spike
  remains an experiment. Large-file latency and allocation gates are unmeasured;
  byte-location conversion is deliberately simple and should be indexed before
  making an IDE-scale performance claim.

## Reproducible verification

```sh
cargo fmt -p dbide-sql -- --check
cargo test -p dbide-sql
cargo clippy -p dbide-sql --all-targets -- -D warnings
```

`tests/editor_contract.rs` exercises non-mutating source handling, Unicode and
UTF-16, literal/comment/dollar-quote boundaries, incomplete buffers, formatting
comment refusal, catalog/schema ambiguity, alias/CTE navigation and scope,
quoted identifiers, preview validation/staleness, wildcard rename, rejected
unsafe refactoring and completion recovery. No user databases or network
services are accessed by these tests.
