# UX Research: Database Workbench

Status observed 2026-10-22. This is a product UX reference for the current Rust
prototype. It records concrete workflows from official product documentation;
it does not claim parity, prescribe an implementation architecture, or narrow
the feature scope in `FEATURE-MATRIX.md`.

## Current product state

| Area | Observed state | Evidence boundary |
|---|---|---|
| Connections | A `+ New` action opens a profile form. PostgreSQL, SQLite, and MySQL profiles can be selected; connection setup, test, credential, and TLS controls are present. | Native prototype; database engines and profile options do not imply complete engine support. |
| Menus | Native File and Query menus expose new/open/save SQL, run script/current statement, cancel, format, and completion. | Implemented prototype. No general Preferences screen or complete View/Database/Help action hierarchy observed. |
| SQL drafts | Recovery can be enabled explicitly. It stores local SQL drafts and restores tabs without connecting or executing SQL. | Implemented prototype; query results and full connection/session state are not restored. |
| Explorer | Loaded objects group by schema, support text search, expand/collapse and manual refresh. The UI reports when metadata reached its limit. | Bounded metadata snapshot, not a lazy catalog tree. Search only covers what has been loaded. |
| Table data | The table viewer uses 100-row bounded pages with previous/next/reload and a search over the loaded page. | Prototype remains read-only; the UI explicitly says row-edit validation is pending and Apply is disabled. |
| Query result grid | Shows retained results, can move between result sets, and copies a selected cell. | Result-set navigation is not row paging; result-grid sorting is disabled and no filter UI was observed. |
| Execution context | A profile is selected at workbench level. History/recovered documents can retain a required profile ID; ordinary SQL tabs do not yet expose independent datasource/database/schema/session selection. | Wrong-target protection for context-bound documents is distinct from a complete per-tab context model. |
| Localization | The visible UI labels are English. No Korean/English locale catalog or language preference was observed. | Localization is pending; this is not a finding about Korean input support. |

These are source observations, not end-to-end acceptance results. Native GUI,
keyboard, Korean IME, VoiceOver, restart-recovery, database correctness, and
cross-engine behavior need their own verification. No feature-parity percentage
is inferred.

## Research-backed workflows

### Connection, navigator, and execution context

DataGrip places datasource, schema/search-path, and session controls on the
query-console or SQL-file toolbar. A query console is attached to a datasource
and has a database session; SQL files can attach to a datasource/session.
DataGrip offers shared, shared-plus-introspection, and dedicated session
modes. The Database Explorer supports selecting schemas for introspection,
filtering displayed objects, refreshing, and opening object actions.

DBeaver’s Database Navigator makes connection state and object actions visible
in the tree: connect, invalidate/reconnect, disconnect, open an SQL editor,
view/edit/create an object, filter, generate SQL, import/export, and compare or
migrate. Its preferences include child fetch size and how much tree expansion
to restore at startup. TablePlus demonstrates saved connection groups/tags,
connection colors, connection URL import, and reopening prior workspaces.

**Product implication:** a tab’s target should be legible at the point of Run.
Connection selection, database/schema/search path, and physical session are
separate concepts. Profile-wide selection can remain useful for new tabs, but
must not silently retarget an existing tab. A context that no longer resolves
must block execution and explain the missing or changed target.

### Catalog scope, refresh, and completion

DataGrip metadata is loaded into an IDE model and feeds the object tree, DDL,
completion, navigation, and search. Its documentation distinguishes full,
incremental, and fragment refresh. For selected databases, introspection levels
can load only names, names plus structure, or the full object source. The
documentation warns that broad introspection can take time and disk space. The
object filter controls visibility; schema selection controls what is
introspected. These are not the same setting.

DBeaver offers configurable child fetch size for long lists and lazy metadata
features in the Navigator. TablePlus also documents lazy loading and offers
object-aware completion settings such as tables, keywords, and schema prefixes.

**Product implication:** a bounded catalog needs a visible scope and an honest
partial state. A text filter must not imply that unseen objects were searched.
Manual refresh, background refresh, and full cache invalidation should be
separate actions when their cost differs. Completion should show which loaded
catalog/context it used and avoid presenting a partial candidate set as
complete.

### Results and table data

DataGrip’s grid provides first/previous/next/last row-page navigation, page-size
selection, filter and sort controls, value editor, data view modes, and reload.
Its settings separate the visible page size from result prefetch size: larger
prefetch uses fewer network round trips but more memory. For table data, sorting
can send a new `ORDER BY`; local sorting only reorders the current page. The
documentation says server-side sorting is not available for arbitrary query
result sets.

DBeaver supports bounded result segments, automatic or explicit fetch-next-page,
optional row count, and client/server/adaptive sorting. TablePlus offers
column-condition filters, including `IS NULL`, and warns that exporting from a
data browser can export only the current page while table export can cover the
whole table.

**Product implication:** the UI must name the unit being navigated: result tab,
row page, fetched segment, or full result. It should say whether filter/sort
changed the SQL or only the currently loaded rows. NULL and empty text must stay
visibly distinct, and export should state whether it covers the selected page,
retained query result, or full table.

### Files, recovery, history, and preferences

DataGrip stores query consoles separately from ordinary project files, offers
query history, and has settings for SQL resolution, execution, and data editing.
DBeaver presents SQL scripts and result sets in editor tabs and exposes editor,
result, metadata, and transaction preferences. TablePlus documents query
history per connection, search and actions to run/copy/save/favorite a query;
its history can include SQL generated by GUI data edits. It also exposes
preferences for autosave, keymaps, data-table behavior, and reopening workspaces.

**Product implication:** local draft recovery and query history serve different
purposes. Restoring a draft must not open a connection or rerun SQL. History
entries should retain their target identity and open without execution. A
central Preferences entry should group connection behavior, SQL editor,
results/data, recovery, keymap, and language, instead of scattering only a few
settings across panels.

### Errors, cancellation, and keyboard behavior

DataGrip distinguishes canceling a statement from deactivating its data source;
deactivation can stop the IDE’s wait while server execution continues. It also
exposes session operations and current statement selection from query editor
controls. DBeaver offers reconnect and explicit fetch/cancel actions. TablePlus
supports Run All and Run Current as selectable execution modes and configurable
return-on-error behavior.

**Product implication:** report cancel-requested, server-cancelled, connection
retired, and outcome-unknown as separate states. Preserve completed earlier
result sets on script failure and identify the failed statement. Core actions
must be reachable by menu, toolbar, and keyboard; keyboard focus and accessible
names should expose the same context as the visible controls.

## Official product sources

The comparison uses product help pages as workflow references, not as
independent quality measurements. DataGrip references are current 2026.2 help
pages. DBeaver pages are official docs, with one Data Editor preferences page
under Team Edition docs. Several TablePlus workflow pages show older update
dates; use their concrete affordances as examples and re-check current behavior
before treating a detail as a requirement.

- [DataGrip query consoles](https://www.jetbrains.com/help/datagrip/query-consoles.html)
- [DataGrip sessions](https://www.jetbrains.com/help/datagrip/managing-connection-sessions.html)
- [DataGrip introspection](https://www.jetbrains.com/help/datagrip/introspection.html)
- [DataGrip schemas and refresh](https://www.jetbrains.com/help/datagrip/schemas.html)
- [DataGrip data editor and viewer](https://www.jetbrains.com/help/datagrip/data-editor-and-viewer.html)
- [DataGrip data editor preferences](https://www.jetbrains.com/help/datagrip/settings-tools-database-data-views.html)
- [DBeaver Database Navigator](https://dbeaver.com/docs/dbeaver/Database-Navigator/)
- [DBeaver result-set navigation](https://dbeaver.com/docs/dbeaver/Navigation/)
- [DBeaver Data Editor preferences](https://dbeaver.com/docs/team-edition/desktop/Data-Editor-preferences/)
- [DBeaver SQL Editor](https://dbeaver.com/docs/dbeaver/SQL-Editor/)
- [TablePlus Getting Started](https://docs.tableplus.com/getting-started)
- [TablePlus query history](https://tableplus.com/docs/query-editor/query-history)
- [TablePlus connection management](https://docs.tableplus.com/gui-tools/manage-connections)
- [TablePlus SQL Editor preferences](https://docs.tableplus.com/preferences/general/sql-editor)
- [TablePlus keymap](https://docs.tableplus.com/preferences/keymap)
