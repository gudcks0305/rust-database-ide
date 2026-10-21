# SQL parser M0 spike

`dbide-sql-spike` compares sqlparser 0.63.0 and Tree-sitter 0.25.10 with
tree-sitter-sequel 0.3.11. These exact versions are pinned in Cargo.lock.
Tree-sitter 0.27.1 was considered; GPUI component requires the 0.25 runtime.
One workspace cannot link two Tree-sitter runtimes, so the spike shares 0.25.10.

```sh
cargo test -p dbide-sql-spike
cargo run --release -p dbide-sql-spike -- --large
```

Fixtures include semicolons in literals/comments, nested block comments,
PostgreSQL dollar quoting, CTEs, Korean identifiers/text, transaction scripts,
function bodies, COPY declaration, incomplete SELECT/string and escape strings.
Known statement-count contracts are asserted. Unsupported dialect constructs
are reported as results, not silently deleted from the corpus.

The incremental edit test uses UTF-8 byte offsets (including a newline and
emoji insertion), then compares every node's kind, byte/point ranges, named/error/
missing flags and structure to a full reparse. A regression fixture proves that
equal S-expression shapes with different byte ranges are rejected.
The CLI measures 10 KiB/1 MiB and optional 10 MiB synthetic buffers, five edits
each. It includes string/tree copy and edit time; it is not UI input-to-paint
latency, semantic completion, or a DataGrip comparison.

This is a parser experiment, not an SQL execution splitter. Driver raw SQL
execution must remain possible when either parser rejects the input. PostgreSQL
script transaction semantics must not change because a parser splits statements.
Tree-sitter offers partial error trees but the tested grammar's dialect coverage
must be recorded. sqlparser offers an AST, not catalog-aware name resolution.

Observed limitation: the Tree-sitter grammar can accept incomplete-looking SQL
such as `SELECT id, FROM` without an ERROR node, while sqlparser rejects it.
`has_error == false` must never be treated as proof of valid SQL. Tests require
full UTF-8 input coverage plus the strict parser diagnostic, not agreement between
the two candidates on every invalid buffer.
