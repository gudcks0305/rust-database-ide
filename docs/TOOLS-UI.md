# Native database tools panel

`dbide-tools-ui` supplies `Entity<ToolsPanel>` for the existing GPUI main window.
The parent initializes GPUI/component themes and assets, then mounts:

```rust
cx.new(|cx| dbide_tools_ui::ToolsPanel::new(request, initial_sql, window, cx))
```

`request: Option<dbide_profiles::ConnectionRequest>` and the SQL text are copied
when the panel opens. Its header identifies profile name, host, port and database.
Subsequent parent profile/tab changes do not silently retarget an operation. Close
and reopen the panel to select another connection. Passwords are kept in memory,
never displayed/logged/persisted by this crate; there are no Keychain calls.

## Available tabs

- **Schema:** explicit comma-separated schema scope, default `public`. Capture
  calls `dbide_workflows::capture_schema` on a dedicated PostgreSQL read-only
  connection, showing captured schema/table/column counts, scope, completeness
  and provenance. Save snapshot JSON and open desired snapshot JSON or supported
  PostgreSQL DDL through native file dialogs. DDL default schema is the first
  schema in the scope input. Unsupported DDL rejects the entire input.
- **Objects:** browse actual captured schemas, table columns/defaults/generated
  markers, primary keys, foreign keys, index metadata and view definitions.
  Basic draft forms generate ordinary btree indexes over explicit captured
  column names and plain CREATE / CREATE OR REPLACE / DROP VIEW. Identifiers
  are quoted; every output is parsed as exactly one PostgreSQL statement.
  View definitions reject modifying CTEs, SELECT INTO and locking queries;
  function side effects cannot be certified by the parser and require review.
  DROP uses RESTRICT; constraint-owned index removal and materialized view
  management are refused. Replace/drop drafts carry explicit destructive labels.
  Existing captured relation-name collisions are diagnosed. Advanced index
  expressions, predicates, INCLUDE, opclasses, collation and unsupported table
  metadata are displayed honestly and not recreated by the basic form. Column
  detail caps at 300, FK/index detail at 200 each; complete capture remains in
  snapshot JSON. Captured views show their query definition; ACLs, options and
  triggers are outside the basic form. Copy/save/open-console always refer to
  the generated draft snapshot, even after later form edits. No SQL executes.
- **Migration:** compare desired source against captured actual target through
  `compare_snapshots`. The direction is **desired → actual**: proposed operations
  change actual to match desired. Show SQL, destructive/data-validation/blocked
  flags, warnings and implicit-commit metadata. Export a SQL preview; there is no
  database apply button or claim of execution/freshness/atomicity. Table preview
  caps at 300 operations; export contains the complete bounded plan.
- **Relationships:** captured foreign keys, table/column cards, an inline native
  byte-SVG preview and SVG export from `dbide_schema::er`. The same graph caps
  at 80 tables, 24 columns/table, 200 edges and 2,000,000 SVG bytes. Truncation and
  out-of-scope targets are visible. XML escaping and standalone SVG generation
  belong to the schema library. No foreign keys are inferred from names.
- **EXPLAIN:** multiline SQL field, exactly one parsed statement. Calls
  `explain_plan(..., false, ...)` through a dedicated read-only session; ANALYZE
  is never requested. Expand/collapse plan nodes with estimated costs, rows and
  widths. At most 300 visible nodes, indentation capped at 20 levels. Results are
  estimates; this does not display actual execution timing or execute a query.
- **Data compare:** independently open before/after tagged JSON transfer files;
  enter explicit comma-separated column names. Exact kinds/values use
  `dbide_data::diff_by_key`. Both schemas must match. Missing, duplicate or NULL
  identities fail visibly; no positional identity is guessed. Show inserted,
  deleted, changed and unchanged counts over both complete inputs, plus first
  100 changes with before/after typed cells (long cell display is truncated).
  Limits: 10,000 rows and 16 MiB per input. Inputs use the versioned tagged
  `dbide_data::write_json` format, not arbitrary JSON arrays.

Live schema/EXPLAIN and object drafts are PostgreSQL only. SQLite/MySQL requests display the engine-aware fixed
connection header (SQLite shows its file path) but disable unsupported live actions. Desired schema imports
and data comparison work without a live connection. Charts, native dump tools,
DDL execution and broader M4 features are outside this pane's current coverage.

## Operation and file safety

Database/network, parsing, filesystem reads and serialization execute in named
worker threads with dedicated current-thread Tokio runtimes. GPUI only receives
bounded results. Catalog/plan workflow defaults enforce 15-second statement and
2-second lock timeouts; each tools worker has an outer 135-second deadline.
A cancellation token is checked before worker execution and every 50 ms while
awaiting asynchronous operations. Cancel/drop retires the read-only workflow
future and its dedicated session. It never cancels the parent's SQL session.
CPU/file processing already in progress completes its current bounded operation;
there is no forced thread termination or claim that a completed export is undone.

Revision IDs reject stale results and prevent old completions from clearing a
new operation's busy/error state. Native dialog cancellation and all worker
result/error paths clear loading. Closing drops the panel's cancellation token;
workers are not joined on the UI thread. No user database is contacted by unit
tests.

Schema text reads cap at 16 MB; tagged JSON validates its raw/typed budgets.
Exports first write and sync a temporary file in the destination directory,
then atomically persist without clobbering. **Existing destinations, including
symlinks, are refused even if the OS save dialog allows selecting them.** Choose
a new filename. Import/export errors retain previous successful inputs and
report actionable errors. A changed snapshot invalidates its migration preview.

Controls use native GPUI component Buttons/TextInputs, accessibility labels and
Table/Tree/Status/Alert roles. Layout uses the main workbench dark neutral palette.
This implementation has source/compile/unit validation; interactive macOS
VoiceOver, keyboard navigation, SVG rendering and GUI smoke validation must be
reported separately. The native app is not launched by crate checks.

## Validation

```sh
cargo check -p dbide-tools-ui
cargo test -p dbide-tools-ui --lib
cargo clippy -p dbide-tools-ui --all-targets --no-deps -- -D warnings
```

Pure tests cover stale operation results/cancel/error completion, explicit scope
and key selection, duplicate row identity, pre-cancelled worker execution and
atomic export refusal to overwrite an existing destination. Tests do not require
GPUI globals, native windows, user profiles or database writes.

## Opening object drafts in the main workbench

The parent subscribes to `ToolsEvent` emitted by `ToolsPanel`:

```rust
pub enum ToolsEvent {
    OpenSqlDraft { profile: dbide_profiles::ConnectionProfile, sql: String },
}
```

The profile is the full password-free configuration captured at panel open,
including its stable ID, engine and target. The parent must open a **new dirty
console** for this exact captured profile after checking it still exists and
matches. If it changed, diagnose the mismatch and ask the user to reopen tools.
Never attach to the current arbitrary selection or execute the event's SQL.
Credentials remain owned by the main workbench and are never emitted in events.

Additional pure tests cover engine-aware SQLite target labels, quoted identifier
injection resistance, exact captured index columns, constraint-owned index
refusal, exactly-one-query view definitions, explicit destructive replacement /
DROP RESTRICT and materialized-view rejection. Native GUI behavior remains an
unperformed manual validation gate.

## Korean and English

`ToolsPanel::new(request, initial_sql, window, cx)` remains compatible and uses
`Locale::default()` (Korean). Hosts with an explicit preference use:

```rust
ToolsPanel::new_with_locale(request, initial_sql, locale, window, cx)
// Existing Entity<ToolsPanel>, including while a read-only operation is pending:
tools.update(cx, |panel, cx| panel.set_locale(locale, cx));
```

`locale()` returns the panel's current `dbide_i18n::Locale`. Locale is explicit
per panel; this crate does not mutate global GPUI state or persist preferences.
Changing language repaints headings, tabs, actions, ARIA labels, forms, empty
states, progress/status text, draft descriptions, boolean labels and diagnostic
wrappers. It preserves the captured connection, selected tab/object, SQL/editor
values, results and operation revision/cancellation state. Async completions use
the current display locale rather than capturing the language at submission.

Tools messages and summary templates live in the shared typed `dbide-i18n`
catalog. Its existing completeness, per-language formatting and placeholder
parity tests cover the additional entries. Summary interpolation is a bounded
UI preview and may show an ellipsis for arguments exceeding 4096 UTF-8 bytes;
file exports, clipboard SQL and `ToolsEvent` SQL remain the original payload.
Application validation/worker errors are translated at display time. PostgreSQL
workflow diagnostics have an explicit raw origin: even a raw diagnostic equal
to an application message stays verbatim, under a translated error wrapper.

SQL, identifiers, profile names, filenames/paths, data cells, PostgreSQL plan
node names and server diagnostics remain unchanged. Schema/workflow library
warnings, canonical SQL keywords/type names, provenance/debug metadata and
unsupported-feature markers may remain English as source metadata. The GPUI
component package does not ship a Korean built-in locale in this version;
component-owned context menus/prompts and OS dialogs may retain English or the
OS language. This is application-message coverage, not a claim that every native
or third-party component is fully localized. Interactive language switching,
VoiceOver and keyboard checks remain a separate GUI gate; no native app is
launched by these crate checks.
