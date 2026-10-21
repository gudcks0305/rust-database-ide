# Native SQL workbench

2026-10-21: Rust/GPUI database workbench with local connection profiles,
SQL editing, database object discovery, query history and bounded result grids.
This remains an M0/M1 implementation experiment; no DataGrip parity or measured
performance improvement is claimed.

## Run and isolation

```sh
cargo run --release -p dbide-ui-spike
# App-owned isolated profile location for native QA (not a HOME override):
DBIDE_PROFILE_STORE="$PWD/tmp/ui-qa/profiles.json" target/release/dbide-ui-spike
# No connection or SQL is dispatched during normal startup.
target/release/dbide-ui-spike --smoke-seconds=5
# Explicit project-owned PostgreSQL fixture only:
target/release/dbide-ui-spike --fixture-smoke --smoke-seconds=10
cargo test -p dbide-ui-spike --lib
```

The fixture smoke uses the project's disposable loopback PostgreSQL fixture.
It does not discover user credentials, load keychain entries or connect to a
saved profile. Inspect emitted query outcome/retained rows/error before treating
smoke termination as a successful query.

## Connection workflow

The default view is an empty workbench with a connection settings drawer when
no profile is selected. Choose **PostgreSQL**, **SQLite** or **MySQL**. Network
engines expose name/host/port/database/username, session password, TLS mode,
optional CA path and timeout, then **Test**, **Save**, or **Save & connect**.
Pristine network port defaults follow PostgreSQL 5432 / MySQL 3306; once the
user edits the port, engine changes preserve it even if it was restored to the
default value. Switching through SQLite does not erase the network draft.

SQLite instead exposes a database path, native **Open file…** / **New path…**
choosers and an unchecked-by-default **Allow file creation on Test / Connect**
choice. Choosing a path never creates a file. Test/Connect can create a missing
file only when that explicit profile choice is enabled. Authentication, Keychain
and TLS inputs are hidden and omitted from SQLite requests. **Open SQLite
read-only** enforces the adapter's read-only file mode and disables creation.
Network **Read-only session default** is advisory, and the form explicitly says
that database privileges remain the authorization boundary. No absolute
permission or immutable read-only claim is made for PostgreSQL/MySQL. Port, timeout and profile validation errors
remain in the drawer. Saved profiles appear in the left sidebar. Selecting a
profile does not connect. **Edit** changes it; **Remove profile** offers Undo.

Profiles contain no passwords. Password inputs use masked native `InputState`
and the password semantic role. Passwords stay in the process session by default.
The unchecked **Remember password in Keychain on Save** choice writes the exact
profile's password only after explicit Save. **Load Keychain** reads only that
profile ID; **Forget stored password** explicitly removes its Keychain entry.
There is no automatic Keychain read/write on startup or profile selection. A
load error leaves the input available for manual entry. Unchecking remember
never silently removes a previously stored entry. Removing a profile also does
not silently delete its credential; forget it explicitly first if desired. New profiles default
to Verify full. Both Require and Verify full check certificate trust and
hostname in this implementation; Require does not weaken verification. The local fixture explicitly uses
TLS Disable. See the profile/driver tests for storage and TLS contracts.

Normal startup only reads this app's local profile file. `DBIDE_PROFILE_STORE`
selects an explicit app-owned file for QA; absent it the ProfileStore default
location is used. Load/save failures appear as persistent diagnostics. They are
not silently treated as successfully saved profiles.

Connection failures display a persistent Alert panel with error category,
exact host/port/database target, safe message, remediation, Retry connection,
Test connection and Edit settings. Errors remain visible until a new explicit
connection/query/test attempt succeeds or starts. A failed connection never
produces a Connected badge.

## Editor and execution

The SQL editor supports syntax highlighting, line numbers, undo/selection,
clipboard and native marked-text input. **Run** / Cmd+Enter submits the current
text. When a profile is configured but disconnected the button reads
**Connect & Run**: the bridge opens the configured session before dispatching
that single SQL request. It does not rerun a failed SQL statement. Editing SQL
alone never executes it. Pending connection/query/cancel work disables duplicate
Run submissions. Blank SQL and missing profiles disable Run.

The UI uses nonblocking bridge submissions and polls at most 32 events every
40 ms. Database network work runs on a private Tokio worker. Connection,
configuration, policy, execution and schema responses are matched to operation
IDs. The active profile/connection identity changes only after the exact worker
Configured acknowledgement. Run is disabled while activation is pending; a
rejected activation retains the prior active identity. Changing profiles or
applying profile edits is blocked while wire state is Active/Failed until
Commit, Rollback or explicit Disconnect. Switching profiles invalidates previous
schema and SQL-assistance snapshots. Cancellation captures the
immutable execution key, or submit operation before the first execution event.
A delayed cancellation acknowledgement for an old execution cannot change the
currently displayed execution state. A terminal report, including connection
retirement, determines whether subsequent Run reconnects.

Each SQL tab owns an independent native `EditorState`, undo stack, dirty marker,
revision and result snapshot. **New** and **Open…** create new tabs and preserve
existing unsaved tabs. Open accepts up to 16 UTF-8 SQL files, 64 KiB each.
**Save as…** saves the exact selected-tab snapshot through a native file panel;
later edits remain dirty even if an older snapshot finishes saving. Tab close
offers Save, Keep open or Discard changes. Window close/Cmd+Q also warns about
unsaved tabs, open transactions or active execution. File I/O runs on the
background executor and publishes successful saves atomically. **Save** writes
the retained canonical path; **Save as…** chooses a new path. Cmd+O opens,
Cmd+S saves, Cmd+Shift+S saves as, Cmd+N/Cmd+T creates a tab, and Cmd+W performs
dirty-aware tab close, including while an editor input is focused.

External `.sql` file drops use the same loader as Open. The extension is
case-insensitive, directories/devices/FIFOs are rejected, UTF-8 is required and
the open descriptor is limited to 64 KiB even if the file grows after stat.
Symlinks intentionally resolve to a canonical regular file; a replacement final
symlink is refused on Unix. Existing canonical files focus their current tabs
without replacing dirty buffers. Up to 16 files are processed per open/drop;
rejections are shown per file and no connection/SQL is dispatched. UTF-8 BOM is
decoded on load; CRLF remains intact; saves use plain UTF-8. Native Open, Unicode,
uppercase extension, canonical deduplication and dirty-buffer preservation were
verified by the coordinator. Physical cross-application Finder drag remains
unverified due the native automation foreground-window limitation; hook/source
and eight loader tests are separate evidence. Tabs currently
remain in memory until explicitly saved; automatic session restoration is not
implemented.

Tabs explicitly share the selected connection, whose actual name/host/database
appears in the transaction bar. Each submitted query captures its originating
tab ID/title and active profile/target. Terminal results are installed only in
that tab's snapshot, even if the user changes tabs while it runs. Result labels
identify that captured origin. Query concurrency remains one bridge execution.

**Run current** / Cmd+Shift+Enter asks `dbide-sql::statement_at_cursor` for a safe
boundary at the editor's UTF-8 byte cursor. Unsupported/uncertain boundaries show
an error and dispatch no SQL; there is no whole-script fallback. **Run script** /
Cmd+Enter remains explicit whole-buffer execution. The initial sample is the
engine-neutral `SELECT 1 AS result`. **Format** / Cmd+Shift+F
preserves undo and refuses commented/unsupported input rather than dropping
comments. **Complete** / Cmd+Space opens a floating completion list backed by
current schema table/column names and scope; clicking a candidate applies only
the matching tab/revision/source snapshot. SQL assistance lists catalog names
and diagnostics. Alias/CTE rename requires an explicit preview and Apply;
application checks document identity, revision and source hash. It never renames
catalog objects or executes DDL. Supported SELECT/CTE analysis remains narrower
than arbitrary server SQL; analysis does not gate Run script.

## Explorer, results and export

After connecting, **Refresh** loads the selected engine's schema/table/view list.
Clicking an object opens a safely identifier-quoted `SELECT * ... LIMIT 100` in
a new editor tab; it does not execute that statement. PostgreSQL/SQLite use
doubled double quotes and MySQL uses doubled backticks. Formatting, current
statement selection, completion, analysis and rename use the actual selected
connection's dialect. Profile, result-origin and shared-session labels show the
engine plus network target or SQLite file path. The current explorer shows object names,
kinds and column counts. Hierarchical column browsing, DDL and semantic search
are separate remaining features. Schema limits are reported in status.

Results preserve script order, including each rowset and command count.
Previous/next controls select individual results. SQL NULL and empty text remain
distinct in the model. Error/notice/retained-byte/truncation/transaction-state
information remains visible after partial failure or retention limits. The
bridge currently returns bounded terminal reports; it does not stream row
batches or establish first-row streaming latency.

**Export CSV…** and **JSON…** export the selected retained rowset through
`dbide-data` on a background executor. CSV uses its explicit `\N` NULL marker;
empty text remains text. Literal text equal to that marker is rejected even
when quoted, preventing silent NULL conversion. The diagnostic recommends JSON
for such values. Typed JSON preserves the exact driver-provided Text/Null model
without numeric/date inference. Exports use finite row/byte budgets and an
atomic temporary-file publication; a failed write preserves an existing target.
Export reflects the retained snapshot, including visible truncation, and does
not issue another database query. Command-only pages and the synthetic stress
source cannot be exported as query results.

**Import preview…** reads CSV or the typed JSON format into a separate preview
panel without any database writes. CSV expects a header and the explicit `\N`
NULL contract, so files authored elsewhere require that same convention.
Preview is bounded to 1,000 rows / 4 MiB, and displays its columns, count,
truncation and first 20 rows. JSON exceeds the row budget with an error rather
than silently truncating typed input. No import-submit control is provided.

## Basic explorer and table viewing (2026-10-22)

The default sidebar reserves most of its height for a schema → table/view →
column tree. Connection rows are compact and their list scrolls independently.
Search matches loaded schema/object/column names and type names. The explorer
shows loaded object/column counts and explicitly marks capped metadata subsets;
it does not silently hide a further UI-only 500-object subset. Every field has
an accessibility label containing name, full type, NULL, PK/FK and default.

After an explicit Connect or Connect & Run, metadata loads once automatically.
Normal startup still opens no database connection and submits no SQL. Table
clicks expand fields. **Structure** opens a compact field grid (Name, Type,
NULL, Key, Default/extra), horizontally scrollable for complete values, with
**Copy fields**. PostgreSQL adds live FK targets, indexes and view definitions
through a separate bounded read-only catalog capture. Other adapters explicitly
state when relationship/index details were not loaded.

**SQL** opens a quoted SELECT draft without executing it. **Data** is an explicit
read gesture: ordinary PostgreSQL tables use the read-only paged viewer; other
engines and views open a bounded quoted SELECT result. PostgreSQL metadata that
cannot support the paged viewer falls back to that same read-only query workflow,
without relaxing editing identity/type guards. This never executes opened or
dropped SQL files. The paged viewer reads at most100 rows per page, reports total
unknown, and labels Find as client search of the loaded page. Concurrent changes
can shift pages. Its current full-screen presentation closes back to the SQL
tabs/explorer; embedding data as another workbench tab is pending.

Row-edit staging/model code remains experimental M2 work. **All row editing
controls are hidden and Apply is blocked at both the capture and host-dispatch
boundaries in this reviewable M0/M1 release.** Namespace rebinding and prepared
portal validation remain unmet gates; no GUI row-write acceptance is claimed.
Metadata/OID/fingerprint checks alone are not a global side-effect sandbox.

Native macOS App/File/Query menus expose the same New/Open/Save/Save As/Close,
Run/current-statement/Cancel/Format/Complete and guarded Quit actions. Run and
Cancel follow current availability. Cold start and recovery focus the active
editor (a new-profile form focuses its name field), with deferred app-level
fallback handlers when the window has no focused editor. These handlers reuse
normal recovery/transaction close checks; menu Quit does not bypass them.

## Profile interoperation and SQL recovery

Every GUI profile mutation now uses `ProfileStore::update` with one bounded
upsert/remove/select/history/preference change. A single background queue orders
file locking/fsync work; the UI never writes its stale whole profile list.
Successful acknowledgments merge fresh CLI profiles and retain session-only
history. Profile Save activates its captured settings only after persistence
succeeds. History tabs retain their originating profile ID: Run is disabled if
the selected profile differs or was removed. Opening history never runs SQL.

**Recovery: Off/On** is an explicit, separate opt-in from persisted SQL history.
When enabled, SQL drafts, order, active tab, exact saved/dirty baselines, file
paths and history-profile requirements are saved locally to `workspace.json`
next to the selected profile store, using the bounded private0600 workspace
store. `editor-preferences.json` records the opt-in. These files can contain
sensitive SQL values. Disabling stops recovery/restoration and retains the prior
snapshot; it does not silently delete it. Autosaves debounce for900ms and run
one writer in the background. Normal close flushes the latest snapshot before
removing the window; validation/I/O failure keeps the window open with feedback.
Individual dirty tab Close still offers Save/Keep/Discard. Database execution
and open/unknown-live transaction close confirmation remain separate.

Restoration never follows SQL file paths, opens a connection, loads a password,
or runs SQL. Paths are metadata only, so a changed/missing original file cannot
replace recovered SQL or its dirty baseline. Malformed/newer/oversize recovery
files are preserved and recovery remains blocked until explicit local repair.
After a runtime recovery failure, the recovery control can disable it for the
current window without overwriting the failing files. Dock reopening creates a
new window when none remain. Native recovery/close/reopen acceptance must be
recorded separately from pure model/store tests.

## Transaction controls

Auto/Manual reflects the worker's acknowledged commit policy, and **Wire**
shows actual PostgreSQL ReadyForQuery state: Idle, Active, Failed or Unknown.
Unknown is honest while disconnected/retired or before the first report/policy
acknowledgement. Manual begins the next ordinary query only when wire state is
Idle. Direct SQL transaction statements are observed by the driver. Policy
changes are disabled while an execution or open transaction is active.

**Commit** requires an existing idle bridge session with Active wire state;
**Rollback** also accepts Failed or Unknown on a live connection. Unknown on a
live MySQL session may conceal an open manual transaction; profile changes,
policy changes and window close therefore require resolution/confirmation.
Disconnected startup Unknown does not imply a transaction. Fresh explicit
Connect observes the native state through an idempotent commit-policy
acknowledgement, without executing SQL or assuming Idle. Neither reconnects, replays SQL nor begins a
new transaction. Failed transactions disable Commit and require Rollback.
Reports retain wire state. Profile activation/removal is blocked in Active or
Failed state; explicit Disconnect is the visible path for discarding the
session. Dedicated commit/rollback reports retain the originating tab identity.

## History and secondary tools

**History** shows completed SQL requests, outcomes and retained row counts.
Clicking an entry opens its SQL in a new tab without executing. History is
session-only by default. **Save history locally** explicitly enables persistence;
the panel states that SQL may contain sensitive values. Turning persistence off
removes history from the saved workspace while retaining current session items.
Profile storage applies its own validated bounds and atomic save contract.

**Tools** opens the integrated `dbide-tools-ui` workbench with a captured copy
of the active profile/credential request and SQL. Its header identifies that
fixed target; it does not retarget when another profile is selected later.
The parent SQL editor and Run handlers are unmounted while the Tools view is
visible, and **Close tools** returns to the existing documents and results.
The component exposes scoped catalog snapshots, migration/ER/EXPLAIN and data
comparison workflows according to its own engine capabilities. Current live
catalog/EXPLAIN workflows are PostgreSQL-only; offline snapshot/data workflows
remain available for other engines or no profile. Tools does not execute a
migration or import merely by opening a preview.

**Advanced** contains the opt-in logical 1,000,000 × 200 stress source, Origin /
Last cell, redraw probe and metrics. Stress mode is no longer the default product
view. It generates only requested cells and retains 200 column descriptors,
not a million-row collection. The same virtual DataTable renders real retained
PostgreSQL snapshots with pinned first column and both-axis virtualization.

## Accessibility and measurement limits

GPUI `0.3.9` supplies AccessKit on macOS. Controls use GPUI Component Button's
native semantic/disabled behavior; inputs have visible labels and accessible
names. The SQL editor is labelled multiline input. Grid/header/row/cell roles,
counts, indices and selected values are supplied only for rendered cells.
Status and diagnostic nodes have Status/Alert roles. These APIs and compilation
do not establish full VoiceOver, keyboard or IME conformance. Native QA must
check Korean composition, candidate placement, undo, control activation,
selection/copy and offscreen grid traversal on this binary.

**Metrics** / Cmd+Shift+M emits JSON. The redraw probe is opt-in and retains at
most 10,000 intervals. `--smoke-seconds=N` emits metrics and quits. Root timing
is main-entry to root construction, not first GPU paint or interactive latency.
Rendered ranges include partial/overscan work; scrolling columns are normalized
after fixed-column offset. Callback counts are cumulative. No RSS/p95/latency
or relative DataGrip claim follows from these counters. See `BENCHMARKS.md`.

## Dependency/source record and verification

Rust toolchain 1.99.1; exact GPUI pins are `gpui-pre`/`gpui-pre-platform` 0.3.9,
`gpui-component`/`gpui-kit-assets` 0.8.0 (Apache-2.0). Runtime Metal shader
compilation avoids installing the separate Xcode Metal compiler. SQL highlighting
uses syntect to avoid the parser spike's conflicting Tree-sitter links version.
No GPL Zed editor or DataGrip code/assets are included. Native C/Objective-C/
Metal platform dependencies remain; this is not a transitive pure-Rust claim.

Primary source APIs were inspected locally from the exact pinned crates:
[GPUI](https://docs.rs/gpui-pre/0.3.9/gpui/),
[Component](https://docs.rs/gpui-component/0.8.0/gpui_component/),
[Input](https://docs.rs/gpui-component/0.8.0/gpui_component/input/),
[Table](https://docs.rs/gpui-component/0.8.0/gpui_component/table/).

The UI unit suite has 35 tests: virtual range edges, fixed-column translation,
far-corner generation, ordered multi-result/NULL/partial-error/unknown command
count conversion, exact Text/NULL transfer and literal marker collision safety,
atomic failed-write preservation, disconnected/duplicate Run admission,
matching/stale metadata settlement, tab ownership, dirty-save snapshots,
rename identity/revision/hash, explicit remember with an in-memory credential
store, active profile identity after matching configuration acknowledgement,
pristine/custom engine ports, dialect/identifier quoting, and SQLite/MySQL form
projection with password-free profile serialization round trips, duplicate grid
header identities, closed table metadata/row projection, stale execution,
multiline TEXT/JSON preservation, DEFAULT insertion, incoming ON UPDATE CASCADE,
nonfinite/BCE temporal projection and unknown-live transaction guards.
No test invokes native Keychain. Native credential integration remains an
explicit separately opted-in test in the credential crate.

`cargo check -p dbide-ui-spike`, `cargo clippy -p dbide-ui-spike --all-targets -- -D warnings`, and `cargo build --release -p dbide-ui-spike` are the UI compile
checks. Native QA, fixture execution and packaging belong to the coordinator;
this implementation worker does not launch the app. The preceding UI revision
passed coordinator native save/bad-port diagnostics/edit/reconnect/query/schema
QA; those observations do not establish native behavior for these new controls.
Upstream `block` 0.1.6's future-incompatibility warning remains.
