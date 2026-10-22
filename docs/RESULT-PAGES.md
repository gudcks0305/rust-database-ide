# Live result pages and PostgreSQL cursors

Open **SQL 도구 → 읽기 전용 페이지 조회…** / **SQL tools → Read-only paged query…**.
Review the captured SQL and target, then press Run. Rows appear as they arrive;
the view does not wait for the entire query to complete. The original SQL and
connection credential are captured once, separately from console transactions.
This does not resume an older, already-truncated normal Run result.

## Reading and fetching

SQLite and MySQL stream automatically on an independent read-only connection.
PostgreSQL opens a new READ ONLY transaction and declares one `NO SCROLL` SQL
cursor. The first logical batch contains at most 200 rows. At its end, the cursor
waits for an explicit **다음 가져오기 / Fetch next** request. Browsing previously
received pages reads the local cache; it does not FETCH or rerun SQL.

PostgreSQL SQL FETCH materializes a requested wire batch before returning it.
To show a prefix earlier, a logical 200-row request uses wire FETCH sizes
16, 64, 64 and 56. These all advance the same cursor. Normal command completion
and row counts are checked for every wire FETCH; only the complete logical batch
opens admission for the next user request. Duplicate/stale demands are refused.
An exact 200-row boundary may require an additional empty fetch to establish EOF.
The original SELECT is declared once, never rewritten with OFFSET or replayed.

In the owned PostgreSQL 18.6 slow-row fixture (`pg_sleep(0.03)` per row), the old
single FETCH 200 delivered its first row at approximately 6.7 seconds. The small
wire batches delivered the first driver callback in 578–594 ms, while the logical
batch finished at 6.66–6.79 seconds and the backend was still active at that first
callback. These are synthetic driver measurements, not physical display latency
or general performance comparisons. PostgreSQL's [utility-result execution](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/tcop/pquery.c)
explains the materialization behavior. Blocking sorts, aggregates and slow source
operations can still delay first rows.

## Partial data and limits

- Fully written row prefixes become readable immediately, including when the
  producer stops producing rows temporarily. The growing last page refreshes
  without resetting column widths, selection or scroll.
- During receipt, cursor wait, errors and cancellation, the UI shows **partial**
  data and received rows; it does not claim a known final total.
- Complete means native EOF **and** cursor/transaction/connection cleanup succeeded.
  A late error, cancellation, expiry or byte limit preserves received pages and
  reports the failure. There is no automatic reconnect or SQL replay.
- Source cap: 1,000,000 rows and64 MiB. A page holds at most200 rows and512 KiB of
  encoded records; wide rows can split a FETCH across several cached pages.
- Actual spool cap:64 MiB including JSON escaping/header/newlines. Header cap:64 KiB;
  index cap:65,536 pages. A single oversized row fails without silent truncation.
- Ordinary streams have a30-second total budget. PG cursors have30 seconds of
  aggregate active work,60 seconds idle between demands, and300 seconds total
  lifetime. Cache browsing does not extend either cursor timer. Native cleanup
  has one additional bounded3-second allowance.

The private temporary file stores rows and columns, not SQL or credentials. It is
removed after its last owner is released. Read/write failures preserve the last
valid displayed page; they never trigger a new SQL execution. Native driver/server
working memory and blocked OS calls are outside the payload/time guarantees.
Crash/force-kill cleanup is not guaranteed. The old success-only `query_pages` API
remains available for callers requiring a fully completed capture.

## UI and lifecycle

The result view is independent of the normal console grid. Switching tabs hides
it while preserving the captured query; **Tools → Show paged query…** restores it.
Cached Next/Previous take precedence over another server FETCH. One outstanding
demand and one page-read worker prevent duplicated reads and stale installs.

The compact header holds connection/document context and actions. SQL, limits,
full target and execution details are expandable. The grid fills the remaining
area; the footer shows row range, loaded pages, received rows and state. Cell
copy uses the original value (`\N` for SQL NULL), including Cmd+C and right-click.
Cancel and result-focus shortcuts target this visible result, not a hidden console.

Close requests cancellation, waits for the worker and backend cleanup, then closes
automatically. Window close retains its intent while related local storage settles;
existing unsaved-content prompts still apply. Cursor expiry keeps the partial data
but cannot reconnect to the old cursor. Start another explicit query to read again.

PostgreSQL is the only server-cursor mode in this release. MySQL/SQLite provide
progressive streaming and local cached-page navigation. All modes retain the
[engine-specific SQL/type restrictions](STREAMING-EXPORT.md). Native READ ONLY
transactions do not sandbox every database function's other effects.

## Verification

Tests cover first and second rows while the producer is paused before EOF, growing
pages, immutable prefixes, immediate repeated Fetch clicks, byte boundaries, late
SQL/storage errors, cancellation, empty results, stale reads, controller Drop and
file cleanup. Owned PG contracts cover paused backends, 401/400/0 row boundaries,
active/idle/lifetime limits, source receiver closure and native retirement.

```sh
cargo test -p dbide-result-store --lib
cargo test -p dbide-pg-spike --test sql_cursor_contract
# Only the checked project-owned PG loopback fixture:
cargo test -p dbide-pg-spike --test sql_cursor_contract -- --include-ignored
cargo test -p dbide-result-store --lib -- --include-ignored
```

See the [dated live-result validation artifact](../evidence/live-cursor-ui-validation.json) for the exact native UI checks and
current local-test counts. Defaults do not imply any entire database IDE milestone
or DataGrip parity is complete.
