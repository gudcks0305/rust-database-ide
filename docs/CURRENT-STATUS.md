# Current implementation and remaining acceptance

Updated 2026-10-23. This supersedes historical M0 limitations only where explicitly
listed below. It does not declare DataGrip parity or any entire M1–M6 phase complete.
The [42 feature groups](FEATURE-MATRIX.md) remain the full backlog.
**Priority reset:** [M0/M1 acceptance](M0-M1-ACCEPTANCE.md) comes first. M3–M6 remains authorized follow-on work; the core UX and
session correctness gaps are being addressed before wider integration. PostgreSQL row Apply remains disabled because namespace
rebinding acceptance has not passed; model/batch tests do not override that blocker.

## Basic workflow fixes and portable configuration

Settings and connections can be exported as one versioned JSON bundle from
Preferences → Settings transfer or CLI `config export/inspect/import`. Import
creates a new configuration directory, remaps relative file references under an
explicit root, and opens a separate window. Credentials, SQL/history, project
files and actual DB/CA files are excluded. Foreign absolute paths require CLI
`--map`; GUI supports choosing the relative root. See [transfer instructions](CONFIGURATION-TRANSFER.md).

Native validation exported four profiles and restored language, 16px editor font,
250-row table page size and recovery preference. A relocated SQLite file returned
the expected two rows in the restored window; the original SQL, result and live
connection survived. Opening an alias of the same configuration reused its
connected window. Korean/English native menus followed the active window, and
one Cmd+Q saved both windows' separate recovery snapshots before exiting.

Basic fixes verified by actual native interaction: clicked SQL tabs receive input,
Save As rejects non-.sql names before writing, successive saves work and show
feedback, a new file error takes priority over an existing connection error, and
the native Cancel menu cancels an in-flight connection attempt. A fresh console
selects the saved profile without connecting or submitting SQL automatically.
The settings panel exposes summaries, paths and operation feedback to accessibility.

[Validation](../evidence/portable-config-basic-validation.json): 533 default tests
passed /73 ignored, workspace Clippy and native packaging passed. The ignored
live DB suites were not rerun for these settings/UI changes. CI remains inactive.
SQLite table/field/data browsing passed; SQLite/MySQL rich index/DDL inspector
support remains incomplete, and broader feature parity is not claimed.

## Native streaming, stored result pages and bounded previews

GUI **SQL tools → Run editor SQL to CSV…** and CLI `dbide export` now share
`dbide-export`. The GUI captures the full editor SQL and SQLite/PG/MySQL profile when
opened, shows them for review, then starts only after explicit destination selection
and Run. It executes independently in native read-only mode and publishes a new CSV
only after success. Dedicated worker, approximate progress, cancellation and terminal
outcome remain independent of active tabs. Graceful window close cancels and waits;
another inspector can hide/reopen the same job without discarding it. Korean/English
switching is supported. See [the exact export contract](STREAMING-EXPORT.md).

Earlier SQLite native acceptance: the normal grid retained1,323 of6,000 rows; CSV contained all6,000,
with every row checked. Editing SQL after capture did not change the exported query.
File selection alone did not execute. Switching tabs during export kept the captured
source; window-close cancellation left no final or provisional CSV. Shared-service
fixtures cover limits, preflight, readonly, late errors, cancellation and zero rows.
[Validation](../evidence/gui-streaming-export-validation.json):471 passed /62 ignored;
ignored live tests were not run in that earlier batch.

The next implementation extends the shared service to PostgreSQL and MySQL,
using native READ ONLY transactions and engine-specific single-query restrictions.
Release CLI exports of5,000 rows per engine were verified row by row. The new
[stored result page view](RESULT-PAGES.md) runs once, then reads bounded pages
from a private temporary file after completion. PG5,001/26 pages and MySQL5,000/25
pages passed live whole-store walks/roundtrips. Native GUI validation walked
SQLite6,000/30 pages, preserved random values on return and after changing tabs,
verified raw Cmd+C and paged Cmd+. cancellation, and reached the final row of
401-row PG/MySQL captures. GUI MySQL CSV401 rows was also verified.

[Current validation](../evidence/streams-pages-validation.json):492 default tests
passed /70 ignored; eight new live DB tests were explicitly run and passed
separately. CI is still inactive. A separate QA bundle resolved a capture-tool
error on the older window. A subsequent implementation adds live first-page display for received rows and
PostgreSQL demand-driven SQL cursors. One logical 200-row request is split into
small wire FETCHes for earlier prefixes. Partial/error/cancelled data is clearly
labelled; cleanup precedes Complete, and close waits automatically. SQLite/MySQL
continue streaming while local pages are browsed. See [the current live result
contract](RESULT-PAGES.md). MySQL server cursors and continuation of an older
already-truncated normal Run are not implemented.

The [live cursor/UI validation](../evidence/live-cursor-ui-validation.json) records
510 default tests passed /73 ignored, plus three new owned live DB cases executed
separately. Native checks include receiving rows before FETCH completion, double-click
demand deduplication, exact right-click copy from SQL focus, a missing-file connection
error followed by successful retarget/run, current-statement menu/shortcut, MySQL
close-and-retire, and one Cmd+Q during an active PG fetch. UI cleanup moved verbose
information into details and made the result grid dominant. Project AGENTS now
requires basic user-flow verification before completion claims.

Grid display and AX values use bounded256-byte escaped previews; raw copy/export
retain original data. Optional GPUI profiler measurements and their limits are in
[the frame report](UI-FRAME-PERFORMANCE-2026-10-22.md). These results do not establish
a general performance gain or DataGrip superiority.

## Folder workspaces and agent contracts

Versioned `.dbide/project.json` plus ordinary SQL files now support explicit folder workspaces. The Files sidebar browses one directory at a time, creates SQL files/folders, preserves existing tabs on workspace switch, defaults Save As to the project directory, and persists recent/current roots privately. New file tabs resolve the project's exact default target or remain unbound; opening never connects or executes. GUI and CLI project saves share a nonblocking writer lock and content-hash comparison. Save As remains create-only inside any known project, including an inactive source project. Recovery schema3 preserves document project ownership and migrates schemas1/2 without reading source paths.

`dbide workspace init/show/files/read/write/mkdir/bind/context/plan` and `agent capabilities` provide versioned JSON. Context omits SQL/history/rows; explicit read returns SQL. Plan only produces argv with absolute file/config paths, explicit database and expected SQL/endpoint hashes. Guarded execution validates them before authentication/dispatch; relative SQLite file targets are refused. Existing read-only MCP scope is unchanged. See [workspace contracts](WORKSPACES.md), [official-tool comparison](DB-TOOL-BENCHMARK-2026-10-22.md), and `evidence/folder-workspace-qa.json` for dated verification.

Limits: single-root browser,512 returned entries/4096 scanned per directory,64KiB SQL, no filesystem watcher/Git integration/isolated tab sets per project/multiple attached roots. Non-cooperating file writers and malicious concurrent directory swaps are outside the cooperative lock guarantee.

## UI refinement and retained-result search

The result toolbar now opens a local text finder (`Cmd+Option+F`, or `Cmd+F` with the query grid focused). Enter searches the retained page off the UI thread; subsequent Enter/Shift+Enter and arrows move between matching cells. Searches never fetch from the database. Limits:100,000 cells,8MiB original text,64KiB per cell and1,000 matches; clipped scans are explicitly partial. SQL NULL is excluded from text search, while literal `NULL` is searchable. Case-insensitive matching uses Unicode lowercase, not normalization/full case folding. Changing the result page invalidates the search, and callbacks check exact page identity before selecting.

Open-tab overview includes dirty markers and connection/database targets. Right-side inspectors share a draggable, session-local width; narrower panels retain the central editor. Value/history/object finder rows use shorter labels with full detail in tooltips, the query grid shares workspace colors, and repeated result counts are removed. Full captured result provenance remains in the result heading tooltip. Persistent settings/CLI contracts are unchanged.

## Latest DataGrip workflow comparison

The [2026-10-22 gap audit](DATAGRIP-GAPS-2026-10-22.md) adds saved-metadata object search (`Cmd+Shift+O`), searchable/filterable query history (`Cmd+Shift+H`), explicit selected SQL (`Cmd+Alt+Enter`), read-only value/record inspectors, and loaded-result TSV range copy. These are bounded partial implementations; parameter binding, FK row navigation, keepalive/startup scripts, introspection levels and broader session controls remain tracked. Verification for this batch is recorded in `evidence/datagrip-features-qa.json`.

## Core database workflows

| Area | Implemented | Verification and remaining limits |
|---|---|---|
| Connections | PostgreSQL, SQLite, MySQL native adapters; general profiles, test, reconnect-on-Run, typed diagnostics, TLS CA/hostname checks, explicit Keychain | Owned PG18.6 and MySQL8.4.11 live tests; SQLite native tests. PG UI failure/edit/retry verified. Keychain fake UUID native roundtrip passed. SSH/jump hosts absent. |
| Settings and CLI | Password-free atomic JSON, shared process lock and partial-update API; CLI profile/config/query/schema/format/export | Concurrent CLI subprocess changes preserved. GUI uses ordered background partial mutations; a concurrent CLI addition survived a GUI preference save in native QA. Shared Korean/English Preferences, editor font/tab width, table page-size default, recovery controls and CLI language get/set are integrated; native runtime switching and Korean restart persistence passed. Tools and table-view controls now share the catalog; raw DB errors, technical tokens and some shared components retain English. Tools runtime switching preserved an existing form value; table-view switching preserved page2 and search text in native QA. PG/MySQL CLI JSON/CSV live tests passed; successful SQL remains reported if history persistence fails. |
| Transactions | Auto/Manual policy separate from actual server state, explicit commit/rollback, targeted cancellation and no replay | PG ReadyForQuery I/T/E observed through minimal driver patch; native UI Idle/Active/Failed/rollback verified. MySQL passive OK/EOF flags, ERR conservatively Unknown; no internal diagnostic-overwriting query. |
| SQL documents | Separate editor/undo/result ownership, file open/save, dirty-close prompts, current/selected/script execution, formatting, explicit completion, bounded searchable history | Native tab retention and close cancellation tested. Opt-in restart restoration of two exact dirty buffers and active tab passed native QA without automatic connection/query. Dedicated lazy sessions and connection/configured-database selectors passed native A/B transaction/cancel isolation and disconnected target restoration. Explicit completion supports caret placement, Up/Down, Enter/Tab, Esc, undo, Cmd+J and optional Ctrl+Space; current-console metadata refresh is explicit. Typing-triggered completion and parameter prompts remain pending. |
| Result data | Bounded retained results, virtual grid, multiple result pages, NULL/empty distinction, selected-cell copy, selected snapshot CSV/JSON | PG and SQLite native display verified. The PostgreSQL read-only table viewer paged an owned 105-row fixture as 100+5 and highlighted a Find match. Opening it from a different database branch preserved the existing SQL console target and query; its separate reader passed live transaction-isolation checks. Generic query result-set selection is separate. PostgreSQL table tabs support typed AND filters, raw-column server sort with verified identity tie-breaks, and50/100/250/500-row pages. Multiple retained table tabs share one sequential reader; SQL sessions remain separate. Read-only value/record inspection, lossless JSON formatting and bounded row/column/explicit-range TSV copy now work on retained pages. Normal Run remains bounded; separate read-only live/cursor views and CSV streaming are available. Generic server sort for arbitrary SQL remains pending. |
| Catalog metadata | Additive scoped schema/object/column probes with time/row/byte limits; 16 MiB accounted-payload cache foundation with leased snapshot accounting | SQLite scoped tests and PG/MySQL cross-database/same-name fixtures passed. Lazy connection/database/schema/object/field navigation and scoped cache admission are wired into the host. Native PG/SQLite/MySQL hierarchy, fields, expired-snapshot markers and loaded-name search passed. At most two probes run, with a sixteen-request queue; metadata connect and query each have separate four-second budgets. Private versioned JSON shards persist basic metadata plus typed index/constraint/view details and are read only for requested branches. Disk-restored snapshots are explicitly historical; private writer locking protects the128MiB indexed-payload/2048-shard budget. CLI catalog list/show does not connect to databases. No claim of full background indexing or a process RSS cap. |
| Row changes | Typed PG bound DML plans, DEFAULT/generated values, identity/metadata checks, optimistic predicates, batch/savepoint behavior | Core type/conflict tests passed, but a verified namespace-rebinding race remains. GUI Apply is disabled pending M2 acceptance. Read-only data viewing remains available; no claim of safe general row editing. |
| Schema tools | PG bounded read-only capture, semantic fingerprints, constrained migration preview, ER SVG, plan-only EXPLAIN, data comparison | Baseline/rich catalog live captures passed. Objects panel and index/view SQL drafts implemented. Exact view capture avoids scanning unrelated objects; native table index details and view definition passed. Broader tool workflow acceptance remains pending. Migration apply and EXPLAIN ANALYZE absent. |
| AI and MCP | Opt-in Codex app-server WS client, owned authenticated launcher, declarative extension backend, read-only MCP stdio | WS mocks, restricted metadata probe, and one real ChatGPT-authenticated gpt-6.1-sol turn passed (synthetic SELECT only, no SQL execution). Components preserved; main AI UI integration deferred until basic milestones pass. Owned test server stopped. MCP boundary review and benign live regressions passed. |

Detailed contracts and commands: [profiles](PROFILES.md), [CLI](CLI.md),
[PG](PG-SPIKE.md), [SQLite](SQLITE-ADAPTER.md), [MySQL](MYSQL-ADAPTER.md),
[data](DATA-FEATURES.md), [workflows](WORKFLOWS.md), [tools UI](TOOLS-UI.md),
[assistant](AI-ASSISTANT.md), [MCP](MCP-SERVER.md).

## Verification entry points

```sh
bash scripts/verify-current.sh
# Explicitly opts into already-running project-owned fixtures and their test data:
bash scripts/verify-current.sh --owned-live
bash scripts/package-ui.sh
uv run --python '>=3.14' scripts/measure-ui.py
```

The live verifier requires baseline/rich catalog seed data and the `consoles` second
database seed for PostgreSQL/MySQL; see [fixture setup](FIXTURES.md). Ignored tests
are not counted as passing live tests. Native
Keychain, GUI interaction, physical IME, accessibility and comparative performance
remain separate checks. The [console/locale checkpoint](../evidence/console-locale-validation.json)
passed332 default workspace tests,57 separately opted-in live tests and5 vendor
wire tests. A subsequent UI-only capacity-feedback refinement passed58 UI tests
and native capacity QA. These rechecks are not added to the workspace total. Test counts are dated observations,
not coverage percentages.

The later [navigator/locale checkpoint](../evidence/navigator-locale-validation.json)
passed358 initial workspace tests/62 ignored, then81 touched UI/i18n tests/3 ignored,
plus three separately selected read-only live checks. These rechecks are not summed.

The [workspace polish checkpoint](../evidence/workspace-polish-validation.json)
passed393 workspace tests/62 ignored and a subsequent90 UI tests/3 ignored.
Native keyboard completion, cross-database table tabs, typed filters/sort, persisted
layout/settings and disconnected basic/detail metadata browsing are recorded in
[workspace QA](../evidence/ui-workspace-polish-qa.json). Rechecks are not summed.

Current native observations are in
[console/locale QA](../evidence/ui-console-locale-qa.json),
[core/recovery QA](../evidence/ui-native-core-recovery-qa.json),
[lazy navigator/Tools locale QA](../evidence/ui-navigator-locale-qa.json) and earlier
[phase3 QA](../evidence/ui-native-phase3-qa.json). They identify the exact tested
binary and fixes awaiting retest. Historical [M0 results](M0-RESULTS.md) describe
their recorded binaries, including now-replaced transaction limitations.

[SQL file QA](../evidence/sql-file-qa.json) records the later file/drop build:
external-folder Open, uppercase extension, canonical-path tab reuse, dirty-tab
preservation and non-SQL diagnostics passed natively. The root `ExternalPaths`
drop hook uses the same tested loader. Physical Finder-to-window dragging remains
unverified because the automation surface exposed Finder only as a Stage Manager
thumbnail. Current limits: 16 files per open/drop, 32 tabs, 64 KiB UTF-8 per file;
an initial UTF-8 BOM is removed, CRLF preserved, saves use plain UTF-8.

## Lightweight behavior and outstanding work

The app retains bounded query snapshots and virtualizes rendered cells. This is
not yet a complete streaming/page cache. A small retained snapshot does not bound
the driver's transient allocation for a single very large field. The benchmark
plan remains [BENCHMARKS.md](BENCHMARKS.md); no equivalent DataGrip performance
comparison has been completed. Startup root-render instrumentation is not first
paint or input responsiveness.

Continue the integrated per-console, hierarchical catalog and Korean/English workflows,
then extend typing-triggered completion, broader engine data views and complete keyboard/accessibility acceptance.
Follow with row-edit correctness, server filtering/sorting, index/view editing,
import mapping/application, and wider engines and advanced functions. SQL Server, Oracle, MongoDB, Redis, cloud discovery,
debugging, release/update and cross-platform acceptance remain tracked work.
