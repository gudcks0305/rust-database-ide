# Current implementation and remaining acceptance

Updated 2026-10-21. This supersedes historical M0 limitations only where explicitly
listed below. It does not declare DataGrip parity or any entire M1–M6 phase complete.
The [42 feature groups](FEATURE-MATRIX.md) remain the full backlog.
**Priority reset:** [M0/M1 acceptance](M0-M1-ACCEPTANCE.md) comes first. New M3–M6
expansion is frozen. PostgreSQL row Apply remains disabled because namespace
rebinding acceptance has not passed; model/batch tests do not override that blocker.

## Core database workflows

| Area | Implemented | Verification and remaining limits |
|---|---|---|
| Connections | PostgreSQL, SQLite, MySQL native adapters; general profiles, test, reconnect-on-Run, typed diagnostics, TLS CA/hostname checks, explicit Keychain | Owned PG18.6 and MySQL8.4.11 live tests; SQLite native tests. PG UI failure/edit/retry verified. Keychain fake UUID native roundtrip passed. SSH/jump hosts absent. |
| Settings and CLI | Password-free atomic JSON, shared process lock and partial-update API; CLI profile/config/query/schema/format/export | Concurrent CLI subprocess changes preserved. GUI migration to partial mutations remains in progress. PG/MySQL CLI JSON/CSV live tests passed; successful SQL remains reported if history persistence fails. |
| Transactions | Auto/Manual policy separate from actual server state, explicit commit/rollback, targeted cancellation and no replay | PG ReadyForQuery I/T/E observed through minimal driver patch; native UI Idle/Active/Failed/rollback verified. MySQL passive OK/EOF flags, ERR conservatively Unknown; no internal diagnostic-overwriting query. |
| SQL documents | Separate editor/undo/result ownership, file open/save, dirty-close prompts, current/script execution, formatting, explicit completion, bounded history | Native tab retention and close cancellation tested. Session/restart restoration, automatic completion, independent session per tab and parameter prompts remain missing. All current tabs explicitly share one selected physical session. |
| Result data | Bounded retained results, virtual grid, multiple result pages, NULL/empty distinction, selected-cell copy, selected snapshot CSV/JSON | PG and SQLite native display verified. Previous/next controls select result sets, not row pages. Progressive fetch, full-stream export, grid search/filter/sort/range copy remain missing. |
| Row changes | Typed PG bound DML plans, DEFAULT/generated values, identity/metadata checks, optimistic predicates, batch/savepoint behavior | Core type/conflict tests passed, but a verified namespace-rebinding race remains. GUI Apply is disabled pending M2 acceptance. Read-only data viewing remains available; no claim of safe general row editing. |
| Schema tools | PG bounded read-only capture, semantic fingerprints, constrained migration preview, ER SVG, plan-only EXPLAIN, data comparison | Baseline/rich catalog live captures passed. Objects panel and index/view SQL drafts implemented; native interaction acceptance pending. Migration apply and EXPLAIN ANALYZE absent. |
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

The live verifier requires both baseline and rich catalog seed data; see the fixture
setup documentation. Ignored tests are not counted as passing live tests. Native
Keychain, GUI interaction, physical IME, accessibility and comparative performance
remain separate checks. The [integrated checkpoint](../evidence/current-validation.json)
passed 247 default workspace tests, 37 separately opted-in live tests, and 5 vendor
wire tests. The later row editor/recovery/AI UI integration still requires a new
whole-workspace run before final acceptance. Test counts are dated observations,
not coverage percentages.

Current native observations are in
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

Prioritize row editing and conflict UX, real paging/filter/sort, workspace recovery,
catalog navigation and index/view editing, import mapping/application, then wider
engines and advanced functions. SQL Server, Oracle, MongoDB, Redis, cloud discovery,
debugging, release/update and cross-platform acceptance remain tracked work.
