# Product UX Backlog

Status observed 2026-10-22. Priorities below add workflow structure to the
existing scope in [FEATURE-MATRIX.md](FEATURE-MATRIX.md); they do not remove or
defer its advanced capabilities. `Prototype` means source/UI exists but the
acceptance tests below are not yet established. `Not verified` means do not
describe the behavior as shipped or supported.

## P0 — Make target context explicit and stable

**User outcome:** every SQL tab shows where it will run and keeps that target
until the user changes it in that tab.

**Current state:** the workspace has profile selection and query execution
menus. History/recovered tabs can retain a required profile. Ordinary SQL tabs
still rely on the currently configured workbench profile; independent
per-tab datasource/database/schema/session selectors are not complete.

**Work:**

- Add a context strip to each SQL tab with profile, database, schema or ordered
  search path, and session status.
- Let a tab explicitly bind or switch target. Use the workspace-selected
  profile only when creating a new unbound tab.
- Block Run when the tab’s required profile/context is missing, changed, or
  disconnected. Offer a clear rebind action; never execute against another
  tab’s current profile by accident.
- Show connection state separately from selected profile state.

**Acceptance:** create two profiles with a same-named table but different
marker values. Bind two tabs to different targets, switch the workspace
selection, and run each tab. Each query must return its bound marker. Removing
or invalidating a profile must disable Run with the affected target named.
Changing a tab’s context must update its visible label before the next query.

## P1 — Treat the explorer as a bounded, honest catalog tree

**User outcome:** find objects quickly without loading an unbounded catalog or
mistaking a partial snapshot for a complete one.

**Current state:** schema grouping, text search, expand/collapse, manual refresh,
loaded object/column counts, and a metadata-limit warning are present as a
prototype. Catalog capture is a bounded flat metadata query; there is no user
choice of introspection scope/level or lazy per-schema fetch yet.

**Work:**

- Add visible schema/database selection as the metadata scope, separate from
  filters that only hide objects from view.
- Load tree branches on expand with bounded child batches. Retain a continuation
  state or explicit “subset only” marker at limits.
- Separate refresh-selected-object/schema from force-refresh-all-cache.
- Preserve profile/database identity with each metadata snapshot. Mark
  completion and object inspectors as based on stale or partial metadata.
- Keep advanced object details, DDL, indexes, views, and migration capabilities
  in the existing feature backlog; this item improves discovery/access.

**Acceptance:** with a generated catalog larger than one fetch batch, opening a
connection loads only the initial bounded view; expanding a schema retrieves
its next objects without fetching all schemas. Search reports whether it covers
the loaded subset or full selected scope. Refreshing one schema leaves unrelated
branches intact; full refresh visibly invalidates and rebuilds the snapshot.

## P1 — Add a real Preferences surface and localization foundation

**User outcome:** discover and adjust behavior in one predictable place, using
Korean or English labels consistently.

**Current state:** native File and Query menus expose core editor actions and a
recovery toggle is available in the tab strip. No general Preferences screen,
central keymap settings, or Korean/English localization catalog was observed.
The visible interface is English.

**Work:**

- Add Preferences under the application menu and a keyboard shortcut.
- Start with Connection, SQL Editor, Results and Data, Recovery, Keymap, and
  Language sections. Searchable setting names and short help text should make
  defaults discoverable.
- Establish locale IDs, message catalogs, plural handling, and fallback rules
  before translating the whole UI. Migrate user-facing labels incrementally;
  keep SQL, identifiers, and driver errors in their original form with a clear
  localized explanation when available.
- Keep text expansion, accessibility labels, and keyboard shortcuts valid in
  both locales. Do not treat Unicode paste as Korean IME verification.

**Acceptance:** change a setting, restart, and verify the persisted value and
its effect. Switch Korean/English and verify menu, dialogs, errors, empty states,
and accessible labels change together without changing SQL text. Keyboard
navigation remains usable in both locales. Test physical Korean IME composition
separately from pasted Korean text.

## P1 — Give result data explicit paging, filter, and sort semantics

**User outcome:** explore large results with a clear memory bound and know
whether a control affects the database or only loaded rows.

**Current state:** query results are bounded retained snapshots with multiple
result-set navigation. The regular grid has sorting disabled and no row-page
controls. A PostgreSQL table viewer has 100-row server pages and search over
the loaded page, but remains read-only and sort/filter controls are not
complete.

**Work:**

- Add row paging or fetch-next-segment controls to query results; distinguish
  row-page navigation from switching between result sets.
- Configure page size, fetch chunk size, and retained-byte limit separately.
- Add filters and sort with an explicit `Database` or `Loaded rows` mode. For
  table data, show when the query was re-issued with `WHERE` or `ORDER BY`. For
  arbitrary query results, do not imply a server-side reorder unless it is
  actually supported.
- Keep NULL, empty string, and absent/default values visibly distinct. Label
  exports as selected page, fetched result, or full object.
- Continue row CRUD only through the verified metadata/identity/transaction
  flow tracked in the feature matrix; do not make arbitrary query results
  editable by default.

**Acceptance:** stream a generated multi-million-row fixture while keeping
memory under the configured budget. Fetching the next page must not restart an
unrelated query or corrupt an earlier result. A filter for NULL must exclude
empty text; a local sort must identify itself as applying only to fetched rows.
Cancel during fetch and export, then verify terminal state and partial-file
handling.

## P1 — Make menus and object actions discoverable

**User outcome:** perform common work from the editor and navigator without
remembering hidden toolbar locations.

**Current state:** File and Query menus cover new/open/save, script/current
execution, cancel, format, and completion. The explorer has schema and object
expansion plus object actions, but does not yet provide a complete object-type
menu or Preferences/Database menu hierarchy.

**Work:**

- Keep menu and toolbar actions synchronized, with disabled states that explain
  why the operation is unavailable.
- Add target-aware object context actions as their backing capabilities become
  available: open data, inspect structure, copy qualified name, generate SQL,
  refresh, and supported DDL/import/export tools.
- Add keyboard shortcuts for high-frequency navigation, result paging,
  copy/paste, filters, and object search. Support remapping in Preferences.
- Ensure menus show only valid actions for the selected engine/object type.

**Acceptance:** keyboard-only user can create/open a tab, choose target, run
current statement, cancel, navigate results and reach a table from search.
Right-clicking a table, view, schema, and connection offers distinct valid
actions; unsupported actions are absent or explain the missing capability.

## P2 — Improve history, recovery, and repeatable work

**User outcome:** recover drafts safely and find prior work by text and target.

**Current state:** opt-in local SQL draft recovery restores tab source and
profile binding without connecting or executing. Query history is bounded and
can be persisted only after an explicit choice; current history view is not
searchable and does not offer favorites or filters.

**Work:**

- Add history search by SQL text, profile, time, and outcome. Show target and
  result status beside each entry.
- Open history into a new tab bound to its original profile and never execute
  on open.
- Keep recovery and history separate; do not restore cached results by replaying
  SQL. Preserve the current opt-in/privacy disclosure for local SQL storage.
- Add saved query/favorite organization after target binding and local file
  recovery semantics are stable.

**Acceptance:** recover several named dirty tabs after restart without opening a
connection. Open a history item and verify its profile binding; it must not run
until an explicit Run action. Search results identify truncation/retention
limits and persistence setting.

## P2 — Keep advanced capabilities in scope

Tree, preferences, localization, and result work are product foundations. They
do not replace later database functionality. Continue `FEATURE-MATRIX.md`
coverage for SQL semantic analysis/refactor, verified table editing, import and
export, object DDL, schema/data diff, diagrams, EXPLAIN, charts, NoSQL, more
engines, extensibility, and platform/accessibility work. Track support per
engine and capability, with acceptance evidence; do not report a single
parity percentage.

## UX reference links

- [DataGrip query consoles and datasource/schema binding](https://www.jetbrains.com/help/datagrip/query-consoles.html)
- [DataGrip sessions](https://www.jetbrains.com/help/datagrip/managing-connection-sessions.html)
- [DataGrip metadata introspection](https://www.jetbrains.com/help/datagrip/introspection.html)
- [DataGrip schemas and refresh](https://www.jetbrains.com/help/datagrip/schemas.html)
- [DataGrip data editor and viewer](https://www.jetbrains.com/help/datagrip/data-editor-and-viewer.html)
- [DataGrip data editor settings](https://www.jetbrains.com/help/datagrip/settings-tools-database-data-views.html)
- [DBeaver Database Navigator](https://dbeaver.com/docs/dbeaver/Database-Navigator/)
- [DBeaver result navigation](https://dbeaver.com/docs/dbeaver/Navigation/)
- [DBeaver Data Editor preferences](https://dbeaver.com/docs/team-edition/desktop/Data-Editor-preferences/)
- [TablePlus Getting Started](https://docs.tableplus.com/getting-started)
- [TablePlus query history](https://tableplus.com/docs/query-editor/query-history)
- [TablePlus connection management](https://docs.tableplus.com/gui-tools/manage-connections)
