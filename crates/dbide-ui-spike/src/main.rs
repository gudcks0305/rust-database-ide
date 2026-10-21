mod completion_model;
mod documents;
mod native_menus;
mod preferences;
mod table_tabs;
mod workbench_shell;
use dbide_i18n::{Locale, Message, tr};
mod object_inspector;
mod profile_form;
mod profile_persistence;
use profile_persistence::{ProfileMutation, ProfileWrite};
mod console_selector;
mod table_editor;
mod table_open;
mod workbench_actions;
mod workbench_consoles;
use dbide_ui_spike::console_session::{
    ConsoleEffect, ConsoleRoute, ConsoleSession, MAX_CONSOLE_WORKERS, SessionPhase,
};
mod workbench_explorer;
mod workbench_panels;
mod workbench_tables;
mod workspace_recovery;
use dbide_bridge::{
    BridgeEvent, CancelTarget, CommitPolicy, DatabaseBridge, ServerTransactionState,
};
use dbide_profiles::{
    ConnectionProfile, ConnectionRequest, DatabaseEngine, HistoryEntry, ProfileStore, TlsMode,
    WorkspaceState,
};
use dbide_ui_spike::workbench_model::{DocumentModel, OwnedRenamePreview, ResultOwner};
use dbide_ui_spike::{
    LOGICAL_COLUMNS, LOGICAL_ROWS, QuerySummary, ResultPage, bounded_range, column_name,
    fixture_value, logical_column_range, split_report,
};
use documents::{ResultSnapshot, SqlDocument};
use gpui::{prelude::*, *};
use gpui_component::{
    Disableable, Root, Sizable, Theme, ThemeMode,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Editor, EditorState, Input, InputEvent, InputState, RopeExt},
    table::{Column, DataTable, TableDelegate, TableEvent, TableState},
};
use gpui_kit_assets::Assets;
use profile_form::ProfileForm;
use serde::Serialize;
use std::{
    collections::{HashSet, VecDeque},
    ops::Range,
    sync::Arc as Rc,
    time::{Duration, Instant},
};
use workbench_tables::TableOperation;

actions!(
    dbide,
    [
        Quit,
        ReportMetrics,
        CopyCell,
        RunQuery,
        RunCurrent,
        CancelQuery,
        OpenPreferences,
        FormatSql,
        CompleteSql,
        RefreshCompletionMetadata,
        NextWorkspaceTab,
        PreviousWorkspaceTab,
        FocusCatalog,
        FocusResults,
        ToggleResults,
        ToggleCatalog,
        NewSqlFile,
        OpenSqlFile,
        SaveSqlFile,
        SaveSqlAs,
        CloseSqlTab
    ]
);
const BG: u32 = 0x111820;
const PANEL: u32 = 0x18212b;
const BORDER: u32 = 0x2a3644;
const TEXT: u32 = 0xdce1e9;
const MUTED: u32 = 0x8b95a5;
const ACCENT: u32 = 0x79b5ee;
const INITIAL_SQL: &str = "-- Write a query, then press ⌘Enter to run.\nSELECT 1 AS result;";
const FIXTURE_SQL: &str = "SELECT id, name, email, status, balance, display_name FROM dbide_benchmark.customers ORDER BY id LIMIT 1000;";

#[derive(Clone)]
enum GridSource {
    Logical,
    Postgres(Rc<ResultPage>),
}
struct ResultGrid {
    source: GridSource,
    selected_cell: Option<(usize, usize)>,
    columns: Vec<Column>,
    cell_render_calls: u64,
    visible_rows: Range<usize>,
    visible_columns: Range<usize>,
}
impl ResultGrid {
    fn new(source: GridSource) -> Self {
        let names: Vec<String> = match &source {
            GridSource::Logical => (0..LOGICAL_COLUMNS).map(column_name).collect(),
            GridSource::Postgres(page) => page.columns.clone(),
        };
        Self {
            columns: dbide_ui_spike::result_grid_columns(names),
            source,
            selected_cell: None,
            cell_render_calls: 0,
            visible_rows: 0..0,
            visible_columns: 0..0,
        }
    }
    fn row_count(&self) -> usize {
        match &self.source {
            GridSource::Logical => LOGICAL_ROWS,
            GridSource::Postgres(p) => p.rows.len(),
        }
    }
    fn value(&self, row: usize, col: usize) -> String {
        match &self.source {
            GridSource::Logical => fixture_value(row, col),
            GridSource::Postgres(p) => p
                .rows
                .get(row)
                .and_then(|r| r.get(col))
                .map(|v| v.clone().unwrap_or_else(|| "NULL".into()))
                .unwrap_or_default(),
        }
    }
    fn is_null(&self, row: usize, col: usize) -> bool {
        match &self.source {
            GridSource::Logical => (row + col).is_multiple_of(17) && col > 5,
            GridSource::Postgres(p) => p
                .rows
                .get(row)
                .and_then(|r| r.get(col))
                .is_some_and(Option::is_none),
        }
    }
}
impl TableDelegate for ResultGrid {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.row_count()
    }
    fn column(&self, ix: usize, _: &App) -> Column {
        self.columns[ix].clone()
    }
    fn render_header(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        div().id("headers").role(Role::Row).aria_row_index(0)
    }
    fn render_th(
        &mut self,
        col: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .id(("header", col))
            .role(Role::ColumnHeader)
            .aria_column_index(col)
            .text_color(rgb(TEXT))
            .font_weight(FontWeight::SEMIBOLD)
            .aria_label(self.columns[col].name.clone())
            .child(self.columns[col].name.clone())
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        div().id(("row", row)).when(row < self.row_count(), |el| {
            el.role(Role::Row).aria_row_index(row + 1)
        })
    }
    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.cell_render_calls += 1;
        let value = self.value(row, col);
        let selected = self.selected_cell == Some((row, col));
        div()
            .id(("cell", row * self.columns.len().max(1) + col))
            .role(Role::Cell)
            .aria_row_index(row + 1)
            .aria_column_index(col)
            .aria_selected(selected)
            .aria_label(format!(
                "{}, row {}: {}",
                self.columns[col].name,
                row + 1,
                if value.is_empty() {
                    "empty text"
                } else {
                    &value
                }
            ))
            .aria_value(value.clone())
            .size_full()
            .overflow_hidden()
            .text_ellipsis()
            .text_size(px(12.))
            .text_color(rgb(if self.is_null(row, col) { MUTED } else { TEXT }))
            .child(value)
    }
    fn visible_rows_changed(
        &mut self,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.visible_rows = bounded_range(range, self.row_count());
        cx.notify();
    }
    fn visible_columns_changed(
        &mut self,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        // The virtualizer excludes the fixed ID column from this range.
        self.visible_columns = logical_column_range(
            range,
            usize::from(!self.columns.is_empty()),
            self.columns.len(),
        );
        cx.notify();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Executing,
    Cancelling,
    Disconnecting,
}
impl ConnectionState {
    fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting…",
            Self::Connected => "Connected",
            Self::Executing => "Executing…",
            Self::Cancelling => "Cancelling…",
            Self::Disconnecting => "Disconnecting…",
        }
    }
}
#[derive(Serialize)]
struct Metrics<'a> {
    schema_version: u8,
    fixture: &'static str,
    logical_rows: usize,
    logical_columns: usize,
    retained_fixture_rows: usize,
    retained_column_descriptors: usize,
    visible_rows: Range<usize>,
    visible_columns: Range<usize>,
    generated_cell_render_calls: u64,
    sql_utf8_bytes: usize,
    input_change_events: u64,
    root_render_calls: u64,
    elapsed_ms: f64,
    first_root_render_ms: Option<f64>,
    forced_redraw_intervals_ms: Vec<f64>,
    query: Option<&'a QuerySummary>,
    limitations: &'static str,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum DiagnosticContext {
    Other,
    Capacity,
    Connection,
    Schema,
    Query,
}
struct Diagnostic {
    body: (String, String, String),
    context: DiagnosticContext,
    owner: Option<ResultOwner>,
}
impl Diagnostic {
    fn new(body: (String, String, String)) -> Self {
        Self {
            body,
            context: DiagnosticContext::Other,
            owner: None,
        }
    }
}
struct Workbench {
    editor: Entity<EditorState>,
    documents: Vec<SqlDocument>,
    active_document: u64,
    tab_scroll: ScrollHandle,
    last_visible_tab: Option<(bool, u64)>,
    last_tab_space: Option<(u16, u16, bool, bool, bool)>,
    next_document: u64,
    pending_owner: Option<ResultOwner>,
    result_owner: Option<ResultOwner>,
    show_sql_tools: bool,
    completions: Vec<dbide_sql::Completion>,
    completion_source: Option<(u64, u64, String)>,
    completion_selected: usize,
    completion_cursor: usize,
    completion_route: Option<ConsoleRoute>,
    completion_schema_revision: u64,
    sql_diagnostics: Vec<dbide_sql::Diagnostic>,
    rename_name: Entity<InputState>,
    rename_preview: Option<OwnedRenamePreview>,
    commit_policy: CommitPolicy,
    transaction_state: ServerTransactionState,
    policy_operation: Option<dbide_bridge::OperationId>,
    credential_busy: bool,
    close_prompt_open: bool,
    quit_after_close: bool,
    menu_state: Option<(bool, bool, bool)>,
    locale: Locale,
    settings_store: Option<dbide_settings::SettingsStore>,
    settings_busy: bool,
    settings_pending: VecDeque<dbide_settings::SettingsChange>,
    app_settings: dbide_settings::AppSettings,
    resize_drag: Option<(bool, Point<Pixels>, f32)>,
    results_collapsed: bool,
    catalog_collapsed: bool,
    settings_error: Option<String>,
    preferences: Option<Entity<dbide_preferences_ui::PreferencesPanel>>,
    transfer_preview: Option<dbide_data::DataPreview>,
    grid: Entity<TableState<ResultGrid>>,
    bridge: Option<DatabaseBridge>,
    store: Option<ProfileStore>,
    workspace: WorkspaceState,
    profile_writes: VecDeque<ProfileWrite>,
    profile_write_busy: bool,
    recovery: workspace_recovery::Recovery,
    passwords: dbide_ui_spike::session_credentials::SessionCredentials,
    configured: Option<ConnectionRequest>,
    form: Option<ProfileForm>,
    form_error: Option<String>,
    diagnostic: Option<Diagnostic>,
    schema: Vec<dbide_bridge::SchemaObject>,
    navigator: dbide_ui_spike::navigator::Navigator,
    object_search: Entity<InputState>,
    expanded_schemas: HashSet<String>,
    expanded_objects: HashSet<String>,
    structure: Option<Entity<object_inspector::ObjectInspector>>,
    schema_loading: bool,
    schema_requested: bool,
    schema_truncated: bool,
    schema_operation: Option<dbide_bridge::OperationId>,
    show_history: bool,
    show_tools: bool,
    tools: Option<Entity<dbide_tools_ui::ToolsPanel>>,
    tools_label: String,
    table_editor: Option<Entity<table_editor::TableEditor>>,
    table_tabs: Vec<table_tabs::TableTab>,
    active_table_tab: Option<u64>,
    next_table_tab_id: u64,
    pending_table_tab: Option<u64>,
    table_operation: Option<TableOperation>,
    pending_table_open: Option<dbide_ui_spike::table_open_model::PendingTableOpen>,
    table_open_feedback: Option<table_open::TableOpenFeedback>,
    table_disconnect_operation: Option<dbide_bridge::OperationId>,
    connection_revision: u64,
    console_poll_cursor: usize,
    console_database_editor: Option<ConsoleRoute>,
    last_execution: Option<dbide_pg_spike::ExecutionKey>,
    deleted: Option<ConnectionProfile>,
    pending_sql: Option<String>,
    connection_operation: Option<dbide_bridge::OperationId>,
    configuration_operation: Option<dbide_bridge::OperationId>,
    pending_configuration: Option<ConnectionRequest>,
    connect_after_configuration: bool,
    cancellation_operation: Option<dbide_bridge::OperationId>,
    before_test: Option<ConnectionState>,
    file_name: String,
    connection: ConnectionState,
    active_execution: Option<dbide_pg_spike::ExecutionKey>,
    active_operation: Option<dbide_bridge::OperationId>,
    pages: Vec<Rc<ResultPage>>,
    selected_page: usize,
    logical_mode: bool,
    summary: Option<QuerySummary>,
    details: String,
    started: Instant,
    first_render: Option<Duration>,
    render_count: u64,
    input_changes: u64,
    status: String,
    recording: bool,
    last_frame: Option<Instant>,
    frame_intervals: VecDeque<f64>,
    fixture_smoke: bool,
    _subscriptions: Vec<Subscription>,
}
impl Workbench {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        started: Instant,
        fixture_smoke: bool,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx).language("sql");
            state.set_value(
                if fixture_smoke {
                    FIXTURE_SQL
                } else {
                    INITIAL_SQL
                },
                window,
                cx,
            );
            state
        });
        let grid = cx.new(|cx| {
            TableState::new(
                ResultGrid::new(GridSource::Postgres(Rc::new(ResultPage::default()))),
                window,
                cx,
            )
            .loop_selection(false)
            .col_movable(false)
            .sortable(false)
            .cell_selectable(true)
            .row_header(false)
        });
        let object_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search tables or columns"));
        let subscriptions = vec![
            cx.subscribe(&object_search, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&editor, |this, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.input_changes += 1;
                    this.completions.clear();
                    if let Some(doc) = this.documents.iter_mut().find(|d| d.model.id == 1) {
                        doc.model.changed();
                    }
                    cx.notify();
                }
            }),
            cx.observe(&grid, |_, _, cx| cx.notify()),
            cx.subscribe(&grid, |_, grid, event, cx| {
                if let TableEvent::SelectCell(row, col) = event {
                    grid.update(cx, |state, cx| {
                        state.delegate_mut().selected_cell = Some((*row, *col));
                        cx.notify();
                    });
                }
            }),
        ];
        let (bridge, status) = match DatabaseBridge::new() {
            Ok(bridge) => (Some(bridge), "Ready · add a connection to begin".into()),
            Err(error) => (None, format!("Database worker unavailable: {error}")),
        };
        cx.spawn_in(window, async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
                if view
                    .update_in(cx, |this, w, cx| {
                        this.poll_bridge(cx);
                        this.finish_pending_table(w, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let selected_store = match std::env::var_os("DBIDE_PROFILE_STORE") {
            Some(path) => ProfileStore::open(std::path::PathBuf::from(path)),
            None => ProfileStore::open_default(),
        };
        let (store, workspace, load_error) = match selected_store {
            Ok(store) => match store.load() {
                Ok(state) => (Some(store), state, None),
                Err(e) => (Some(store), WorkspaceState::default(), Some(e.to_string())),
            },
            Err(e) => (None, WorkspaceState::default(), Some(e.to_string())),
        };
        let navigator = store
            .as_ref()
            .and_then(|store| store.path().parent())
            .map(|root| {
                dbide_ui_spike::navigator::Navigator::with_store(
                    Default::default(),
                    root.join("catalog-cache"),
                )
            })
            .unwrap_or_default();
        let initial_source = editor.read(cx).value().to_string();
        let rename_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("New alias or CTE name"));
        let mut this = Self {
            editor: editor.clone(),
            documents: vec![SqlDocument {
                model: DocumentModel::new(1, "console.sql".into(), initial_source),
                required_profile_id: None,
                session: ConsoleSession::new(1),
                password_draft: cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .placeholder("Session password")
                }),
                database_draft: cx
                    .new(|cx| InputState::new(window, cx).placeholder("Database name")),
                editor,
                results: ResultSnapshot::default(),
            }],
            active_document: 1,
            tab_scroll: ScrollHandle::new(),
            last_visible_tab: None,
            last_tab_space: None,
            next_document: 2,
            pending_owner: None,
            result_owner: None,
            show_sql_tools: false,
            completions: vec![],
            completion_source: None,
            completion_selected: 0,
            completion_cursor: 0,
            completion_route: None,
            completion_schema_revision: 0,
            sql_diagnostics: vec![],
            rename_name,
            rename_preview: None,
            commit_policy: CommitPolicy::Auto,
            transaction_state: ServerTransactionState::Unknown,
            policy_operation: None,
            credential_busy: false,
            close_prompt_open: false,
            quit_after_close: false,
            menu_state: None,
            locale: Locale::default(),
            settings_store: None,
            settings_busy: false,
            settings_pending: VecDeque::new(),
            app_settings: dbide_settings::AppSettings::default(),
            resize_drag: None,
            results_collapsed: false,
            catalog_collapsed: false,
            settings_error: None,
            preferences: None,
            transfer_preview: None,
            grid,
            bridge,
            store,
            workspace,
            profile_writes: VecDeque::new(),
            profile_write_busy: false,
            recovery: workspace_recovery::Recovery::default(),
            passwords: dbide_ui_spike::session_credentials::SessionCredentials::default(),
            configured: None,
            form: None,
            form_error: None,
            diagnostic: load_error.map(|e| {
                Diagnostic::new((
                    "Workspace could not be loaded".into(),
                    e,
                    "Resolve the local profile file error before saving.".into(),
                ))
            }),
            schema: vec![],
            navigator,
            object_search,
            expanded_schemas: HashSet::new(),
            expanded_objects: HashSet::new(),
            structure: None,
            schema_loading: false,
            schema_requested: false,
            schema_truncated: false,
            schema_operation: None,
            show_history: false,
            show_tools: false,
            tools: None,
            tools_label: String::new(),
            table_editor: None,
            table_tabs: vec![],
            active_table_tab: None,
            next_table_tab_id: 1,
            pending_table_tab: None,
            table_operation: None,
            pending_table_open: None,
            table_open_feedback: None,
            table_disconnect_operation: None,
            connection_revision: 0,
            console_poll_cursor: 0,
            console_database_editor: None,
            last_execution: None,
            deleted: None,
            pending_sql: None,
            connection_operation: None,
            configuration_operation: None,
            pending_configuration: None,
            connect_after_configuration: false,
            cancellation_operation: None,
            before_test: None,
            file_name: "console.sql".into(),
            connection: ConnectionState::Disconnected,
            active_execution: None,
            active_operation: None,
            pages: vec![],
            selected_page: 0,
            logical_mode: false,
            summary: None,
            details: "No query executed".into(),
            started,
            first_render: None,
            render_count: 0,
            input_changes: 0,
            status,
            recording: false,
            last_frame: None,
            frame_intervals: VecDeque::new(),
            fixture_smoke,
            _subscriptions: subscriptions,
        };
        if fixture_smoke {
            let request = ConnectionRequest {
                profile: ConnectionProfile::fixture(),
                password: "dbide_fixture_only".into(),
            };
            this.configure(request, cx);
            this.connect_after_configuration = true;
        } else if let Some(profile) = this
            .workspace
            .profiles
            .iter()
            .find(|p| Some(&p.id) == this.workspace.selected_profile_id.as_ref())
            .cloned()
        {
            this.configure(
                ConnectionRequest {
                    profile,
                    password: String::new(),
                },
                cx,
            );
        } else {
            this.form = Some(ProfileForm::new(ConnectionProfile::new(), "", window, cx));
        }
        if let Some(form) = &this.form {
            form.fields[0].update(cx, |field, cx| field.focus(window, cx));
        } else {
            this.editor
                .update(cx, |editor, cx| editor.focus(window, cx));
        }
        this.navigator.sync_profiles(&this.workspace.profiles);
        this.init_settings(cx);
        this.init_recovery(window, cx);
        this
    }
    fn install_source(&mut self, source: GridSource, cx: &mut Context<Self>) {
        self.grid.update(cx, |grid, cx| {
            *grid.delegate_mut() = ResultGrid::new(source);
            grid.clear_selection(cx);
            grid.refresh(cx);
            grid.scroll_to_row(0, cx);
            grid.scroll_to_col(0, cx);
            cx.notify();
        });
    }
    fn set_mode(&mut self, logical: bool, cx: &mut Context<Self>) {
        self.logical_mode = logical;
        if logical {
            self.install_source(GridSource::Logical, cx);
        } else {
            self.install_source(
                GridSource::Postgres(
                    self.pages
                        .get(self.selected_page)
                        .cloned()
                        .unwrap_or_else(|| Rc::new(ResultPage::default())),
                ),
                cx,
            );
        }
        cx.notify();
    }
    fn connect(&mut self, cx: &mut Context<Self>) {
        if self.busy()
            || self.connection != ConnectionState::Disconnected
            || self.configured.is_none()
        {
            return;
        }
        if let Some(bridge) = &self.bridge {
            match bridge.connect() {
                Ok(operation) => {
                    self.connection_operation = Some(operation);
                    self.diagnostic = None;
                    self.connection = ConnectionState::Connecting;
                    self.status = "Opening database connection…".into();
                }
                Err(e) => self.status = format!("Connect request failed: {e:?}"),
            }
        }
        cx.notify();
    }
    fn cancel_query_action(&mut self, _: &CancelQuery, _: &mut Window, cx: &mut Context<Self>) {
        if self.active_table_tab.is_none() && !self.fixture_smoke {
            let route = self.console().route();
            if let Some(target) = self.console().cancel_target() {
                self.cancel_console(route, target, cx);
            }
            return;
        }
        let target = self
            .active_execution
            .map(CancelTarget::Execution)
            .or_else(|| self.active_operation.map(CancelTarget::Operation));
        if target.is_some() {
            self.cancel(target, cx);
        }
    }
    fn run_query(&mut self, _: &RunQuery, _: &mut Window, cx: &mut Context<Self>) {
        self.submit_query(cx);
    }
    fn submit_query(&mut self, cx: &mut Context<Self>) {
        let sql = self.editor.read(cx).value().to_string();
        self.submit_sql(sql, cx);
    }
    fn submit_sql(&mut self, sql: String, cx: &mut Context<Self>) {
        if !self.sql_view_active() {
            return;
        }
        if !self.fixture_smoke {
            self.submit_console_sql(sql, cx);
            return;
        }
        if !self.document_profile_matches() {
            self.status = self.document_connection_label();
            cx.notify();
            return;
        }
        if !self.can_run(cx) || sql.trim().is_empty() {
            return;
        }
        if let Some(bridge) = &self.bridge {
            match bridge.execute(sql.clone()) {
                Ok(operation) => {
                    self.pending_sql = Some(sql);
                    self.pending_owner = self.current_owner();
                    self.diagnostic = None;
                    self.active_operation = Some(operation);
                    self.connection = ConnectionState::Executing;
                    self.status = "Query submitted…".into();
                    self.set_mode(false, cx);
                }
                Err(e) => self.status = format!("Query request failed: {e:?}"),
            }
        }
        cx.notify();
    }
    fn cancel(&mut self, target: Option<CancelTarget>, cx: &mut Context<Self>) {
        if self.connection != ConnectionState::Executing {
            return;
        }
        let Some(target) = target else {
            return;
        };
        if let Some(bridge) = &self.bridge {
            match bridge.cancel(target) {
                Ok(operation) => {
                    self.cancellation_operation = Some(operation);
                    self.connection = ConnectionState::Cancelling;
                    self.status = "Cancellation requested…".into();
                }
                Err(e) => self.status = format!("Cancel request failed: {e:?}"),
            }
        }
        cx.notify();
    }
    fn disconnect(&mut self, cx: &mut Context<Self>) {
        if self.pending_table_open.is_some() {
            self.abort_table_open(
                if self.locale == Locale::Ko {
                    "테이블 열기를 취소하고 테이블 연결만 종료합니다."
                } else {
                    "Table opening cancelled; disconnecting only the table connection."
                }
                .into(),
                true,
                cx,
            );
            return;
        }
        if self.connection == ConnectionState::Disconnected {
            return;
        }
        if let Some(bridge) = &self.bridge {
            match bridge.disconnect() {
                Ok(_) => {
                    self.connection = ConnectionState::Disconnecting;
                    self.status = "Disconnecting…".into();
                }
                Err(e) => self.status = format!("Disconnect request failed: {e:?}"),
            }
        }
        cx.notify();
    }
    fn poll_bridge(&mut self, cx: &mut Context<Self>) {
        self.poll_console_sessions(cx);
        self.navigator.sync_profiles(&self.workspace.profiles);
        self.check_table_open_context(cx);
        if self.navigator.poll() {
            self.sync_navigator_inspector(cx);
            cx.notify();
        }
        // At most 32 events per turn; all DB/network work belongs to the private runtime.
        for _ in 0..32 {
            let Some(event) = self.bridge.as_mut().and_then(DatabaseBridge::try_next) else {
                break;
            };
            if self.table_open_event(&event, cx) {
                continue;
            }
            match event {
                BridgeEvent::Configured { operation } => {
                    let table_configuration = self
                        .pending_table_open
                        .as_ref()
                        .is_some_and(|pending| pending.waiting_configured(operation));
                    if !dbide_ui_spike::workbench_model::complete_configuration(
                        &mut self.configuration_operation,
                        &mut self.pending_configuration,
                        &mut self.configured,
                        operation,
                    ) {
                        continue;
                    }
                    self.connection_revision = self.connection_revision.saturating_add(1);
                    self.last_execution = None;
                    if let Some(request) = self.configured.as_ref().cloned() {
                        if !table_configuration
                            && self
                                .workspace
                                .profiles
                                .iter()
                                .any(|p| p.id == request.profile.id)
                            && self.workspace.selected_profile_id.as_deref()
                                != Some(&request.profile.id)
                        {
                            let _ = self.workspace.select(Some(&request.profile.id));
                            self.persist_profile_mutation(
                                ProfileMutation::Select(request.profile.id.clone()),
                                cx,
                            );
                        }
                        self.passwords.remember(&request);
                        self.connection = ConnectionState::Disconnected;
                        self.status = format!("{} selected · not connected", request.profile.name);
                        self.transaction_state = ServerTransactionState::Unknown;
                        self.schema.clear();
                        self.structure = None;
                        self.expanded_schemas.clear();
                        self.expanded_objects.clear();
                        if !table_configuration {
                            self.completions.clear();
                            self.completion_source = None;
                            self.rename_preview = None;
                            self.sql_diagnostics.clear();
                        }
                        self.schema_operation = None;
                        self.schema_loading = false;
                        self.schema_requested = table_configuration;
                        if table_configuration {
                            self.commit_policy = CommitPolicy::Auto;
                        }
                        self.schema_truncated = false;
                    }
                    if table_configuration {
                        self.connect_after_configuration = false;
                        self.connect_pending_table(operation, cx);
                    } else if std::mem::take(&mut self.connect_after_configuration) {
                        self.connect(cx);
                    }
                }
                BridgeEvent::Connecting {
                    operation,
                    attempt,
                    target,
                } => {
                    if self.active_operation == Some(operation)
                        || self.connection_operation == Some(operation)
                    {
                        if self.active_operation.is_none() {
                            self.connection = ConnectionState::Connecting;
                        }
                        self.status = format!(
                            "Connecting to {} · attempt {}",
                            dbide_ui_spike::workbench_model::connection_target(&target),
                            attempt
                        );
                    }
                }
                BridgeEvent::ConnectionFailed {
                    operation,
                    diagnostic,
                } => {
                    if self.active_operation == Some(operation)
                        && self.table_operation.take().is_some()
                    {
                        self.active_operation = None;
                        self.active_execution = None;
                        self.connection = ConnectionState::Disconnected;
                        self.transaction_state = ServerTransactionState::Unknown;
                        self.connection_revision = self.connection_revision.saturating_add(1);
                        self.last_execution = None;
                        if let Some(editor) = self.table_editor.clone() {
                            editor.update(cx, |e, cx| {
                                e.fail(
                                    format!(
                                        "{}: {} · {}",
                                        dbide_ui_spike::workbench_model::connection_target(
                                            &diagnostic.target
                                        ),
                                        diagnostic.message,
                                        diagnostic.hint
                                    ),
                                    cx,
                                )
                            });
                        }
                        continue;
                    }

                    let schema_failure = dbide_ui_spike::settle_schema_operation(
                        &mut self.schema_operation,
                        &mut self.schema_loading,
                        operation,
                    );
                    let configuration_failure = self.configuration_operation == Some(operation);
                    if configuration_failure {
                        self.configuration_operation = None;
                        self.pending_configuration = None;
                        self.connect_after_configuration = false;
                    }
                    if schema_failure
                        || configuration_failure
                        || self.active_operation == Some(operation)
                        || self.connection_operation == Some(operation)
                    {
                        // Metadata uses a separate connection. Its failure must not
                        // retire the main session or clear an in-flight execution.
                        if !schema_failure && !configuration_failure {
                            self.connection = self
                                .before_test
                                .take()
                                .unwrap_or(ConnectionState::Disconnected);
                            self.connection_operation = None;
                            self.active_operation = None;
                            self.active_execution = None;
                            self.pending_sql = None;
                        }
                        self.diagnostic = Some(Diagnostic::new((
                            format!(
                                "{:?} · {}",
                                diagnostic.kind,
                                dbide_ui_spike::workbench_model::connection_target(
                                    &diagnostic.target
                                )
                            ),
                            diagnostic.message.into(),
                            diagnostic.hint.into(),
                        )));
                        if let Some(diagnostic) = &mut self.diagnostic {
                            diagnostic.context = if schema_failure {
                                DiagnosticContext::Schema
                            } else {
                                DiagnosticContext::Connection
                            };
                        }
                        self.status = if schema_failure {
                            "Schema refresh failed · review diagnostic below"
                        } else {
                            "Connection failed · review diagnostic below"
                        }
                        .into();
                    }
                }
                BridgeEvent::ConnectionTested { operation } => {
                    if self.connection_operation == Some(operation) {
                        self.connection_operation = None;
                        self.connection = self
                            .before_test
                            .take()
                            .unwrap_or(ConnectionState::Disconnected);
                        self.diagnostic = None;
                        self.form_error =
                            Some("Connection test succeeded. Save and connect when ready.".into());
                        self.status = "Connection test succeeded".into();
                    }
                }
                // Database discovery UI is introduced with the per-console context step.
                BridgeEvent::DatabasesLoaded { .. }
                | BridgeEvent::SchemasLoaded { .. }
                | BridgeEvent::ObjectsLoaded { .. }
                | BridgeEvent::ColumnsLoaded { .. } => {}
                BridgeEvent::SchemaLoaded {
                    operation,
                    objects,
                    truncated,
                } => {
                    if !dbide_ui_spike::settle_schema_operation(
                        &mut self.schema_operation,
                        &mut self.schema_loading,
                        operation,
                    ) {
                        continue;
                    }
                    self.expanded_schemas
                        .extend(objects.iter().map(|o| o.schema.clone()));
                    self.schema = objects;
                    self.schema_truncated = truncated;
                    self.status = if truncated {
                        "Schema loaded · object limit reached"
                    } else {
                        "Schema loaded"
                    }
                    .into();
                }
                BridgeEvent::Connected { operation } => {
                    if self.active_operation != Some(operation)
                        && self.connection_operation != Some(operation)
                    {
                        continue;
                    }
                    let table_connecting = self
                        .pending_table_open
                        .as_ref()
                        .is_some_and(|pending| pending.waiting_connected(operation));
                    self.connection_revision = self.connection_revision.saturating_add(1);
                    self.last_execution = None;
                    if self.active_operation.is_none() {
                        self.connection = ConnectionState::Connected;
                    }
                    if self.active_operation.is_none()
                        && !self.fixture_smoke
                        && let Some(bridge) = &self.bridge
                    {
                        match bridge.set_commit_policy(self.commit_policy) {
                            Ok(policy_operation) => {
                                self.policy_operation = Some(policy_operation);
                                if table_connecting
                                    && let Some(pending) = &mut self.pending_table_open
                                {
                                    pending.observe(operation, policy_operation);
                                }
                            }
                            Err(error) => {
                                if table_connecting {
                                    self.abort_table_open(format!("Table transaction-state observation was not submitted: {error:?}"),false,cx);
                                    continue;
                                }
                                self.status = format!(
                                    "Connected; transaction status synchronization failed: {error:?}"
                                );
                            }
                        }
                    }
                    self.connection_operation = None;
                    self.diagnostic = None;
                    self.status = format!(
                        "{} connected",
                        dbide_ui_spike::workbench_model::engine_name(self.engine())
                    );
                    if self.fixture_smoke {
                        self.submit_query(cx);
                        self.fixture_smoke = false;
                    }
                }
                BridgeEvent::Executing {
                    operation,
                    execution,
                } => {
                    if self.active_operation != Some(operation) {
                        continue;
                    }
                    self.active_execution = Some(execution);
                    self.last_execution = Some(execution);
                    if self.connection != ConnectionState::Disconnecting {
                        self.connection = ConnectionState::Executing;
                        self.status = format!("Query {} running", execution.execution);
                    }
                }
                BridgeEvent::Cancelling { execution, .. } => {
                    if self.active_execution != Some(execution) {
                        continue;
                    }
                    if self.connection != ConnectionState::Disconnecting {
                        self.connection = ConnectionState::Cancelling;
                        self.status = "Cancellation in progress…".into();
                    }
                }
                BridgeEvent::Finished { operation, report } => {
                    if self.active_operation != Some(operation) {
                        continue;
                    }
                    if let Some(kind) = self.table_operation.take() {
                        self.table_report(report, kind, cx);
                    } else {
                        self.accept_report(report, cx);
                    }
                    if !self.schema_requested && self.connection == ConnectionState::Connected {
                        self.refresh_schema(cx);
                    }
                }
                BridgeEvent::CommitPolicyChanged {
                    operation,
                    policy,
                    transaction_state,
                } => {
                    if self.policy_operation != Some(operation) {
                        continue;
                    }
                    self.policy_operation = None;
                    self.commit_policy = policy;
                    self.transaction_state = transaction_state;
                    self.status =
                        format!("Commit policy: {policy:?} · transaction: {transaction_state:?}");
                    let table_observed =
                        self.observe_pending_table(operation, transaction_state, cx);
                    if !table_observed && !self.schema_requested {
                        self.refresh_schema(cx);
                    }
                }
                BridgeEvent::Disconnected { operation } => {
                    if self
                        .table_disconnect_operation
                        .is_some_and(|expected| Some(expected) != operation)
                    {
                        continue;
                    }
                    if self.pending_table_open.is_some()
                        || (self.table_disconnect_operation.is_some()
                            && self.table_disconnect_operation == operation)
                    {
                        self.table_reader_disconnected(cx);
                    }
                    self.table_disconnect_operation = None;
                    self.connection_revision = self.connection_revision.saturating_add(1);
                    self.last_execution = None;
                    if let Some(editor) = self.table_editor.clone() {
                        editor.update(cx, |e, cx| {
                            e.fail(
                                "Connection closed. Reconnect and reopen the table before editing."
                                    .into(),
                                cx,
                            )
                        });
                    }
                    self.active_execution = None;
                    self.active_operation = None;
                    self.connection = ConnectionState::Disconnected;
                    self.transaction_state = ServerTransactionState::Unknown;
                    self.status = "Disconnected · result snapshot retained".into();
                }
                BridgeEvent::Error { operation, message } => {
                    if operation.is_some()
                        && operation == self.active_operation
                        && self.table_operation.take().is_some()
                    {
                        self.active_execution = None;
                        self.active_operation = None;
                        // No terminal report proves retirement; retain a potentially live
                        // table transaction as Unknown until explicit resolution.
                        self.connection = if self.configured.is_some() {
                            ConnectionState::Connected
                        } else {
                            ConnectionState::Disconnected
                        };
                        self.transaction_state = ServerTransactionState::Unknown;
                        self.connection_revision = self.connection_revision.saturating_add(1);
                        self.last_execution = None;
                        if let Some(feedback) = &mut self.table_open_feedback {
                            feedback.message = message.clone();
                            feedback.neutral = false;
                        }
                        if let Some(editor) = self.table_editor.clone() {
                            editor.update(cx, |e, cx| e.fail(message.clone(), cx));
                        }
                        continue;
                    }
                    // Late schema/configuration/cancel errors from a replaced request
                    // must not change this profile or overwrite the latest diagnostic.
                    if operation.is_some()
                        && operation != self.active_operation
                        && operation != self.connection_operation
                        && operation != self.configuration_operation
                        && operation != self.cancellation_operation
                        && operation != self.schema_operation
                        && operation != self.policy_operation
                    {
                        continue;
                    }
                    if operation.is_some() && operation == self.policy_operation {
                        self.policy_operation = None;
                    }
                    if operation.is_some() && operation == self.cancellation_operation {
                        self.cancellation_operation = None;
                        if self.active_operation.is_none() {
                            continue;
                        }
                    }
                    if operation.is_some() && operation == self.configuration_operation {
                        self.configuration_operation = None;
                        self.pending_configuration = None;
                        self.connect_after_configuration = false;
                    }
                    if self.connection == ConnectionState::Connecting
                        || operation.is_none()
                        || operation == self.active_operation
                    {
                        self.connection = ConnectionState::Disconnected;
                        self.active_execution = None;
                        self.active_operation = None;
                    } else if self.connection == ConnectionState::Cancelling {
                        // A stale cancellation is rejected without altering the current query.
                        self.connection = if self.active_execution.is_some() {
                            ConnectionState::Executing
                        } else {
                            ConnectionState::Connected
                        };
                    }
                    self.schema_loading = false;
                    self.status = format!("Database error: {message}");
                    self.diagnostic = Some(Diagnostic::new((
                        "Database operation failed".into(),
                        message.clone(),
                        "Review connection settings, then retry the operation.".into(),
                    )));
                    self.details = message;
                }
            }
            cx.notify();
        }
    }
    fn select_page(&mut self, next: bool, cx: &mut Context<Self>) {
        if self.pages.is_empty() {
            return;
        }
        self.selected_page = if next {
            (self.selected_page + 1).min(self.pages.len() - 1)
        } else {
            self.selected_page.saturating_sub(1)
        };
        self.set_mode(false, cx);
    }
    fn metrics(&self, cx: &App) -> Metrics<'_> {
        let table = self.grid.read(cx);
        let grid = table.delegate();
        Metrics {
            schema_version: 2,
            fixture: if self.logical_mode {
                "logical-grid-v1"
            } else {
                "database-workbench-v3"
            },
            logical_rows: grid.row_count(),
            logical_columns: grid.columns.len(),
            retained_fixture_rows: if self.logical_mode {
                0
            } else {
                self.summary.as_ref().map_or(0, |s| s.retained_rows)
            },
            retained_column_descriptors: grid.columns.len(),
            visible_rows: bounded_range(table.visible_range().rows().clone(), grid.row_count()),
            visible_columns: logical_column_range(
                table.visible_range().cols().clone(),
                usize::from(!grid.columns.is_empty()),
                grid.columns.len(),
            ),
            generated_cell_render_calls: grid.cell_render_calls,
            sql_utf8_bytes: self.editor.read(cx).value().len(),
            input_change_events: self.input_changes,
            root_render_calls: self.render_count,
            elapsed_ms: self.started.elapsed().as_secs_f64() * 1000.,
            first_root_render_ms: self.first_render.map(|d| d.as_secs_f64() * 1000.),
            forced_redraw_intervals_ms: self.frame_intervals.iter().copied().collect(),
            query: self.summary.as_ref(),
            limitations: "Root render/forced-redraw intervals are not key-to-paint/GPU presentation. Ranges describe rendered cells including partial/overscan, columns normalized after pinned ID. Accessibility semantics exist but VoiceOver/IME require native validation. No performance parity claim.",
        }
    }
    fn report_metrics(&mut self, _: &ReportMetrics, _: &mut Window, cx: &mut Context<Self>) {
        println!(
            "{}",
            serde_json::to_string(&self.metrics(cx)).expect("serialize metrics")
        );
        self.status = "Metrics written to stdout".into();
        cx.notify();
    }
    fn copy_cell(&mut self, _: &CopyCell, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((row, col)) = self.grid.read(cx).selected_cell() {
            let text = self.grid.read(cx).delegate().value(row, col);
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.status = format!("Copied row {}, column {}", row + 1, col + 1);
            cx.notify();
        }
    }
    fn jump(&mut self, last: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.grid.update(cx, |grid, cx| {
            let rows = grid.delegate().row_count();
            let cols = grid.delegate().columns.len();
            if rows == 0 || cols == 0 {
                return;
            }
            let row = if last { rows - 1 } else { 0 };
            let col = if last { cols - 1 } else { 0 };
            grid.set_selected_cell(row, col, cx);
            grid.scroll_to_row(row, cx);
            grid.scroll_to_col(col, cx);
            grid.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }
    fn busy(&self) -> bool {
        self.recovery.close_after
            || self.configuration_operation.is_some()
            || self.policy_operation.is_some()
            || matches!(
                self.connection,
                ConnectionState::Connecting
                    | ConnectionState::Executing
                    | ConnectionState::Cancelling
                    | ConnectionState::Disconnecting
            )
    }
    fn document_profile_matches(&self) -> bool {
        dbide_ui_spike::workbench_model::profile_binding_matches(
            self.documents
                .iter()
                .find(|d| d.model.id == self.active_document)
                .and_then(|d| d.required_profile_id.as_deref()),
            self.console().target().map(|p| p.id.as_str()),
        )
    }
    fn document_connection_label(&self) -> String {
        self.console_context_label()
    }
    fn can_run(&self, cx: &App) -> bool {
        if !self.fixture_smoke {
            return self.console().target().is_some()
                && !self.console().busy()
                && !self.recovery.close_after
                && self.tools.is_none()
                && self.active_table_tab.is_none()
                && !self.editor.read(cx).value().trim().is_empty();
        }
        dbide_ui_spike::can_submit_query(
            self.bridge.is_some(),
            self.configured.is_some(),
            self.busy() || self.tools.is_some() || !self.document_profile_matches(),
            &self.editor.read(cx).value(),
        )
    }
    fn button(&self, id: &'static str, label: &'static str) -> Button {
        Button::new(id).label(self.ui_label(label)).small().ghost()
    }
    fn transaction_may_be_open(&self) -> bool {
        dbide_ui_spike::workbench_model::transaction_may_be_open(
            self.transaction_state,
            self.connection == ConnectionState::Connected,
        )
    }
    fn configure(&mut self, request: ConnectionRequest, cx: &mut Context<Self>) -> bool {
        if self.busy() || self.profile_write_busy || self.pending_table_open.is_some() {
            return false;
        }
        if self.transaction_may_be_open() {
            self.diagnostic=Some(Diagnostic::new(("Resolve the current transaction state".into(),"The active connection has not changed.".into(),"Rollback resynchronizes an Unknown/Failed live session. Commit requires Active state. Resolve it or explicitly Disconnect before changing profiles.".into())));
            cx.notify();
            return false;
        }
        if let Some(bridge) = &self.bridge {
            match bridge.configure(request.clone()) {
                Ok(operation) => {
                    self.configuration_operation = Some(operation);
                    self.pending_configuration = Some(request);
                    cx.notify();
                    return true;
                }
                Err(e) => self.status = format!("Configure failed: {e:?}"),
            }
        }
        cx.notify();
        false
    }
    fn edit_profile(&mut self, new: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() || self.profile_write_busy {
            return;
        }
        let profile = if new {
            ConnectionProfile::new()
        } else {
            self.configured
                .as_ref()
                .map(|r| r.profile.clone())
                .unwrap_or_default()
        };
        let password = self.passwords.get(&profile).unwrap_or_default();
        self.form = Some(ProfileForm::new(profile, &password, window, cx));
        self.form_error = None;
        cx.notify();
    }
    fn save_profile(&mut self, connect: bool, cx: &mut Context<Self>) {
        let Some(form) = &self.form else {
            return;
        };
        match form.request(cx) {
            Ok(request) => {
                if self.profile_write_busy || !self.profile_writes.is_empty() {
                    self.form_error =
                        Some("A profile change is being saved. Try again when it finishes.".into());
                    cx.notify();
                    return;
                }
                let remember = self.form.as_ref().is_some_and(|f| f.remember_password);
                self.profile_writes.push_back(ProfileWrite {
                    mutation: ProfileMutation::Upsert(request.profile.clone()),
                    activate: Some((request, connect, remember)),
                });
                self.form_error = Some("Saving profile…".into());
                self.start_profile_write(cx);
            }
            Err(e) => self.form_error = Some(e),
        }
        cx.notify();
    }
    fn test_profile(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let request = if let Some(form) = &self.form {
            form.request(cx)
        } else {
            self.configured
                .clone()
                .ok_or_else(|| "Add a connection first".into())
        };
        match request {
            Ok(request) => {
                if let Some(bridge) = &self.bridge {
                    match bridge.test_connection(request) {
                        Ok(operation) => {
                            self.connection_operation = Some(operation);
                            self.before_test = Some(self.connection);
                            self.connection = ConnectionState::Connecting;
                            self.diagnostic = None;
                            self.form_error = None;
                            self.status = "Testing connection…".into();
                        }
                        Err(e) => self.form_error = Some(format!("Test failed: {e:?}")),
                    }
                }
            }
            Err(e) => self.form_error = Some(e),
        }
        cx.notify();
    }
    fn remove_profile(&mut self, cx: &mut Context<Self>) {
        let Some(profile) = self
            .form
            .as_ref()
            .map(|f| f.original.clone())
            .or_else(|| self.configured.as_ref().map(|r| r.profile.clone()))
        else {
            return;
        };
        let browser_matches = self
            .configured
            .as_ref()
            .is_some_and(|r| r.profile.id == profile.id);
        if self.busy() || (browser_matches && self.transaction_may_be_open()) {
            self.form_error=Some("Finish the active browser operation or disconnect its transaction before removing this connection.".into());
            cx.notify();
            return;
        }
        self.workspace.remove(&profile.id);
        self.passwords.forget_profile(&profile.id);
        self.persist_profile_mutation(ProfileMutation::Remove(profile.id.clone()), cx);
        if browser_matches {
            self.disconnect(cx);
            self.configured = None;
            self.schema.clear();
        }
        self.deleted = Some(profile);
        self.form = None;
        self.navigator.sync_profiles(&self.workspace.profiles);
        cx.notify();
    }
    fn refresh_schema(&mut self, cx: &mut Context<Self>) {
        self.schema_requested = true;
        self.navigator.sync_profiles(&self.workspace.profiles);
        if let Some(key) = self
            .configured
            .as_ref()
            .and_then(|r| self.navigator.root(&r.profile.id))
        {
            self.navigator_expand(key, true, cx);
        }
        cx.notify();
    }
}
impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self
            .active_table_tab
            .map(|id| (true, id))
            .unwrap_or((false, self.active_document));
        let tab_space = (
            f32::from(window.viewport_size().width) as u16,
            self.app_settings.sidebar_width,
            self.preferences.is_some(),
            self.structure.is_some(),
            self.catalog_collapsed,
        );
        if self.last_visible_tab != Some(visible) || self.last_tab_space != Some(tab_space) {
            let index = if visible.0 {
                self.table_tabs
                    .iter()
                    .position(|t| t.id == visible.1)
                    .map(|i| i + self.documents.len())
            } else {
                self.documents.iter().position(|d| d.model.id == visible.1)
            };
            if let Some(index) = index {
                self.tab_scroll.scroll_to_item(index);
            }
            self.last_visible_tab = Some(visible);
            self.last_tab_space = Some(tab_space);
        }

        self.refresh_core_placeholders(window, cx);
        let normal = self.active_table_tab.is_none() && self.tools.is_none();
        let menu_state = (
            normal,
            normal && self.can_run(cx),
            if self.active_table_tab.is_some() {
                self.active_operation.is_some()
            } else {
                self.console().active_operation.is_some()
            },
        );
        if self.menu_state != Some(menu_state) {
            cx.set_menus(native_menus::menus(
                self.locale,
                menu_state.0,
                menu_state.1,
                menu_state.2,
            ));
            self.menu_state = Some(menu_state);
        }
        self.render_count += 1;
        self.first_render
            .get_or_insert_with(|| self.started.elapsed());
        if self.recording {
            let now = Instant::now();
            if let Some(last) = self.last_frame.replace(now) {
                if self.frame_intervals.len() == 10_000 {
                    self.frame_intervals.pop_front();
                }
                self.frame_intervals
                    .push_back((now - last).as_secs_f64() * 1000.);
            }
            window.request_animation_frame();
        }
        let table = self.grid.read(cx);
        let grid = table.delegate();
        let rows = grid.row_count();
        let cols = grid.columns.len();
        let rendered_rows = bounded_range(table.visible_range().rows().clone(), rows);
        let rendered_columns = logical_column_range(
            table.visible_range().cols().clone(),
            usize::from(cols > 0),
            cols,
        );
        let range_label = if rendered_rows.is_empty() {
            "No rendered rows".into()
        } else {
            format!(
                "Rendered rows {}–{} · columns {}–{} · first column pinned",
                rendered_rows.start + 1,
                rendered_rows.end,
                rendered_columns.start + 1,
                rendered_columns.end
            )
        };
        let mut result_label = if self.logical_mode {
            "Logical grid · 1,000,000 × 200".into()
        } else if let Some(page) = self.pages.get(self.selected_page) {
            format!(
                "{} {} / {} · {}{}",
                tr(self.locale, Message::Results),
                self.selected_page + 1,
                self.pages.len(),
                if page.command_only {
                    if self.locale == Locale::Ko {
                        "명령 완료"
                    } else {
                        "Command"
                    }
                    .into()
                } else {
                    if self.locale == Locale::Ko {
                        format!("보관된 {}행", page.rows.len())
                    } else {
                        format!("{} retained rows", page.rows.len())
                    }
                },
                page.affected_rows
                    .map(|n| if self.locale == Locale::Ko {
                        format!(" · 행 수 {n}")
                    } else {
                        format!(" · count {n}")
                    })
                    .unwrap_or_default()
            )
        } else {
            self.ui_label("Query results").to_string()
        };
        if let Some(owner) = &self.result_owner {
            result_label = format!("{} · {}", result_label, owner.label());
        }
        let command_only = !self.logical_mode
            && self
                .pages
                .get(self.selected_page)
                .is_some_and(|p| p.command_only);
        let busy = self.busy();
        let query_busy = self.console().busy();
        let query_configured = self.console().target().is_some();
        let sidebar =
            div()
                .w(px(self.app_settings.sidebar_width as f32))
                .flex_shrink_0()
                .bg(rgb(PANEL))
                .border_r_1()
                .border_color(rgb(BORDER))
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(48.))
                        .px_4()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(self.ui_label("Connections")),
                        )
                        .child(
                            self.button("new-profile", "+ New")
                                .disabled(busy)
                                .on_click(cx.listener(|s, _, w, cx| s.edit_profile(true, w, cx))),
                        ),
                )
                .child(self.database_explorer(cx))
                .when(self.deleted.is_some(), |el| {
                    el.child(self.button("undo-delete", "Undo profile removal").on_click(
                        cx.listener(|s, _, _, cx| {
                            if let Some(p) = s.deleted.take() {
                                let _ = s.workspace.upsert(p.clone());
                                s.persist_profile_mutation(ProfileMutation::Upsert(p), cx);
                                cx.notify();
                            }
                        }),
                    ))
                })
                .child(
                    div()
                        .px_3()
                        .py_3()
                        .border_t_1()
                        .border_color(rgb(BORDER))
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            self.button("history-toggle", "History")
                                .on_click(cx.listener(|s, _, _, cx| {
                                    s.show_history = !s.show_history;
                                    if s.show_history {
                                        s.show_sql_tools = false;
                                        s.transfer_preview = None;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(self.button("preferences-toggle", "Preferences").on_click(
                            cx.listener(|s, _, w, cx| {
                                s.open_preferences_action(&OpenPreferences, w, cx)
                            }),
                        ))
                        .child(
                            self.button("advanced-toggle", "Advanced")
                                .on_click(cx.listener(|s, _, _, cx| {
                                    s.show_tools = !s.show_tools;
                                    cx.notify();
                                })),
                        )
                        .child(
                            self.button("tools-toggle", "Tools")
                                .disabled(
                                    self.configuration_operation.is_some()
                                        || self.pending_table_open.is_some(),
                                )
                                .on_click(cx.listener(|s, _, w, cx| s.open_tools(w, cx))),
                        ),
                );

        let result_body = div()
            .id("result-grid")
            .role(Role::Grid)
            .aria_label(if self.logical_mode {
                "Logical stress test results"
            } else {
                "Database query results"
            })
            .aria_row_count(rows.saturating_add(1))
            .aria_column_count(cols)
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .font_family("Menlo")
            .key_context("FixtureGrid")
            .on_action(cx.listener(Self::copy_cell))
            .when(command_only, |el| {
                el.child(
                    div()
                        .p_6()
                        .text_color(rgb(MUTED))
                        .child("Command completed. Affected row count appears in the result tab."),
                )
            })
            .when(
                !command_only && (self.logical_mode || !self.pages.is_empty()),
                |el| {
                    el.child(
                        DataTable::new(&self.grid)
                            .small()
                            .stripe(false)
                            .bordered(false),
                    )
                },
            )
            .when(!self.logical_mode && self.pages.is_empty(), |el| {
                el.child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .child(
                            div()
                                .text_size(px(28.))
                                .text_color(rgb(0x586984))
                                .child("▤"),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(15.))
                                .child(self.ui_label(if query_busy {
                                    "Query in progress"
                                } else {
                                    "Your results appear here"
                                })),
                        )
                        .child(div().text_color(rgb(MUTED)).child(self.ui_label(if query_busy {
                            "Waiting for the database. You can cancel this execution."
                        } else if query_configured {
                            "Run the editor with ⌘Enter. A disconnected session reconnects first."
                        } else {
                            "Create a connection, then run your first query."
                        }))),
                )
            });
        let mut content = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.console_context_bar(cx))
            .child(self.query_toolbar(cx))
            .when(
                self.pending_table_open.is_some() || self.table_open_feedback.is_some(),
                |el| el.child(self.table_open_banner(cx)),
            )
            .child(
                div()
                    .when(!self.results_collapsed, |el| {
                        el.h(px(workbench_shell::editor_panel_height(
                            self.app_settings.editor_height,
                            f32::from(window.viewport_size().height),
                        ) as f32))
                            .flex_shrink_0()
                    })
                    .when(self.results_collapsed, |el| el.flex_1())
                    .relative()
                    .min_h_0()
                    .p_3()
                    .font_family("Menlo")
                    .child(
                        Editor::new(&self.editor)
                            .aria_label("SQL query editor")
                            .text_size(px(self.app_settings.editor_font_size as f32))
                            .h(relative(1.))
                            .appearance(false)
                            .bordered(false),
                    )
                    .when(!self.completions.is_empty(), |el| {
                        el.child(self.completion_popup(cx))
                    }),
            )
            .when(!self.results_collapsed, |el| {
                el.child(self.split_handle(false, cx))
            })
            .child(
                div()
                    .h(px(34.))
                    .flex_shrink_0()
                    .px_3()
                    .border_y_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(PANEL))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_weight(FontWeight::MEDIUM)
                            .child(result_label),
                    )
                    .child(
                        div()
                            .id("result-actions")
                            .min_w_0()
                            .max_w(relative(0.8))
                            .overflow_x_scroll()
                            .flex()
                            .gap_1()
                            .child(
                                self.button(
                                    "toggle-results",
                                    if self.results_collapsed { "▴" } else { "▾" },
                                )
                                .tooltip(if self.locale == Locale::Ko {
                                    "결과 영역 접기/펼치기 · ⌘⌥J"
                                } else {
                                    "Toggle results · ⌘⌥J"
                                })
                                .on_click(cx.listener(
                                    |s, _, w, cx| s.toggle_results(&ToggleResults, w, cx),
                                )),
                            )
                            .child(
                                self.button("prev-result", "‹")
                                    .disabled(self.selected_page == 0)
                                    .on_click(cx.listener(|s, _, _, cx| s.select_page(false, cx))),
                            )
                            .child(
                                self.button("next-result", "›")
                                    .disabled(self.selected_page + 1 >= self.pages.len())
                                    .on_click(cx.listener(|s, _, _, cx| s.select_page(true, cx))),
                            )
                            .child(
                                self.button("copy", "Copy cell")
                                    .disabled(self.grid.read(cx).selected_cell().is_none())
                                    .on_click(
                                        cx.listener(|s, _, w, cx| s.copy_cell(&CopyCell, w, cx)),
                                    ),
                            )
                            .child(self.result_more_menu(cx)),
                    ),
            )
            .when(!self.results_collapsed, |el| el.child(result_body));
        if self.console().diagnostic.is_some()
            && !self.fixture_smoke
            && !self
                .diagnostic
                .as_ref()
                .is_some_and(|d| d.context == DiagnosticContext::Capacity)
        {
            content = content.child(self.console_diagnostic(cx));
        } else if let Some(diagnostic) = &self.diagnostic {
            let (title, message, hint) = &diagnostic.body;
            content = content.child(
                div()
                    .id("diagnostic")
                    .role(Role::Alert)
                    .aria_label(format!("{title}. {message}. {hint}"))
                    .flex_shrink_0()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(0x654439))
                    .bg(rgb(0x302522))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xf0b797))
                            .child(title.clone()),
                    )
                    .child(div().text_color(rgb(TEXT)).child(message.clone()))
                    .child(div().text_color(rgb(MUTED)).child(hint.clone()))
                    .child(self.diagnostic_controls(cx)),
            );
        }
        if self.summary.is_some() {
            content = content.child(
                div()
                    .id("query-details")
                    .role(Role::Status)
                    .aria_label(self.details.clone())
                    .max_h(px(75.))
                    .overflow_y_scroll()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child(self.details.clone()),
            );
        }
        if self.show_tools {
            content = content.child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        self.button(
                            "logical-mode",
                            if self.logical_mode {
                                "Return to results"
                            } else {
                                "Grid stress test"
                            },
                        )
                        .on_click(cx.listener(|s, _, _, cx| s.set_mode(!s.logical_mode, cx))),
                    )
                    .child(
                        self.button("origin", "Origin")
                            .on_click(cx.listener(|s, _, w, cx| s.jump(false, w, cx))),
                    )
                    .child(
                        self.button("last", "Last cell")
                            .on_click(cx.listener(|s, _, w, cx| s.jump(true, w, cx))),
                    )
                    .child(
                        self.button(
                            "record",
                            if self.recording {
                                "Stop probe"
                            } else {
                                "Redraw probe"
                            },
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.recording = !s.recording;
                            s.last_frame = None;
                            cx.notify();
                        })),
                    )
                    .child(self.button("metrics", "Metrics").on_click(
                        cx.listener(|s, _, w, cx| s.report_metrics(&ReportMetrics, w, cx)),
                    ))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child(range_label),
                    ),
            );
        }
        if let Some(editor) = self.active_table_entity() {
            content = div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .when(
                    self.pending_table_open.is_some() || self.table_open_feedback.is_some(),
                    |el| el.child(self.table_open_banner(cx)),
                )
                .child(self.table_editor_view(&editor, cx));
        } else if let Some(tools) = &self.tools {
            content = self.captured_tools_view(tools, cx);
        }
        let content = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.document_tabs(cx))
            .child(content);
        let mut body = div()
            .flex()
            .flex_1()
            .min_h_0()
            .when(!self.catalog_collapsed, |el| {
                el.child(sidebar).child(self.split_handle(true, cx))
            })
            .child(content);
        if let Some(preferences) = &self.preferences {
            body = body.child(self.preferences_panel(preferences, cx));
        } else if self.form.is_some() {
            body = body.child(self.connection_form_panel(cx));
        } else if let Some(inspector) = &self.structure {
            body = body.child(
                div()
                    .w(px(760.))
                    .max_w(relative(0.58))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .bg(rgb(PANEL))
                    .border_l_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(if self.locale == Locale::Ko {
                                "테이블 구조"
                            } else {
                                "Object structure"
                            })
                            .child(
                                self.button("close-structure", "Close")
                                    .on_click(cx.listener(|s, _, _, cx| {
                                        s.structure = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(div().flex_1().min_h_0().child(inspector.clone())),
            );
        } else if let Some(preview) = &self.transfer_preview {
            body = body.child(self.transfer_panel(preview, cx));
        } else if self.show_sql_tools {
            body = body.child(self.sql_tools_panel(cx));
        } else if self.show_history {
            body = body.child(
                div()
                    .w(px(340.))
                    .flex_shrink_0()
                    .bg(rgb(PANEL))
                    .border_l_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .p_4()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.ui_label("Query history")),
                    )
                    .child(
                        div()
                            .px_4()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(if self.workspace.persist_history {
                                "History saved locally. SQL text may contain sensitive values."
                            } else {
                                "Session only. SQL history is not written to disk."
                            }),
                    )
                    .child(
                        div().p_3().child(
                            self.button(
                                "persist-history",
                                if self.workspace.persist_history {
                                    "Stop saving history"
                                } else {
                                    "Save history locally"
                                },
                            )
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.workspace.persist_history = !s.workspace.persist_history;
                                s.persist_profile_mutation(
                                    ProfileMutation::PersistHistory(
                                        s.workspace.persist_history,
                                        s.workspace.history.clone(),
                                    ),
                                    cx,
                                );
                                cx.notify();
                            })),
                        ),
                    )
                    .child(
                        div()
                            .id("history-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px_3()
                            .children(self.workspace.history.iter().rev().enumerate().map(
                                |(ix, entry)| {
                                    let sql = entry.sql.clone();
                                    let history_profile=entry.profile_id.clone();
                                    let history_target=entry.execution_target.clone();
                                    div()
                                        .id(("history", ix))
                                        .role(Role::Button)
                                        .aria_label(format!("Load query: {}", entry.sql))
                                        .p_3()
                                        .mb_2()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(BORDER))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(rgb(0x252a33)))
                                        .on_click(cx.listener(move |s, _, w, cx| {
                                            if s.documents.len()>=32 {s.show_sql_tab_limit(cx);return;}
                                            s.new_document(sql.clone(),"history.sql".into(),false,w,cx);
                                            let request=history_target.as_ref().and_then(|t|s.workspace.profiles.iter().find(|p|p.id==t.profile_id).and_then(|p|t.resolve(p).ok())).map(|profile|ConnectionRequest{profile,password:String::new()});
                                            if let Some(doc)=s.documents.iter_mut().find(|d|d.model.id==s.active_document){doc.required_profile_id=Some(history_profile.clone());doc.session=ConsoleSession::new(doc.model.id);if let Some(request)=request{let _=doc.session.bind(doc.session.route(),request);}let database=doc.session.target().map(|p|p.database.clone()).unwrap_or_default();doc.database_draft.update(cx,|input,cx|input.set_value(database,w,cx));doc.password_draft.update(cx,|input,cx|input.set_value(String::new(),w,cx));}
                                            s.status="History SQL opened without execution; Run requires its original profile.".into();
                                        }))
                                        .child(
                                            div().text_color(rgb(MUTED)).text_size(px(10.)).child(
                                                format!(
                                                    "{} · {} rows",
                                                    entry.outcome,
                                                    entry.row_count.unwrap_or(0)
                                                ),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .mt_2()
                                                .max_h(px(75.))
                                                .overflow_hidden()
                                                .font_family("Menlo")
                                                .text_size(px(11.))
                                                .child(entry.sql.clone()),
                                        )
                                },
                            )),
                    ),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family(".AppleSystemUIFont")
            .text_size(px(12.))
            .on_drop::<ExternalPaths>(cx.listener(Self::drop_sql_files))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.border_2().border_color(rgb(ACCENT)))
            .on_action(cx.listener(Self::open_preferences_action))
            .on_action(cx.listener(Self::new_sql_action))
            .on_action(cx.listener(Self::open_sql_action))
            .on_action(cx.listener(Self::save_sql_action))
            .on_action(cx.listener(Self::save_sql_as_action))
            .on_action(cx.listener(Self::close_sql_action))
            .on_action(cx.listener(Self::quit_action))
            .on_action(cx.listener(Self::report_metrics))
            .on_action(cx.listener(Self::run_query))
            .on_action(cx.listener(Self::cancel_query_action))
            .on_action(cx.listener(Self::run_current_action))
            .on_action(cx.listener(Self::format_sql_action))
            .on_action(cx.listener(Self::complete_sql_action))
            .on_action(cx.listener(Self::refresh_completion_metadata_action))
            .on_action(cx.listener(Self::next_workspace_tab))
            .on_action(cx.listener(Self::previous_workspace_tab))
            .on_action(cx.listener(Self::focus_catalog))
            .on_action(cx.listener(Self::focus_results))
            .on_action(cx.listener(Self::toggle_results))
            .on_action(cx.listener(Self::toggle_catalog))
            .capture_key_down(cx.listener(Self::completion_key_down))
            .capture_action(cx.listener(Self::completion_move_up))
            .capture_action(cx.listener(Self::completion_move_down))
            .capture_action(cx.listener(Self::completion_enter))
            .capture_action(cx.listener(Self::completion_tab))
            .capture_action(cx.listener(Self::completion_escape))
            .on_mouse_move(cx.listener(Self::resize_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::resize_end))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::resize_end))
            .child(
                div()
                    .h(px(34.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                self.button("toggle-catalog", "☰")
                                    .tooltip(if self.locale == Locale::Ko {
                                        "탐색기 표시 · ⌘B"
                                    } else {
                                        "Toggle catalog · ⌘B"
                                    })
                                    .on_click(cx.listener(|s, _, w, cx| {
                                        s.toggle_catalog(&ToggleCatalog, w, cx)
                                    })),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.ui_label("Database IDE")),
                            )
                            .child(div().text_color(rgb(MUTED)).child("/"))
                            .child(
                                div()
                                    .text_color(rgb(MUTED))
                                    .child(self.ui_label("Local workspace")),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child(dbide_ui_spike::workbench_model::engine_name(self.engine())),
                    ),
            )
            .child(body)
            .child(
                div()
                    .id("status")
                    .role(Role::Status)
                    .aria_label(self.active_console_status_label())
                    .h(px(28.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(PANEL))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child(self.active_console_status_label()),
                    )
                    .child(
                        div()
                            .max_w(px(470.))
                            .text_ellipsis()
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child(self.recovery_status_label()),
                    )
                    .child(div().text_size(px(10.)).text_color(rgb(MUTED)).child(
                        if self.locale == Locale::Ko {
                            "탐색기: 펼칠 때 조회 · 캐시 사용"
                        } else {
                            "Catalog: on-demand · cached"
                        },
                    )),
            )
            .into_any_element()
    }
}
fn main() {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    let smoke_seconds = args.iter().find_map(|arg| {
        arg.strip_prefix("--smoke-seconds=").map(|n| {
            n.parse::<u64>()
                .expect("--smoke-seconds requires a positive integer")
        })
    });
    let fixture_smoke = args.iter().any(|arg| arg == "--fixture-smoke");
    let application = gpui_platform::application().with_assets(Assets);
    application.on_reopen(|cx| {
        if cx.windows().is_empty() {
            open_workbench(cx, Instant::now(), false, None);
        }
    });
    application.run(move |cx: &mut App| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        cx.on_action(|_: &Quit, cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        });
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("ctrl-tab", NextWorkspaceTab, None),
            KeyBinding::new("ctrl-shift-tab", PreviousWorkspaceTab, None),
            KeyBinding::new("cmd-shift-l", FocusCatalog, None),
            KeyBinding::new("cmd-alt-r", FocusResults, None),
            KeyBinding::new("cmd-alt-j", ToggleResults, None),
            KeyBinding::new("cmd-b", ToggleCatalog, None),
            KeyBinding::new("cmd-.", CancelQuery, None),
            KeyBinding::new("cmd-.", CancelQuery, Some("Input")),
            KeyBinding::new("cmd-,", OpenPreferences, None),
            KeyBinding::new("cmd-,", OpenPreferences, Some("Input")),
            KeyBinding::new("cmd-shift-m", ReportMetrics, None),
            KeyBinding::new("cmd-c", CopyCell, Some("FixtureGrid")),
            KeyBinding::new("cmd-enter", RunQuery, None),
            KeyBinding::new("cmd-enter", RunQuery, Some("Input")),
            KeyBinding::new("cmd-shift-enter", RunCurrent, None),
            KeyBinding::new("cmd-shift-enter", RunCurrent, Some("Input")),
            KeyBinding::new("cmd-j", CompleteSql, None),
            KeyBinding::new("cmd-j", CompleteSql, Some("Input")),
            KeyBinding::new("ctrl-space", CompleteSql, None),
            KeyBinding::new("ctrl-space", CompleteSql, Some("Input")),
            KeyBinding::new("cmd-shift-f", FormatSql, None),
            KeyBinding::new("cmd-o", OpenSqlFile, None),
            KeyBinding::new("cmd-o", OpenSqlFile, Some("Input")),
            KeyBinding::new("cmd-s", SaveSqlFile, None),
            KeyBinding::new("cmd-s", SaveSqlFile, Some("Input")),
            KeyBinding::new("cmd-shift-s", SaveSqlAs, None),
            KeyBinding::new("cmd-shift-s", SaveSqlAs, Some("Input")),
            KeyBinding::new("cmd-n", NewSqlFile, None),
            KeyBinding::new("cmd-n", NewSqlFile, Some("Input")),
            KeyBinding::new("cmd-t", NewSqlFile, None),
            KeyBinding::new("cmd-t", NewSqlFile, Some("Input")),
            KeyBinding::new("cmd-w", CloseSqlTab, None),
            KeyBinding::new("cmd-w", CloseSqlTab, Some("Input")),
        ]);
        open_workbench(cx, started, fixture_smoke, smoke_seconds);
    });
}

fn open_workbench(cx: &mut App, started: Instant, fixture_smoke: bool, smoke_seconds: Option<u64>) {
    let bounds = Bounds::centered(None, size(px(1440.), px(940.)), cx);
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(1360.), px(800.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Rust Database IDE".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| Workbench::new(window, cx, started, fixture_smoke));
                // Fallback actions also work when titlebar/background focus leaves no
                // focused editor in the dispatch tree. Defer until dispatch releases Window.
                let global_window = window.window_handle();
                macro_rules! fallback_action {
                    ($action:ty,$handler:ident,$normal_only:expr) => {{
                        let weak = view.downgrade();
                        cx.on_action(move |action: &$action, cx| {
                            if cx.active_window() == Some(global_window) {
                                let weak = weak.clone();
                                let action = action.clone();
                                cx.defer(move |cx| {
                                    let _ = global_window.update(cx, |_, w, cx| {
                                        let _ = weak.update(cx, |s, cx| {
                                            if !$normal_only
                                                || (s.active_table_tab.is_none()
                                                    && s.tools.is_none())
                                            {
                                                s.$handler(&action, w, cx);
                                            }
                                        });
                                    });
                                });
                            }
                        });
                    }};
                }
                fallback_action!(Quit, quit_action, false);
                fallback_action!(OpenPreferences, open_preferences_action, false);
                fallback_action!(OpenSqlFile, open_sql_action, true);
                fallback_action!(NewSqlFile, new_sql_action, true);
                fallback_action!(SaveSqlFile, save_sql_action, true);
                fallback_action!(SaveSqlAs, save_sql_as_action, true);
                fallback_action!(CloseSqlTab, close_sql_action, false);
                fallback_action!(RunQuery, run_query, true);
                fallback_action!(RunCurrent, run_current_action, true);
                fallback_action!(FormatSql, format_sql_action, true);
                fallback_action!(CompleteSql, complete_sql_action, true);
                fallback_action!(
                    RefreshCompletionMetadata,
                    refresh_completion_metadata_action,
                    true
                );
                fallback_action!(NextWorkspaceTab, next_workspace_tab, false);
                fallback_action!(PreviousWorkspaceTab, previous_workspace_tab, false);
                fallback_action!(FocusCatalog, focus_catalog, false);
                fallback_action!(FocusResults, focus_results, false);
                fallback_action!(ToggleResults, toggle_results, true);
                fallback_action!(ToggleCatalog, toggle_catalog, false);
                fallback_action!(CancelQuery, cancel_query_action, false);
                let close_view = view.downgrade();
                window.on_window_should_close(cx, move |window, cx| {
                    close_view
                        .update(cx, |s, cx| s.request_window_close(window, cx))
                        .unwrap_or(true)
                });
                let weak = view.downgrade();
                if let Some(seconds) = smoke_seconds {
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(Duration::from_secs(seconds.max(1)))
                            .await;
                        let _ = weak.update(cx, |view, cx| {
                            println!(
                                "{}",
                                serde_json::to_string(&view.metrics(cx))
                                    .expect("serialize smoke metrics")
                            );
                            cx.quit();
                        });
                    })
                    .detach();
                }
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("open native GPUI window");
    window
        .update(cx, |_, window, _| window.activate_window())
        .expect("activate window");
    cx.activate(true);
}
