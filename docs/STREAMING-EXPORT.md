# Independent CSV streaming export

`dbide export` streams one supported query from SQLite, PostgreSQL or MySQL into
a **new** CSV file. It uses a separate connection with native read-only settings
or a READ ONLY transaction. It does not continue an older displayed result or
automatically replay a query. [Stored result pages](RESULT-PAGES.md) provide
same-execution browsing through a separate, explicit query operation.

## GUI

For a file-backed SQLite, PostgreSQL or MySQL console, choose **SQL tools → Run editor SQL to CSV…**
(한국어: **SQL 도구 → 편집기 SQL을 CSV로 실행…**). The panel captures the full
editor SQL, profile and document title when opened. It does not use a selection
or the SQL that produced an older displayed result. Review the captured SQL and
database path, choose a fresh output file, then press **Run export**. Choosing a
file alone does not execute SQL or create the CSV.

The GUI uses the same `dbide-export` service and default limits as the CLI. SQL
and target remain fixed if the editor, selected console or saved profile changes.
It runs once on a dedicated worker thread with constant-size approximate progress
counters. The progress payload is source bytes, not final CSV bytes, and provisional
rows do not imply a published file. The terminal result is authoritative, including
when a cancellation request arrives after publication has already succeeded.

Only one export panel/job exists per window. Switching or closing SQL tabs does not
retarget or abandon this independent job. Opening another inspector hides the panel
while preserving its job; **Tools → Show CSV export…** brings it back even from a
table tab. Close the idle panel before capturing a different SQL document. Preferences
language changes update the panel. SQL, paths and original database errors remain
untranslated.

**Cancel** requests cancellation and waits for the worker's actual outcome. Panel
close and destination changes are disabled while working. A graceful window/app
close requests cancellation and keeps the window open until completion is observed;
close again afterwards. This also protects against a close racing local recovery.
A crash or force kill cannot provide this graceful-cleanup guarantee. There is no
automatic retry. A lost worker result is shown as unknown, with instructions to
check the destination before explicitly selecting another output.

The current GUI defaults are fixed: 1,000,000 rows, 64 MiB, 1 MiB source row and
30 seconds. Use CLI options for other finite limits. The existing **Result actions →
Export CSV/JSON** still exports the retained snapshot only. This new command is a
fresh independent execution and cannot see console temporary tables or uncommitted
changes. It is not same-execution progressive fetching.

## CLI

```sh
dbide --config ./profiles.json export --profile local \
  --file ./queries/export.sql --format csv --output ./exports/result.csv
```

SQLite profiles must name an existing absolute file; `:memory:` and relative
paths are rejected. Network profiles use their captured database, TLS and explicit
credential settings. `--database` overrides the selected network database before
target-fingerprint validation (SQLite accepts only `main`). Use `--password-env`,
`--password-stdin` or `--keychain` explicitly; SQL stdin and password stdin cannot
be combined. The output parent must already exist.
`--sql`, `--file` and `--stdin` are mutually exclusive explicit input sources;
SQL is capped at 64 KiB. Connection credentials are never added to summaries or spool metadata.
`--expect-sql-sha256` and `--expect-target-fingerprint` optionally refuse changed
inputs before connection. The target is captured from the original profile once;
the connection always enforces read-only regardless of the profile setting.
The target fingerprint identifies configured endpoint/options, not a database
snapshot, inode, file-content digest or snapshot of a previous result.

The SQLite source connection has native `SQLITE_OPEN_READ_ONLY`, `query_only` and an
authorizer installed before SQL preparation. Native statement boundaries enforce
exactly one column-producing readonly statement; a comment-only tail is allowed.
DML, DDL, ATTACH and mutable PRAGMAs are refused. The separate connection does not
share temporary tables, uncommitted data, session settings or transactions with
the GUI/CLI query session. Successful output reflects this new execution's read
view. There is no retry after dispatch and no history mutation.

## Network engine contracts

PostgreSQL uses a new connection and `BEGIN READ ONLY`. A restricted single
SELECT/CTE AST and native prepare validate the statement/parameters/types; the
original SQL then executes once through the text protocol. This is **prepare
validation, not prepared-plan execution or an immutable namespace snapshot**.
Transaction control, writable CTEs, SELECT INTO, parameters and row locks are
refused. PostgreSQL READ ONLY is not a sandbox against every function side effect;
normal database privileges and function behavior still apply. Results use native
text representations. BYTEA (including nested BYTEA types) and non-finite numeric
scalars are refused; explicit casts to text are allowed. Arrays/composites remain
PostgreSQL text, not a typed interchange format. The adapter currently accepts
at most 16 MiB source rows and 16 GiB cumulative payload, within the CLI's other caps.

MySQL uses native prepare/exec_iter in a new READ ONLY transaction. It restricts
SQL to one parameter-free SELECT/CTE and a conservative builtin-function list.
User variables, locking clauses, executable comments/hints, OUTFILE/DUMPFILE,
backslashes, multiple statements and functions outside that list are refused.
This is deliberately narrower than normal `query`. Unsigned 64-bit integers,
DECIMAL, date/time, NULL and empty text retain their textual values. Binary and
invalid UTF-8 values are refused unless explicitly converted to supported text;
non-finite floats are refused. The driver's packet cap is clamped between 1 KiB and 16 MiB independently of the application row cap.

Both sources require protocol completion and connection retirement before success.
PostgreSQL cancellation uses its native cancel token; MySQL uses a control
connection to KILL the owned query/connection and verifies retirement. Cleanup
has one separate total allowance of up to 3 seconds after the execution deadline.
The synchronous shared service polls each source to completion after flag
cancellation; callers of the raw async adapter must do the same, rather than
aborting/dropping its future. Forced process termination cannot guarantee cleanup.
Native incoming frames/rows may allocate before application limits can inspect
them; neither the byte budget nor a READ ONLY transaction bounds server query
working memory. Source checks cannot preempt a blocked OS write.

PG/MySQL textual payload accounting counts at least 1 byte for each empty/null
value and header. SQLite retains its native scalar accounting (8 bytes per number).
Live progress is approximate and final source counters are authoritative; payload
counts are not directly comparable across engines or equal to encoded CSV bytes.

## Limits and interruption

| Option | Default | Meaning |
|---|---:|---|
| `--max-rows` | 1,000,000 | Maximum complete data rows; one additional row fails the export |
| `--max-bytes` | 67,108,864 | Caps cumulative source payload, CSV logical payload and encoded file bytes |
| `--max-row-bytes` | 1,048,576 | Source row/header cap before application-owned conversion |
| `--timeout-secs` | 30 | Native progress and application deadline, including encoding/sync before publication |

Every limit must be finite and positive. CSV encoded bytes can exceed source
payload because quoting, escaping, labels and number rendering add bytes; either
cap can fail first. Numeric source accounting uses eight bytes per value while
CSV logical accounting uses its scalar representation. Only a current borrowed
SQLite row, one converted row, one expanded CSV field and the encoder's bounded
buffer are retained. NULL markers are expanded field by field, so repeated NULL
cells cannot accumulate a whole row of large marker strings before the output
byte cap is checked.
The full result is never accumulated in `ExecutionReport`, `TabularData` or a
whole-output byte vector.

This bounds application row retention. It does not bound SQLite's native query
working memory, temporary sort files or the native allocation of a very large
cell before its size can be inspected. Native progress checks stop running SQL
on Ctrl+C/deadline; row and pre-publication checks also stop encoding/publication.
A blocked operating-system write or `sync_all` is not a hard realtime deadline.
The existing 250 ms SQLite busy timeout remains enforced. Cancellation ends this
connection and cannot interrupt a later query session.

## CSV contract

Headers preserve positional, duplicate and empty result labels. SQL NULL uses
`\N` by default; empty text remains empty. `--null-marker '<NULL>'` selects a
different nonempty marker. Any real text equal to that marker is refused, even
if CSV would quote it. UTF-8, quotes, commas and embedded line breaks are encoded
by the shared CSV implementation.

Integers and finite REAL values are explicitly rendered as text; CSV does not
preserve SQLite scalar types. Invalid UTF-8 text, BLOBs and non-finite REALs are
refused. Select an explicit textual representation such as `hex(blob_column)`
when that is the intended contract. Text is preserved by default, including
spreadsheet formulas. `--escape-formulas` adds an apostrophe to dangerous
spreadsheet text, including labels, and deliberately changes those values.
Escaping collisions with the selected NULL marker are refused as well.

## Publication and reporting

`--output` is required. Existing files, directories and symlinks are never
replaced; there is no `--force` or stdout data stream. The source database and
its SQLite `-wal`/`-shm`/`-journal` names are forbidden as output destinations. Data is written to
a private temporary file in the destination directory. Only complete native
execution, CSV flush, file sync and final cancel/deadline checks permit atomic
`persist_noclobber` publication. A competing destination creation still wins
safely; the existing file is preserved. Native, encoding, disk, budget, timeout
and cancellation errors discard the provisional file and report that output
was not published. The file sync does not claim directory crash durability.

Success prints one JSON statistics object to stdout with `schema_version`,
`ok`, `format`, `rows`, `encoded_bytes`, `payload_bytes`, absolute UTF-8 `output`
string, captured `execution_target`, `sql_sha256`, `read_only` and
`independent_connection`. Non-UTF-8 output paths are refused with configuration
exit 2 before temporary file creation or SQL execution. If reporting fails after
successful publication, the
error explicitly says the file **was published**; do not replay SQL to fix it.
Argument/configuration errors exit 2, connection errors 3, execution/encoding/
budget/deadline errors 4 and Ctrl+C cancellation 130. Existing `dbide query`
retention/export behavior remains unchanged, including truncation exit 5.

Contract fixtures are in `crates/dbide-cli/tests/export_contract.rs`,
`crates/dbide-sqlite/src/export.rs` and `crates/dbide-data/tests/data_contract.rs`.
They use temporary owned databases only, including a 6,000-row export beyond
512 KiB, late native/encoding errors, native cancellation/deadline and budgets.

## Release observation — 2026-10-22

Apple M5 /24 GiB /macOS27.0, final release CLI, owned SQLite fixture. A recursive
SELECT generated sequential IDs and 100-byte text values without retaining the
whole result in the application. Every CSV row and the last ID were checked
after timing; the source database hash remained unchanged.

| Rows | Encoded CSV bytes | CLI maximum RSS | Wall observation |
|---:|---:|---:|---:|
|6,000|634,904|9,519,104 bytes|0.474s|
|500,000|53,888,906|9,535,488 bytes|0.212s|

RSS is `/usr/bin/time -l` process maximum resident size, not allocated-byte or
whole-system memory. Wall includes the fresh CLI process, setup, encoding,
sync/publication and stdout. There was one sample per size, with the6,000-row
case first; code-signature/OS cache warmup was uncontrolled. These are functional
and memory observations, not throughput rankings or a hard memory bound for
arbitrary SQLite queries. [Raw evidence and SQL](../evidence/performance-2026-10-22-streaming-export.json).
