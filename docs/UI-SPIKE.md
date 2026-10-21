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
and eight loader tests are separate evidence. Optional SQL draft recovery is
implemented and described below; it restores buffers, not live sessions.

Each SQL tab owns a dedicated lazy `ConsoleSession`/bridge and immutable route
(console ID plus generation). Connection and configured-database selectors act
only on that tab. Each submitted query captures its owner and password-free
execution target; inactive tabs receive their own terminal results and history.
Polling is bounded per UI turn. At most eight console workers are admitted;
idle sessions are never silently evicted because temporary objects and session
settings may remain meaningful.

Target changes require no pending operation and a known Idle/disconnected state.
Active, Failed and live Unknown transactions must be resolved or explicitly
disconnected. Rebinding visibly resets the new session's policy to Auto. Browse
profile selection does not retarget an existing console. PostgreSQL/MySQL database
selection opens a new dedicated connection; SQLite identifies its configured
file as `main`. The displayed database is the configured initial target: raw
MySQL `USE`/session setting changes are not tracked by this selector.

Native QA verified A's temporary table and Manual transaction surviving B's
switch to another database, execution and cancellation. A Rollback changed its
visible row count from two to one. B's next Run reconnected only B. See
[console/locale evidence](../evidence/ui-console-locale-qa.json).

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

The navigator loads connection → database → schema → table/view → column
branches only when expanded. PostgreSQL uses the schema level; MySQL databases
lead directly to their objects, and a SQLite file exposes `main` without a fake
network database layer. Browsing never binds or reconfigures an existing SQL
console. **SQL** opens a quoted `SELECT * ... LIMIT 100` draft; **Data** explicitly
runs that bounded read under the branch's captured connection/database in a new
independent console. PostgreSQL/SQLite quote doubled double quotes and MySQL
quotes doubled backticks. Formatting and SQL assistance use each console's
configured target/dialect. MySQL `USE` and arbitrary session SQL can change a
server default without updating that configured-target label.

Branches show returned counts, snapshot age, expiry after60 seconds, errors and
partial/truncated flags. Refresh invalidates that scope's subtree; connection
refresh advances its generation. Search filters only expanded snapshots and
never requests metadata. At most1,000 matching tree rows render, with an
explicit subset notice. Collapse other branches or narrow the filter to inspect
further loaded rows. A partial snapshot is never called complete indexing.

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

The sidebar uses one connection/database/object tree, with compact connection
roots and a per-connection settings action. No duplicate saved-profile card list
consumes its height. Every loaded field has an accessibility label containing
its name, full type, NULL, PK/FK, default and generated/identity marker.

Normal startup creates no metadata worker, opens no database connection and
submits no SQL. SQL consoles retain their existing bounded completion catalog
capture; the navigator does not eagerly load every database or column on
Connect. An explicit legacy browser Connect may expand its database list once.
SQLite metadata probes enforce read-only/no-create at the bridge boundary,
including profiles that allow explicit connection-time creation. Navigator
probes use only credentials from the exact profile ID + endpoint
fingerprint; an explicit password in a matching console can be reused without
writing Keychain or silently authorizing a different host/user/TLS endpoint.

**Structure** holds the column cache lease and displays the compact field grid
(Name, Type, NULL, Key, Default/extra), horizontal overflow, and **Copy fields**.
An explicit PostgreSQL detail action requests indexes, FK targets and view
definitions in the same two-slot navigator queue. These are separate point-in-time
snapshots; their capture times can differ from the basic columns. Detail capture
is read-only and targets exactly one table or view; a view uses one catalog
SELECT without unrelated table/column scans. Capture is capped at1 MiB/4,096
records/2,048 columns; connection and query/workflow deadlines are each at most
4 seconds. Unsupported table kinds, insufficient permissions or capture limits
keep basic fields visible with a detail error. Other engines explicitly
leave these advanced details unloaded.

**Data** on an ordinary PostgreSQL table now prepares its captured database in
an isolated table-reader bridge and reuses the existing100-row paged viewer.
The pending intent checks navigator generation, full captured profile/database,
and the exact Configure → Connect → native Idle observation acknowledgements.
It never rebinds a SQL document or changes saved profile defaults/selection.
A visible captured-target banner reports preparation, errors and cancellation;
Retry is explicit. Cancel/Disconnect/Rollback controls own only the table reader.
A rejected Configure keeps the previous native table session; unknown execution
outcomes keep a potentially live transaction Unknown until explicit resolution.
Stale/closed/cancelled intents cannot later open or submit table SQL. An open or
unknown table transaction blocks changing its target. Existing SQL-console
transactions remain independent throughout preparation and table disconnection.

The viewer reports an unknown total, exposes Next/Previous and labels Find as
client search of the current page; concurrent changes can shift pages. Views,
other engines and unsupported PostgreSQL metadata still use an explicit new
console LIMIT100 read. That fallback is bounded, not a complete or pageable
snapshot. Opened/dropped SQL files never execute automatically. Row viewing
continues to use its older bounded full-schema verification capture; adapting
that path to target-only metadata remains separate work.

Tree rows now use compact count/partial/expired badges, named accessible icon
refresh/edit controls, complete-name tooltips, and an object action menu for
Structure/Data/SQL. Fields stay on one visual line while preserving complete
accessible names and the full Structure grid. The model, scopes and request
identity are unchanged by this density pass. Table-view locale changes retain
the same editor/grid/staging entities; Preferences can open and close alongside
Tools or Data without discarding those views. Recovery success captions also
follow the current language; underlying SQL, identifiers and diagnostics remain
unchanged. The separate console completion snapshot is still captured once;
navigator refresh does not yet refresh it. Explicit console catalog refresh and
a Spotlight-compatible completion shortcut are tracked follow-ups.

Navigator resources are bounded independently from SQL-console workers:
2 metadata request slots,16 queued requests,256 expanded branches,32 polled
bridge events per UI turn, and a16 MiB/1,024-entry cache budget. Cache keys carry
profile ID, endpoint fingerprint, database, scope and generation; result
acceptance also checks slot operation and request ticket. Removed/changed
profiles, subtree refresh and cache clear invalidate stale acknowledgements.
Held expanded/inspector leases remain charged after expiry/invalidation. The
recursive retained-capacity estimate includes key, vector and string capacity;
it is an accounted payload budget, not a process RSS limit. Rejected admissions
stay visible rather than claiming to have freed leased memory. **Clear cache**
releases visible branches and resident entries; in-flight probes keep their
slot until terminal and cannot silently repopulate cleared branches.

Model tests cover cross-database/schema identity, stale profile/ticket replies,
remove/re-add generations, scoped refresh of collapsed children, queue/branch
caps, wrong-scope terminal errors, retained capacity and held-lease admission.
A real test creates two owned temporary SQLite files with same-named tables and
checks lazy stages, actual PK/default fields and isolation. A missing-file test
also verifies that metadata cannot create a SQLite file even when Connect may.
The pending-reader model adds four operation/identity/cancel regressions; an
opt-in owned PG read-only test holds console A in an explicit transaction while
preparing/querying/disconnecting table reader B, then verifies A remains in its
original database/transaction before explicit rollback. An opt-in owned PG
read-only test verifies two advertised databases and separate schema branches.
Native tree/layout/AX validation is pending for this revision.

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
succeeds. History and recovery retain a `StoredExecutionTarget` containing profile
ID, engine, configured database and endpoint fingerprint. Existing records with
only a legacy profile ID remain unresolved until explicit selection. A changed
or missing endpoint cannot silently retarget a restored tab. Opening history
never connects or runs SQL.

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

The UI unit suite covers: virtual range edges, fixed-column translation,
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

## Korean / English preferences

Cmd+, and the application menu open Preferences. Language changes save through
the shared `dbide-settings` store in the background and apply only after a
successful write. Concurrent selections are serialized; corrupt/future settings
are preserved and errors remain visible. CLI `config language get/set ko|en` uses
the same `app-settings.json` beside the chosen profile store. Native QA verified
English startup from CLI settings, both runtime language directions and Korean
after restart, preserving SQL source and distinct restored database targets.

Core menus, controls, connection form and status wrappers use Korean/English
labels. SQL, identifiers, profile names and original database diagnostics remain
verbatim. Some specialized tools and upstream component fallback messages are
still English; this is not a claim of complete translation or physical Korean
IME acceptance. See [preferences](PREFERENCES.md) and [catalog](I18N.md).
