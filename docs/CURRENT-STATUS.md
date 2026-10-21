# Current implementation and remaining acceptance

Updated 2026-10-22. This supersedes historical M0 limitations only where explicitly
listed below. It does not declare DataGrip parity or any entire M1–M6 phase complete.
The [42 feature groups](FEATURE-MATRIX.md) remain the full backlog.
**Priority reset:** [M0/M1 acceptance](M0-M1-ACCEPTANCE.md) comes first. M3–M6 remains authorized follow-on work; the core UX and
session correctness gaps are being addressed before wider integration. PostgreSQL row Apply remains disabled because namespace
rebinding acceptance has not passed; model/batch tests do not override that blocker.

## Core database workflows

| Area | Implemented | Verification and remaining limits |
|---|---|---|
| Connections | PostgreSQL, SQLite, MySQL native adapters; general profiles, test, reconnect-on-Run, typed diagnostics, TLS CA/hostname checks, explicit Keychain | Owned PG18.6 and MySQL8.4.11 live tests; SQLite native tests. PG UI failure/edit/retry verified. Keychain fake UUID native roundtrip passed. SSH/jump hosts absent. |
| Settings and CLI | Password-free atomic JSON, shared process lock and partial-update API; CLI profile/config/query/schema/format/export | Concurrent CLI subprocess changes preserved. GUI uses ordered background partial mutations; a concurrent CLI addition survived a GUI preference save in native QA. Shared Korean/English Preferences and CLI language get/set are integrated; native runtime switching and Korean restart persistence passed. Tools and table-view controls now share the catalog; raw DB errors, technical tokens and some shared components retain English. Tools runtime switching preserved an existing form value; table-view switching preserved page2 and search text in native QA. PG/MySQL CLI JSON/CSV live tests passed; successful SQL remains reported if history persistence fails. |
| Transactions | Auto/Manual policy separate from actual server state, explicit commit/rollback, targeted cancellation and no replay | PG ReadyForQuery I/T/E observed through minimal driver patch; native UI Idle/Active/Failed/rollback verified. MySQL passive OK/EOF flags, ERR conservatively Unknown; no internal diagnostic-overwriting query. |
| SQL documents | Separate editor/undo/result ownership, file open/save, dirty-close prompts, current/script execution, formatting, explicit completion, bounded history | Native tab retention and close cancellation tested. Opt-in restart restoration of two exact dirty buffers and active tab passed native QA without automatic connection/query. Dedicated lazy sessions and connection/configured-database selectors passed native A/B transaction/cancel isolation and disconnected target restoration. Automatic completion and parameter prompts remain pending. |
| Result data | Bounded retained results, virtual grid, multiple result pages, NULL/empty distinction, selected-cell copy, selected snapshot CSV/JSON | PG and SQLite native display verified. The PostgreSQL read-only table viewer paged an owned 105-row fixture as 100+5 and highlighted a Find match. Opening it from a different database branch preserved the existing SQL console target and query; its separate reader passed live transaction-isolation checks. Generic query result-set selection is separate. Progressive fetch, full-stream export, server filter/sort and range copy remain pending. |
| Catalog metadata | Additive scoped schema/object/column probes with time/row/byte limits; 16 MiB accounted-payload cache foundation with leased snapshot accounting | SQLite scoped tests and PG/MySQL cross-database/same-name fixtures passed. Lazy connection/database/schema/object/field navigation and scoped cache admission are wired into the host. Native PG/SQLite/MySQL hierarchy, fields, expired-snapshot markers and loaded-name search passed. At most two probes run, with a sixteen-request queue; metadata connect and query each have separate four-second budgets. No claim of full indexing or a process RSS cap. |
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
then connect current-console metadata freshness to completion and expand keyboard acceptance.
Follow with row-edit correctness, server filtering/sorting, index/view editing,
import mapping/application, and wider engines and advanced functions. SQL Server, Oracle, MongoDB, Redis, cloud discovery,
debugging, release/update and cross-platform acceptance remain tracked work.
