# Stored result pages

**SQL 도구 → 읽기 전용 페이지 조회…** / **SQL tools → Read-only paged query…**
captures the full editor SQL, target and in-memory credential for review. Pressing
Run starts one independent read-only query. SQLite, PostgreSQL and MySQL use the
same sources as [CSV streaming](STREAMING-EXPORT.md), including their supported
SQL/type restrictions and cancellation contracts.

The entire successful result is written once to a private temporary file. Only
then is its first page shown. Next/Previous read this file; they never execute SQL
again, change offsets in a new SQL query, or retain a live database connection.
This provides consistent browsing of that completed execution. It is **not** a
suspended server cursor, partial-results streaming while the query is running,
or continuation of an older normal Run result. The existing console Run and its
retention limits remain unchanged.

## Bounds and ownership

- Source: at most 1,000,000 rows,64 MiB payload,512 KiB source row and 30 seconds.
- Spool: at most 64 MiB actual bytes, including JSON escaping/header/newlines.
- Page: at most 200 rows **and**512 KiB encoded records, so boundaries vary for wide rows.
- Header: at most 64 KiB; page index: at most65,536 entries.
- An oversized single row fails the query; no silent truncation or successful prefix.
- Empty results have one empty page so column names remain available.

Only one raw page, one converted source row, column metadata and the bounded
page index are retained in application memory. Native driver/server working
memory is separate. JSON records preserve UTF-8, NULL and empty text. Values are
native textual representations, not typed database values.

The capture receives an execution ID, SQL SHA-256, captured endpoint fingerprint
and total row count. The private file stores no credentials or SQL text. The last
owner dropping it removes the file. Closing a result panel releases its capture;
normal crash/force-kill recovery of these ephemeral files is not provided.

## UI behavior

The page result has its own grid and state; it does not replace a different
console's result. Switching SQL/table tabs hides the result panel while retaining
its captured job and file. **도구 → 페이지 조회 보기…** / **Tools → Show paged query…**
returns to it. Close the idle panel before opening another independent capture.
The displayed row range uses global row positions; Next/Previous reset the
vertical view to the new page's start.

Page reads run off the UI thread. A failed/cancelled read preserves the last good
page; it never recovers by rerunning SQL. Cancellation/close wait for the worker's
actual outcome. A graceful window-close request cancels an active job and keeps
the window until it stops; close again afterwards. The existing console can run
separately, and the visible page panel's cancel/focus/copy actions are routed to
that panel. Copy cell preserves its raw text (SQL NULL is copied as `\N`).

The model tests cover full-page walks and roundtrips after changing the source
database, JSON escaping, byte boundaries, empty results, late errors, cancel,
deadline, corrupt reads and last-owner file cleanup. Owned live tests additionally
walk 5,001 PG rows/26 pages and 5,000 MySQL rows/25 pages, checking every ID/value,
NULL/empty, a single captured native connection ID, and identical first-page return.
Native UI acceptance remains separate from these source/store tests; see dated
validation evidence for the current verification status.

```sh
cargo test -p dbide-result-store
# Only after checking the project-owned Docker fixture labels and loopback ports:
DBIDE_STREAM_OWNED_LIVE=1 cargo test -p dbide-result-store --test network_pages -- --ignored
```
