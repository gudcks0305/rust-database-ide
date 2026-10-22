use super::*;
use dbide_data::{
    Cell, Edit, FilterOperator, InsertValue, SortDirection, TABLE_PAGE_SIZES, TableFilter,
    TableQuery, TableSort,
};
use dbide_ui_spike::table_edit_model::{Staging, VerifiedTable, cell_text};
use gpui_component::input::{Textarea, TextareaState};
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit_assets::IconName;

// Keep application captions separate from backend data so locale changes can redraw
// existing statuses without rewriting SQL, cell contents, identifiers or diagnostics.
#[derive(Clone)]
enum TableText {
    Message(Message),
    Formatted(Message, Vec<(&'static str, TableText)>),
    Raw(String),
}
impl From<Message> for TableText {
    fn from(value: Message) -> Self {
        Self::Message(value)
    }
}
impl From<String> for TableText {
    fn from(value: String) -> Self {
        Self::Raw(value)
    }
}
impl TableText {
    fn formatted(message: Message, args: Vec<(&'static str, Self)>) -> Self {
        Self::Formatted(message, args)
    }
    fn render(&self, locale: Locale) -> String {
        match self {
            Self::Message(message) => tr(locale, *message).into(),
            Self::Raw(value) => value.clone(),
            Self::Formatted(message, args) => {
                let values: Vec<_> = args
                    .iter()
                    .map(|(key, value)| (*key, value.render(locale)))
                    .collect();
                let args: Vec<_> = values
                    .iter()
                    .map(|(key, value)| (*key, value.as_str()))
                    .collect();
                // Large raw diagnostics remain intact in a separately labelled raw view.
                dbide_i18n::format(locale, *message, &args).unwrap_or_else(|_| {
                    format!(
                        "{}: {}",
                        tr(locale, Message::ErrorDetails),
                        values
                            .iter()
                            .map(|(_, value)| value.as_str())
                            .collect::<Vec<_>>()
                            .join(" · ")
                    )
                })
            }
        }
    }
}

pub use dbide_ui_spike::table_edit_model::RowContext as TableToken;
#[derive(Clone)]
pub enum TableEditorEvent {
    ReadFallback {
        token: TableToken,
        sql: String,
    },
    Load {
        token: TableToken,
        sql: String,
    },
    Apply {
        token: TableToken,
        guard: dbide_bridge::TableIdentityGuard,
        statements: Vec<dbide_bridge::ParameterizedStatement>,
    },
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Capturing,
    Reading,
    Ready,
    Validating,
    Applying,
    ReloadRequired,
}
struct Field {
    input: Entity<TextareaState>,
    null: bool,
    default: bool,
}
struct QueryField {
    column: Option<usize>,
    operator: FilterOperator,
    input: Entity<InputState>,
    _subscription: Subscription,
}
fn query_text(locale: Locale, ko: &'static str, en: &'static str) -> &'static str {
    if locale == Locale::Ko { ko } else { en }
}
pub struct TableEditor {
    locale: Locale,
    request: ConnectionRequest,
    schema: String,
    name: String,
    token: TableToken,
    relation_oid: u32,
    metadata_fingerprint: String,
    metadata: Option<VerifiedTable>,
    rows: Vec<Vec<Cell>>,
    grid: Entity<TableState<ResultGrid>>,
    fields: Vec<Field>,
    selected: Option<usize>,
    insert_mode: bool,
    staging: Staging,
    phase: Phase,
    capture_id: u64,
    form_revision: u64,
    page: usize,
    query: TableQuery,
    query_fields: Vec<QueryField>,
    query_sort: Option<TableSort>,
    query_page_size: usize,
    query_pending: bool,
    read_controls_enabled: bool,
    error: Option<TableText>,
    status: TableText,
    search: Entity<InputState>,
    search_locale: Locale,
    _subscriptions: Vec<Subscription>,
    _field_subscriptions: Vec<Subscription>,
}
impl EventEmitter<TableEditorEvent> for TableEditor {}
impl TableEditor {
    pub(super) fn result_grid(&self) -> Entity<TableState<ResultGrid>> {
        self.grid.clone()
    }

    pub(super) fn focus_results(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.grid.focus_handle(cx), cx);
    }

    #[allow(dead_code)] // Compatibility constructor; the locale-aware host uses new_with_locale.
    pub fn new(
        request: ConnectionRequest,
        schema: String,
        name: String,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_locale(
            request,
            schema,
            name,
            revision,
            Locale::default(),
            window,
            cx,
        )
    }
    pub fn new_with_locale(
        request: ConnectionRequest,
        schema: String,
        name: String,
        revision: u64,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let grid = cx.new(|cx| {
            TableState::new(
                ResultGrid::new(GridSource::Postgres(Rc::new(ResultPage::default()))),
                window,
                cx,
            )
            .cell_selectable(true)
            .row_header(false)
            .sortable(false)
            .col_movable(false)
        });
        let subscription = cx.subscribe_in(&grid, window, |s, grid, event, w, cx| {
            if let TableEvent::SelectCell(row, col)
            | TableEvent::RightClickedCell(row, col)
            | TableEvent::DoubleClickedCell(row, col) = event
            {
                grid.update(cx, |g, cx| {
                    if matches!(event, TableEvent::RightClickedCell(..)) {
                        g.set_selected_cell(*row, *col, cx);
                    }
                    g.delegate_mut().selected_cell = Some((*row, *col));
                    cx.notify();
                });
                s.select_row(*row, w, cx);
            }
            if matches!(event, TableEvent::DoubleClickedCell(..)) {
                w.dispatch_action(Box::new(InspectValue), cx);
            }
        });
        let mut state = Self {
            locale,
            token: TableToken {
                profile: request.profile.clone(),
                connection_revision: revision,
                row_execution: None,
            },
            request,
            schema,
            name,
            relation_oid: 0,
            metadata_fingerprint: String::new(),
            metadata: None,
            rows: vec![],
            grid,
            fields: vec![],
            selected: None,
            insert_mode: false,
            staging: Staging::default(),
            phase: Phase::Capturing,
            capture_id: 0,
            form_revision: 0,
            page: 0,
            query: TableQuery::default(),
            query_fields: vec![],
            query_sort: None,
            query_page_size: 100,
            query_pending: false,
            read_controls_enabled: true,
            error: None,
            status: Message::CapturingTableMetadata.into(),
            search: cx.new(|cx| {
                InputState::new(window, cx).placeholder(query_text(
                    locale,
                    "현재 페이지 검색 · Enter",
                    "Find in page · Enter",
                ))
            }),
            search_locale: locale,
            _subscriptions: vec![subscription],
            _field_subscriptions: vec![],
        };
        let search_subscription =
            cx.subscribe_in(&state.search, window, |s, _, event, w, cx| match event {
                InputEvent::PressEnter { .. } if !s.busy() && s.read_controls_enabled => {
                    s.find(w, cx)
                }
                InputEvent::Change => cx.notify(),
                _ => {}
            });
        state._subscriptions.push(search_subscription);
        state.capture(false, window, cx);
        state
    }
    pub fn set_read_controls_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.read_controls_enabled = enabled;
        cx.notify();
    }
    pub fn rebind_readonly(
        &mut self,
        request: ConnectionRequest,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.token = TableToken {
            profile: request.profile.clone(),
            connection_revision: revision,
            row_execution: None,
        };
        self.request = request;
        self.read_controls_enabled = true;
        self.capture(false, window, cx);
    }
    /// Call immediately after construction to choose the initial bounded read size.
    pub fn set_page_size_default(&mut self, size: usize, cx: &mut Context<Self>) {
        if self.phase == Phase::Capturing
            && self.metadata.is_none()
            && TABLE_PAGE_SIZES.contains(&size)
        {
            self.query.page_size = size;
            self.query_page_size = size;
            cx.notify();
        }
    }
    pub fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.locale = locale;
        cx.notify();
    }
    pub fn has_staged(&self) -> bool {
        self.staging.count() > 0
    }
    pub fn label(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }
    pub fn has_unsaved(&self, _cx: &App) -> bool {
        self.has_staged()
    }
    pub fn edit_revision(&self) -> (u64, u64, u64) {
        (self.staging.revision, self.form_revision, self.capture_id)
    }
    pub fn token(&self) -> TableToken {
        self.token.clone()
    }
    pub fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.fail_text(message.into(), cx);
    }
    fn fail_text(&mut self, message: TableText, cx: &mut Context<Self>) {
        self.capture_id = self.capture_id.wrapping_add(1);
        self.error = Some(message);
        self.phase = Phase::ReloadRequired;
        cx.notify();
    }
    pub fn is_busy(&self) -> bool {
        self.busy()
    }
    pub fn accept_control(
        &mut self,
        report: dbide_pg_spike::ExecutionReport,
        cx: &mut Context<Self>,
    ) {
        self.phase = Phase::ReloadRequired;
        self.status = TableText::formatted(
            Message::TableControlStatus,
            vec![
                ("outcome", format!("{:?}", report.outcome).into()),
                (
                    "transaction",
                    format!("{:?}", report.transaction_state).into(),
                ),
            ],
        );
        self.error = report.terminal_error.map(|e| e.message.into());
        cx.notify();
    }
    fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Capturing | Phase::Reading | Phase::Validating | Phase::Applying
        )
    }
    fn editable(&self) -> bool {
        self.phase == Phase::Ready
            && self
                .metadata
                .as_ref()
                .is_some_and(|m| m.spec.is_some() && m.read_only.is_none())
            && self.token.row_execution.is_some()
    }
    fn capture(&mut self, apply: bool, window: &mut Window, cx: &mut Context<Self>) {
        if apply {
            self.error = Some(Message::TableApplyDisabledError.into());
            cx.notify();
            return;
        }
        self.phase = if apply {
            Phase::Validating
        } else {
            Phase::Capturing
        };
        self.error = None;
        self.status = if apply {
            Message::RevalidatingTable
        } else {
            Message::CapturingLiveMetadata
        }
        .into();
        let request = self.request.clone();
        let schema = self.schema.clone();
        let name = self.name.clone();
        let revision = self.staging.revision;
        self.capture_id = self.capture_id.wrapping_add(1);
        let capture_id = self.capture_id;
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())?;
                    runtime
                        .block_on(dbide_workflows::capture_table_for_edit(
                            &request,
                            &schema,
                            &name,
                            dbide_workflows::CaptureLimits::default(),
                        ))
                        .map_err(|e| e.to_string())
                })
                .await;
            let _ = view.update_in(cx, |s, w, cx| {
                if s.capture_id != capture_id
                    || s.phase
                        != if apply {
                            Phase::Validating
                        } else {
                            Phase::Capturing
                        }
                {
                    return;
                }
                let result = result.and_then(|capture| {
                    VerifiedTable::capture(
                        &capture.snapshot,
                        &s.schema,
                        &s.name,
                        &s.request.profile,
                    )
                    .map(|metadata| (metadata, capture.relation_oid, capture.metadata_fingerprint))
                });
                match result {
                    Ok((metadata, oid, fingerprint)) if apply => {
                        if s.staging.revision != revision
                            || s.metadata.as_ref().is_none_or(|old| {
                                old.table != metadata.table || old.spec != metadata.spec
                            })
                            || oid != s.relation_oid
                            || fingerprint != s.metadata_fingerprint
                        {
                            s.fail_text(Message::TableMetadataChanged.into(), cx);
                            return;
                        }
                        match s.staging.ready_plan().cloned() {
                            Ok(plan) => {
                                let statements = plan
                                    .statements
                                    .into_iter()
                                    .map(|p| dbide_bridge::ParameterizedStatement {
                                        sql: p.sql,
                                        parameters: p
                                            .parameters
                                            .into_iter()
                                            .map(|p| p.value)
                                            .collect(),
                                        expected_affected_rows: Some(p.expected_affected_rows),
                                    })
                                    .collect();
                                s.phase = Phase::Applying;
                                s.status = Message::ApplyingTableBatch.into();
                                cx.emit(TableEditorEvent::Apply {
                                    token: s.token.clone(),
                                    guard: dbide_bridge::TableIdentityGuard {
                                        schema: s.schema.clone(),
                                        table: s.name.clone(),
                                        oid: s.relation_oid,
                                        metadata_fingerprint: Some(s.metadata_fingerprint.clone()),
                                    },
                                    statements,
                                });
                            }
                            Err(e) => s.fail(e, cx),
                        }
                    }
                    Ok((metadata, oid, fingerprint)) => {
                        s.relation_oid = oid;
                        s.metadata_fingerprint = fingerprint;
                        s._field_subscriptions.clear();
                        s.fields = metadata
                            .table
                            .columns
                            .iter()
                            .map(|column| Field {
                                input: cx.new(|cx| TextareaState::new(w, cx)),
                                null: column.nullable,
                                default: column.default.is_some() || column.generated.is_some(),
                            })
                            .collect();
                        for field in &s.fields {
                            s._field_subscriptions.push(cx.subscribe(
                                &field.input,
                                |s, _, event, cx| {
                                    if matches!(event, InputEvent::Change) {
                                        s.form_revision = s.form_revision.wrapping_add(1);
                                        s.staging.preview = None;
                                        cx.notify();
                                    }
                                },
                            ));
                        }
                        // Filter indices belong to this exact capture. Never silently rebind
                        // them when a column is dropped, reordered or changes type.
                        if s.metadata
                            .as_ref()
                            .is_some_and(|old| old.table.columns != metadata.table.columns)
                        {
                            s.query = TableQuery {
                                page_size: s.query.page_size,
                                ..TableQuery::default()
                            };
                            s.query_fields.clear();
                            s.query_sort = None;
                            s.query_page_size = s.query.page_size;
                            s.query_pending = false;
                            s.page = 0;
                        }
                        let sql = match metadata.select_sql_with_query(s.page, &s.query) {
                            Ok(sql) => sql,
                            Err(error) => {
                                s.fail(error, cx);
                                return;
                            }
                        };
                        s.metadata = Some(metadata);
                        s.phase = Phase::Reading;
                        s.status = Message::ReadingTablePage.into();
                        cx.emit(TableEditorEvent::Load {
                            token: s.token.clone(),
                            sql,
                        });
                    }
                    Err(e) => {
                        if apply {
                            s.fail(e, cx);
                        } else {
                            s.status = TableText::formatted(
                                Message::TableCatalogFallback,
                                vec![("error", e.into())],
                            );
                            cx.emit(TableEditorEvent::ReadFallback {
                                token: s.token.clone(),
                                sql: dbide_ui_spike::workbench_model::table_query(
                                    DatabaseEngine::PostgreSql,
                                    &s.schema,
                                    &s.name,
                                ),
                            });
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub fn accept_report(
        &mut self,
        report: dbide_pg_spike::ExecutionReport,
        revision: u64,
        apply: bool,
        cx: &mut Context<Self>,
    ) {
        let key = report.key;
        let (pages, summary) = split_report(report);
        if apply {
            self.phase = Phase::ReloadRequired;
            self.status = TableText::formatted(
                Message::TableApplyStatus,
                vec![
                    ("outcome", format!("{:?}", summary.outcome).into()),
                    (
                        "transaction",
                        format!("{:?}", summary.transaction_state).into(),
                    ),
                    (
                        "acknowledgement",
                        if summary.outcome == dbide_pg_spike::Outcome::Completed {
                            if summary.transaction_state == ServerTransactionState::Idle {
                                Message::CommitAcknowledged
                            } else {
                                Message::SubmittedNotCommitted
                            }
                        } else {
                            Message::NoBatchAcknowledgement
                        }
                        .into(),
                    ),
                ],
            );
            self.error = summary.terminal_error.clone().map(Into::into);
            if summary.outcome == dbide_pg_spike::Outcome::Completed {
                self.staging = Staging::default();
                self.selected = None;
                self.insert_mode = false;
            }
            cx.notify();
            return;
        }
        if summary.outcome != dbide_pg_spike::Outcome::Completed || summary.connection_retired {
            self.fail_text(
                summary.terminal_error.map(Into::into).unwrap_or_else(|| {
                    TableText::formatted(
                        Message::TableReadReloadRequired,
                        vec![("outcome", format!("{:?}", summary.outcome).into())],
                    )
                }),
                cx,
            );
            return;
        }
        let Some(metadata) = &mut self.metadata else {
            return;
        };
        let expected = metadata
            .table
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>();
        let Some(page) = pages.into_iter().find(|p| p.columns == expected) else {
            self.fail_text(Message::UnexpectedTableProjection.into(), cx);
            return;
        };
        match metadata.decode_rows(&page.columns, &page.rows, summary.truncated) {
            Ok(rows) => self.rows = rows,
            Err(error) => {
                metadata.read_only = Some(error.clone());
                self.error = Some(error.into());
                self.rows = vec![];
            }
        }
        let display = if self.rows.len() == page.rows.len() {
            ResultPage {
                columns: page.columns.clone(),
                rows: self
                    .rows
                    .iter()
                    .map(|r| {
                        r.iter()
                            .map(|c| {
                                if *c == Cell::Null {
                                    None
                                } else {
                                    Some(cell_text(c))
                                }
                            })
                            .collect()
                    })
                    .collect(),
                affected_rows: page.affected_rows,
                command_only: false,
            }
        } else {
            page
        };
        self.grid.update(cx, |grid, cx| {
            *grid.delegate_mut() = ResultGrid::new(GridSource::Postgres(Rc::new(display)));
            grid.clear_selection(cx);
            grid.refresh(cx);
            cx.notify();
        });
        self.token.connection_revision = revision;
        self.token.row_execution = Some(key);
        self.phase = Phase::Ready;
        self.selected = None;
        self.insert_mode = false;
        self.status = TableText::formatted(
            Message::LoadedTablePage,
            vec![
                ("page", (self.page + 1).to_string().into()),
                (
                    "rows",
                    self.grid.read(cx).delegate().row_count().to_string().into(),
                ),
            ],
        );
        cx.notify();
    }
    fn form_values(&self, cx: &App) -> Result<Vec<Cell>, String> {
        let metadata = self
            .metadata
            .as_ref()
            .ok_or(tr(self.locale, Message::MetadataUnavailable))?;
        self.fields
            .iter()
            .enumerate()
            .map(|(ix, f)| metadata.cell_from_input(ix, &f.input.read(cx).value(), f.null))
            .collect()
    }
    fn form_dirty(&self, cx: &App) -> bool {
        if self.insert_mode {
            return self
                .fields
                .iter()
                .zip(self.metadata.iter().flat_map(|m| &m.table.columns))
                .any(|(f, c)| {
                    f.default != (c.default.is_some() || c.generated.is_some())
                        || f.null != (c.nullable && !f.default)
                        || (!f.default && !f.null && !f.input.read(cx).value().is_empty())
                });
        }
        let Some(row) = self.selected else {
            return false;
        };
        let baseline = match self.staging.rows.get(&row) {
            Some(Edit::Update { replacement, .. }) => replacement,
            Some(Edit::Delete { .. }) => return false,
            _ => match self.rows.get(row) {
                Some(row) => row,
                None => return false,
            },
        };
        match self.form_values(cx) {
            Ok(values) => values != *baseline,
            Err(_) => true,
        }
    }
    fn select_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected != Some(row) && self.form_dirty(cx) {
            self.confirm_form_switch(Some(row), window, cx);
        } else {
            self.select_row_force(row, window, cx);
        }
    }
    fn begin_insert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.form_dirty(cx) {
            self.confirm_form_switch(None, window, cx);
        } else {
            self.begin_insert_force(window, cx);
        }
    }
    fn confirm_form_switch(
        &mut self,
        row: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_prompt() {
            return;
        }
        let receiver = window.prompt(
            PromptLevel::Warning,
            tr(self.locale, Message::DiscardRowFormTitle),
            Some(tr(self.locale, Message::DiscardRowFormHint)),
            &[
                tr(self.locale, Message::KeepForm),
                tr(self.locale, Message::DiscardFormChanges),
            ],
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            if receiver.await == Ok(1) {
                let _ = view.update_in(cx, |s, w, cx| {
                    if let Some(row) = row {
                        s.select_row_force(row, w, cx)
                    } else {
                        s.begin_insert_force(w, cx)
                    }
                });
            }
        })
        .detach();
    }
    fn select_row_force(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() || self.rows.get(row).is_none() {
            return;
        }
        self.selected = Some(row);
        self.insert_mode = false;
        let values = if let Some(Edit::Update { replacement, .. }) = self.staging.rows.get(&row) {
            replacement.clone()
        } else {
            self.rows[row].clone()
        };
        for (field, value) in self.fields.iter_mut().zip(&values) {
            field.null = *value == Cell::Null;
            field.default = false;
            field.input.update(cx, |input, cx| {
                input.set_value(cell_text(value), window, cx)
            });
        }
        cx.notify();
    }
    fn begin_insert_force(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        self.selected = None;
        self.insert_mode = true;
        if let Some(metadata) = &self.metadata {
            for (field, column) in self.fields.iter_mut().zip(&metadata.table.columns) {
                field.default = column.default.is_some() || column.generated.is_some();
                field.null = column.nullable && !field.default;
                field
                    .input
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }
        }
        self.status = Message::NewRowFormStatus.into();
        cx.notify();
    }
    fn stage(&mut self, delete: bool, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        let result = (|| -> Result<(), String> {
            let metadata = self
                .metadata
                .as_ref()
                .ok_or(tr(self.locale, Message::MetadataUnavailable))?;
            let spec = metadata
                .spec
                .as_ref()
                .ok_or(tr(self.locale, Message::TableReadOnly))?;
            if self.insert_mode {
                let cells = self.form_values(cx)?;
                let values = cells
                    .into_iter()
                    .enumerate()
                    .map(|(ix, c)| {
                        if self.fields[ix].default {
                            InsertValue::Default
                        } else {
                            InsertValue::Value(c)
                        }
                    })
                    .collect();
                self.staging.stage_insert(values, spec)?;
            } else {
                let row = self
                    .selected
                    .ok_or(tr(self.locale, Message::SelectRowFirst))?;
                let original = self
                    .rows
                    .get(row)
                    .ok_or(tr(self.locale, Message::OriginalRowUnavailable))?
                    .clone();
                let edit = if delete {
                    Edit::Delete { original }
                } else {
                    let replacement = self.form_values(cx)?;
                    for (ix, col) in metadata.table.columns.iter().enumerate() {
                        if col.generated.is_some() && original[ix] != replacement[ix] {
                            return Err(TableText::formatted(
                                Message::GeneratedColumnReadOnly,
                                vec![("column", col.name.clone().into())],
                            )
                            .render(self.locale));
                        }
                    }
                    Edit::Update {
                        original,
                        replacement,
                    }
                };
                self.staging.stage_row(row, edit, spec)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.error = None;
                if self.insert_mode {
                    self.insert_mode = false;
                    self.selected = None;
                }
                self.status = TableText::formatted(
                    Message::StagedRowEditsStatus,
                    vec![("count", self.staging.count().to_string().into())],
                );
            }
            Err(e) => self.error = Some(e.into()),
        }
        cx.notify();
    }
    fn preview(&mut self, cx: &mut Context<Self>) {
        if self.form_dirty(cx) {
            self.error = Some(Message::StageFormBeforePreview.into());
            cx.notify();
            return;
        }
        if !self.editable() {
            return;
        }
        let result = self
            .metadata
            .as_ref()
            .and_then(|m| m.spec.as_ref())
            .ok_or(tr(self.locale, Message::TableReadOnly).into())
            .and_then(|spec| self.staging.plan(spec));
        match result {
            Ok(()) => {
                self.error = None;
                self.status = Message::TransactionPreviewReady.into();
            }
            Err(e) => self.error = Some(e.into()),
        }
        cx.notify();
    }
    fn add_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.read_controls_enabled
            || self.busy()
            || self.query_fields.len() >= dbide_data::MAX_TABLE_FILTERS
        {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(query_text(self.locale, "값", "Value"))
        });
        let subscription = cx.subscribe(&input, |s, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                s.query_pending = true;
                cx.notify();
            }
        });
        self.query_fields.push(QueryField {
            column: Some(0),
            operator: FilterOperator::Equal,
            input,
            _subscription: subscription,
        });
        self.query_pending = true;
        cx.notify();
    }
    fn apply_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.read_controls_enabled || self.busy() {
            return;
        }
        let Some(metadata) = self.metadata.as_ref() else {
            return;
        };
        let filters = self
            .query_fields
            .iter()
            .filter_map(|field| field.column.map(|column| (field, column)))
            .map(|(field, column)| {
                Ok(TableFilter {
                    column,
                    operator: field.operator,
                    value: if field.operator.needs_value() {
                        metadata.cell_from_input(column, &field.input.read(cx).value(), false)?
                    } else {
                        Cell::Null
                    },
                })
            })
            .collect::<Result<Vec<_>, String>>();
        let candidate = filters.map(|filters| TableQuery {
            filters,
            sort: self.query_sort,
            page_size: self.query_page_size,
        });
        match candidate.and_then(|query| metadata.select_sql_with_query(0, &query).map(|_| query)) {
            Ok(query) => {
                self.query = query;
                self.query_pending = false;
                self.reload(0, window, cx);
            }
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
            }
        }
    }
    fn reload(&mut self, page: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.read_controls_enabled || self.busy() || window.has_active_prompt() {
            return;
        }
        if self.has_unsaved(cx) {
            let expected = self.edit_revision();
            let receiver = window.prompt(
                PromptLevel::Warning,
                tr(self.locale, Message::DiscardReloadTitle),
                Some(tr(self.locale, Message::DiscardReloadHint)),
                &[
                    tr(self.locale, Message::KeepEdits),
                    tr(self.locale, Message::DiscardAndReload),
                ],
                cx,
            );
            cx.spawn_in(window, async move |view, cx| {
                if receiver.await == Ok(1) {
                    let _ = view.update_in(cx, |s, w, cx| {
                        if s.busy() || s.edit_revision() != expected {
                            return;
                        }
                        s.staging = Staging::default();
                        s.page = page;
                        s.capture(false, w, cx);
                    });
                }
            })
            .detach();
        } else {
            self.page = page;
            self.capture(false, window, cx);
        }
    }
    fn discard_edits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.staging = Staging::default();
        if let Some(row) = self.selected {
            self.select_row_force(row, window, cx);
        } else {
            self.begin_insert_force(window, cx);
            self.insert_mode = false;
        }
        if self.phase == Phase::Ready {
            self.error = None;
        }
        self.status = Message::LocalBuffersDiscarded.into();
        cx.notify();
    }
    fn find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().to_lowercase();
        if query.is_empty() {
            return;
        }
        let start = self.selected.map_or(0, |r| r + 1);
        let count = self.rows.len();
        let found = (0..count)
            .map(|offset| (start + offset) % count)
            .find(|&row| {
                self.rows[row]
                    .iter()
                    .any(|c| cell_text(c).to_lowercase().contains(&query))
            });
        if let Some(row) = found {
            self.select_row(row, window, cx);
            self.grid.update(cx, |g, cx| {
                g.delegate_mut().selected_cell = Some((row, 0));
                g.set_selected_cell(row, 0, cx);
                g.scroll_to_row(row, cx);
            });
        } else {
            self.status = Message::NoMatchLoadedPage.into();
        }
        cx.notify();
    }
    fn query_toolbar(&self, cx: &mut Context<Self>) -> Div {
        let locale = self.locale;
        let busy = self.busy() || !self.read_controls_enabled;
        let columns = self
            .metadata
            .as_ref()
            .map(|m| {
                m.table
                    .columns
                    .iter()
                    .map(|c| (c.name.clone(), c.data_type.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let sort_label = self
            .query_sort
            .and_then(|sort| {
                columns.get(sort.column).map(|c| {
                    format!(
                        "{} {}",
                        c.0,
                        if sort.direction == SortDirection::Ascending {
                            "↑"
                        } else {
                            "↓"
                        }
                    )
                })
            })
            .unwrap_or_else(|| query_text(locale, "기본 정렬", "Default order").into());
        let weak = cx.weak_entity();
        let sort_columns = columns.clone();
        let sort = Button::new("table-sort-column")
            .small()
            .label(format!("{sort_label} ▾"))
            .disabled(busy || columns.is_empty())
            .dropdown_menu(move |mut menu, _, _| {
                let reset = weak.clone();
                menu = menu.item(
                    PopupMenuItem::new(query_text(locale, "기본 정렬", "Default order")).on_click(
                        move |_, _, cx| {
                            let _ = reset.update(cx, |s, cx| {
                                s.query_sort = None;
                                s.query_pending = true;
                                cx.notify();
                            });
                        },
                    ),
                );
                for (index, (name, _)) in sort_columns.iter().enumerate() {
                    for direction in [SortDirection::Ascending, SortDirection::Descending] {
                        let weak = weak.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!(
                                "{} {}",
                                name,
                                if direction == SortDirection::Ascending {
                                    "↑ ASC"
                                } else {
                                    "↓ DESC"
                                }
                            ))
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |s, cx| {
                                    s.query_sort = Some(TableSort {
                                        column: index,
                                        direction,
                                    });
                                    s.query_pending = true;
                                    cx.notify();
                                });
                            }),
                        );
                    }
                }
                menu
            });
        let weak = cx.weak_entity();
        let page_size = Button::new("table-page-size")
            .small()
            .label(format!(
                "{} {} ▾",
                self.query_page_size,
                query_text(locale, "행", "rows")
            ))
            .disabled(busy)
            .dropdown_menu(move |mut menu, _, _| {
                for size in TABLE_PAGE_SIZES {
                    let weak = weak.clone();
                    menu = menu.item(PopupMenuItem::new(size.to_string()).on_click(
                        move |_, _, cx| {
                            let _ = weak.update(cx, |s, cx| {
                                s.query_page_size = size;
                                s.query_pending = true;
                                cx.notify();
                            });
                        },
                    ));
                }
                menu
            });
        let custom_query = !self.query_fields.is_empty()
            || self.query_sort.is_some()
            || !self.query.filters.is_empty()
            || self.query.sort.is_some();
        let mut toolbar = div()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Self::button("table-add-filter", query_text(locale, "+ 필터", "+ Filter"))
                            .disabled(
                                busy || columns.is_empty()
                                    || self.query_fields.len() >= dbide_data::MAX_TABLE_FILTERS,
                            )
                            .on_click(cx.listener(|s, _, w, cx| s.add_filter(w, cx))),
                    )
                    .child(sort)
                    .child(page_size)
                    .child(div().flex_1())
                    .when(self.query_pending, |el| {
                        el.child(
                            Self::button(
                                "table-apply-query",
                                query_text(locale, "조회 적용", "Apply query"),
                            )
                            .primary()
                            .tooltip(query_text(
                                locale,
                                "변경한 조회 조건을 첫 페이지부터 적용",
                                "Apply pending query from page 1",
                            ))
                            .disabled(busy || self.metadata.is_none())
                            .on_click(cx.listener(|s, _, w, cx| s.apply_query(w, cx))),
                        )
                    })
                    .when(custom_query, |el| {
                        el.child(
                            Self::button(
                                "table-clear-query",
                                query_text(locale, "초기화", "Reset"),
                            )
                            .ghost()
                            .disabled(busy || self.metadata.is_none())
                            .on_click(cx.listener(|s, _, w, cx| {
                                s.query_fields.clear();
                                s.query_sort = None;
                                s.query_pending = true;
                                s.apply_query(w, cx);
                            })),
                        )
                    }),
            );
        for (index, field) in self.query_fields.iter().enumerate() {
            let label = field
                .column
                .and_then(|ix| columns.get(ix))
                .map(|c| format!("{} · {}", c.0, c.1))
                .unwrap_or_else(|| query_text(locale, "열 선택", "Choose column").into());
            let choices = columns.clone();
            let weak = cx.weak_entity();
            let column = Button::new(("table-filter-column", index))
                .small()
                .label(format!("{label} ▾"))
                .disabled(busy)
                .dropdown_menu(move |mut menu, _, _| {
                    for (column, (name, ty)) in choices.iter().enumerate() {
                        let weak = weak.clone();
                        menu = menu.item(PopupMenuItem::new(format!("{name} · {ty}")).on_click(
                            move |_, _, cx| {
                                let _ = weak.update(cx, |s, cx| {
                                    if let Some(field) = s.query_fields.get_mut(index) {
                                        field.column = Some(column);
                                        s.query_pending = true;
                                        cx.notify();
                                    }
                                });
                            },
                        ));
                    }
                    menu
                });
            let weak = cx.weak_entity();
            let operator = Button::new(("table-filter-operator", index))
                .small()
                .label(format!("{} ▾", field.operator.label()))
                .disabled(busy)
                .dropdown_menu(move |mut menu, _, _| {
                    for operator in FilterOperator::ALL {
                        let weak = weak.clone();
                        menu = menu.item(PopupMenuItem::new(operator.label()).on_click(
                            move |_, _, cx| {
                                let _ = weak.update(cx, |s, cx| {
                                    if let Some(field) = s.query_fields.get_mut(index) {
                                        field.operator = operator;
                                        s.query_pending = true;
                                        cx.notify();
                                    }
                                });
                            },
                        ));
                    }
                    menu
                });
            toolbar = toolbar.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(26.))
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child(if index == 0 { "WHERE" } else { "AND" }),
                    )
                    .child(column)
                    .child(operator)
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&field.input)
                                .small()
                                .aria_label(format!(
                                    "{} {}",
                                    query_text(locale, "서버 필터 값", "Server filter value"),
                                    index + 1
                                ))
                                .disabled(busy || !field.operator.needs_value()),
                        ),
                    )
                    .child(
                        Button::new(("remove-table-filter", index))
                            .small()
                            .label("×")
                            .disabled(busy)
                            .accessibility_label(query_text(locale, "필터 제거", "Remove filter"))
                            .tooltip(query_text(locale, "필터 제거", "Remove filter"))
                            .on_click(cx.listener(move |s, _, _, cx| {
                                if index < s.query_fields.len() {
                                    s.query_fields.remove(index);
                                    s.query_pending = true;
                                    cx.notify();
                                }
                            })),
                    ),
            );
        }
        if self.metadata.as_ref().is_some_and(|m| {
            m.table.primary_key.is_none()
                && m.spec.as_ref().is_none_or(|spec| spec.identity.is_none())
        }) {
            toolbar = toolbar.child(div().text_size(px(10.)).text_color(rgb(MUTED)).child(
                query_text(
                    locale,
                    "기본 키 없음 · 동일 정렬값의 페이지 순서는 보장되지 않음",
                    "No primary key · tied rows may change page order",
                ),
            ));
        }
        toolbar
    }
    fn button(id: &'static str, label: &'static str) -> Button {
        Button::new(id).label(label).small()
    }
}
impl Render for TableEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.locale;
        if self.grid.read(cx).delegate().locale != locale {
            self.grid.update(cx, |grid, cx| {
                grid.delegate_mut().locale = locale;
                cx.notify();
            });
        }
        if self.search_locale != locale {
            self.search.update(cx, |input, cx| {
                input.set_placeholder(
                    query_text(locale, "현재 페이지 검색 · Enter", "Find in page · Enter"),
                    window,
                    cx,
                )
            });
            for field in &self.query_fields {
                field.input.update(cx, |input, cx| {
                    input.set_placeholder(query_text(locale, "값", "Value"), window, cx)
                });
            }
            self.search_locale = locale;
        }
        let status = self.status.render(locale);
        let query_toolbar = self.query_toolbar(cx);
        let edit_controls = false; // M1 viewer: row writes await the M2 execution validation gate.
        let editable = edit_controls && self.editable();
        let busy = self.busy() || !self.read_controls_enabled;
        let mut form = div()
            .id("table-row-form")
            .w(px(360.))
            .flex_shrink_0()
            .border_l_1()
            .border_color(rgb(BORDER))
            .bg(rgb(PANEL))
            .flex()
            .flex_col()
            .child(
                div()
                    .p_4()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if self.insert_mode {
                        tr(locale, Message::InsertNewRow).into()
                    } else {
                        self.selected
                            .map(|r| {
                                TableText::formatted(
                                    Message::EditRowOnPage,
                                    vec![("row", (r + 1).to_string().into())],
                                )
                                .render(locale)
                            })
                            .unwrap_or_else(|| tr(locale, Message::SelectRowToEdit).into())
                    }),
            );
        let mut inputs = div()
            .id("row-fields")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .flex()
            .flex_col()
            .gap_3();
        if let Some(metadata) = &self.metadata {
            if let Some(reason) = &metadata.read_only {
                inputs = inputs.child(
                    div().text_color(rgb(0xf0b797)).child(
                        TableText::formatted(
                            Message::ReadOnlyReason,
                            vec![("reason", reason.clone().into())],
                        )
                        .render(locale),
                    ),
                );
            }
            inputs = inputs.children(metadata.table.columns.iter().enumerate().map(
                |(ix, column)| {
                    let f = &self.fields[ix];
                    let generated = column.generated.is_some();
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(MUTED))
                                .child(format!(
                                    "{} · {} · {}{}",
                                    column.name,
                                    column.data_type,
                                    if column.nullable {
                                        tr(locale, Message::Nullable)
                                    } else {
                                        "NOT NULL"
                                    },
                                    if generated {
                                        format!(" · {}", tr(locale, Message::GeneratedIdentity))
                                    } else {
                                        String::new()
                                    }
                                )),
                        )
                        .when_some(column.default.as_ref(), |el, value| {
                            el.child(
                                div().text_size(px(10.)).text_color(rgb(MUTED)).child(
                                    TableText::formatted(
                                        Message::DefaultExpression,
                                        vec![("value", value.clone().into())],
                                    )
                                    .render(locale),
                                ),
                            )
                        })
                        .when_some(column.generated.as_ref(), |el, value| {
                            el.child(
                                div().text_size(px(10.)).text_color(rgb(MUTED)).child(
                                    TableText::formatted(
                                        Message::GeneratedExpression,
                                        vec![("value", value.clone().into())],
                                    )
                                    .render(locale),
                                ),
                            )
                        })
                        .child(
                            Textarea::new(&f.input)
                                .aria_label(
                                    TableText::formatted(
                                        Message::RowValueFor,
                                        vec![("column", column.name.clone().into())],
                                    )
                                    .render(locale),
                                )
                                .h(px(72.))
                                .disabled(
                                    !editable
                                        || (!self.insert_mode && self.selected.is_none())
                                        || generated
                                        || f.null
                                        || f.default,
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_3()
                                .child(
                                    Checkbox::new(("null", ix))
                                        .label(format!("NULL · {}", column.name))
                                        .checked(f.null)
                                        .disabled(
                                            !editable || !column.nullable || generated || f.default,
                                        )
                                        .on_click(cx.listener(move |s, value, _, cx| {
                                            s.fields[ix].null = *value;
                                            s.form_revision = s.form_revision.wrapping_add(1);
                                            s.staging.preview = None;
                                            cx.notify();
                                        })),
                                )
                                .when(self.insert_mode, |el| {
                                    el.child(
                                        Checkbox::new(("default", ix))
                                            .label(format!("DEFAULT · {}", column.name))
                                            .checked(f.default)
                                            .disabled(
                                                !editable
                                                    || generated
                                                    || (column.default.is_none()
                                                        && !column.nullable),
                                            )
                                            .on_click(cx.listener(move |s, value, _, cx| {
                                                s.fields[ix].default = *value;
                                                s.form_revision = s.form_revision.wrapping_add(1);
                                                s.staging.preview = None;
                                                cx.notify();
                                            })),
                                    )
                                }),
                        )
                },
            ));
        }
        form = form.child(inputs).child(
            div()
                .p_4()
                .flex()
                .gap_2()
                .border_t_1()
                .border_color(rgb(BORDER))
                .child(
                    Self::button(
                        "stage-update",
                        if self.insert_mode {
                            tr(locale, Message::StageInsert)
                        } else {
                            tr(locale, Message::StageUpdate)
                        },
                    )
                    .primary()
                    .disabled(!editable || (!self.insert_mode && self.selected.is_none()))
                    .on_click(cx.listener(|s, _, _, cx| s.stage(false, cx))),
                )
                .child(
                    Self::button("stage-delete", tr(locale, Message::StageDelete))
                        .disabled(!editable || self.insert_mode || self.selected.is_none())
                        .on_click(cx.listener(|s, _, _, cx| s.stage(true, cx))),
                ),
        );
        let mut root =
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(rgb(BG))
                .text_color(rgb(TEXT))
                .text_size(px(12.))
                .child(
                    div()
                        .h(px(44.))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(MUTED))
                                .child(query_text(locale, "읽기 전용", "READ ONLY")),
                        )
                        .child(
                            Button::new("reload-table")
                                .ghost()
                                .small()
                                .icon(IconName::RefreshCw)
                                .accessibility_label(tr(locale, Message::Reload))
                                .tooltip(tr(locale, Message::Reload))
                                .disabled(busy)
                                .on_click(cx.listener(|s, _, w, cx| s.reload(0, w, cx))),
                        )
                        .child(
                            Button::new("previous-page")
                                .ghost()
                                .small()
                                .icon(IconName::ChevronLeft)
                                .accessibility_label(tr(locale, Message::PreviousPage))
                                .tooltip(tr(locale, Message::PreviousPage))
                                .disabled(busy || self.page == 0)
                                .on_click(cx.listener(|s, _, w, cx| {
                                    s.reload(s.page.saturating_sub(1), w, cx)
                                })),
                        )
                        .child(
                            Button::new("next-page")
                                .ghost()
                                .small()
                                .icon(IconName::ChevronRight)
                                .accessibility_label(tr(locale, Message::NextPage))
                                .tooltip(tr(locale, Message::NextPage))
                                .disabled(
                                    busy || self.grid.read(cx).delegate().row_count()
                                        < self.query.page_size,
                                )
                                .on_click(cx.listener(|s, _, w, cx| {
                                    s.reload(s.page.saturating_add(1), w, cx)
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(MUTED))
                                .child(format!(
                                    "{} {}",
                                    query_text(locale, "페이지", "Page"),
                                    self.page + 1
                                )),
                        )
                        .when(edit_controls, |el| {
                            el.child(
                                Self::button("insert-table-row", tr(locale, Message::NewRow))
                                    .disabled(!editable)
                                    .on_click(cx.listener(|s, _, w, cx| s.begin_insert(w, cx))),
                            )
                        })
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&self.search)
                                    .aria_label(tr(locale, Message::FindLoadedPage))
                                    .small(),
                            ),
                        )
                        .when(!self.search.read(cx).value().is_empty(), |el| {
                            el.child(
                                Self::button(
                                    "clear-page-search",
                                    query_text(locale, "검색 지우기", "Clear search"),
                                )
                                .ghost()
                                .on_click(cx.listener(
                                    |s, _, w, cx| {
                                        s.search.update(cx, |input, cx| input.set_value("", w, cx));
                                        cx.notify();
                                    },
                                )),
                            )
                        }),
                )
                .child(query_toolbar)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(
                            div().flex_1().min_w_0().key_context("ResultGrid").child(
                                DataTable::new(&self.grid)
                                    .small()
                                    .stripe(false)
                                    .bordered(false),
                            ),
                        )
                        .when(edit_controls, |el| el.child(form)),
                )
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .border_t_1()
                        .border_color(rgb(BORDER))
                        .text_color(rgb(MUTED))
                        .id("table-page-status")
                        .role(Role::Status)
                        .aria_label(status.clone())
                        .child(status),
                );
        if let Some(error) = &self.error {
            let error = error.render(locale);
            root = root.child(
                div()
                    .id("table-edit-error")
                    .role(Role::Alert)
                    .aria_label(error.clone())
                    .px_4()
                    .py_2()
                    .bg(rgb(0x302522))
                    .text_color(rgb(0xf0b797))
                    .child(error.clone()),
            );
        }
        let preview = self.staging.ready_plan().ok();
        root = root.when(edit_controls, |el| {
            el.child(
                div()
                    .px_4()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div().child(
                            TableText::formatted(
                                Message::StagedEdits,
                                vec![("count", self.staging.count().to_string().into())],
                            )
                            .render(locale),
                        ),
                    )
                    .child(
                        Self::button(
                            "preview-table-plan",
                            tr(locale, Message::PreviewTransaction),
                        )
                        .disabled(!editable || !self.has_staged())
                        .on_click(cx.listener(|s, _, _, cx| s.preview(cx))),
                    )
                    .child(
                        Self::button("discard-staged", tr(locale, Message::DiscardStaged))
                            .disabled(busy || !self.has_unsaved(cx))
                            .on_click(cx.listener(|s, _, w, cx| s.discard_edits(w, cx))),
                    )
                    .child(
                        Self::button("apply-table-plan", tr(locale, Message::ApplyPreview))
                            .primary()
                            .disabled(true)
                            .on_click(cx.listener(|s, _, w, cx| s.capture(true, w, cx))),
                    ),
            )
        });
        root = root.child(
            div()
                .px_4()
                .py_1()
                .text_size(px(10.))
                .text_color(rgb(MUTED))
                .child(query_text(
                    locale,
                    "페이지마다 새로 조회 · 검색은 현재 페이지에만 적용",
                    "Pages refresh independently · search covers this page",
                )),
        );
        if edit_controls && let Some(plan) = preview {
            root = root.child(
                div()
                    .child(tr(locale, Message::TableApplyDisabled))
                    .id("table-plan-preview")
                    .max_h(px(180.))
                    .overflow_y_scroll()
                    .px_4()
                    .py_2()
                    .bg(rgb(PANEL))
                    .text_size(px(10.))
                    .font_family("Menlo")
                    .child(tr(locale, Message::GuardedTransactionHint))
                    .children(plan.statements.iter().enumerate().map(|(ix, statement)| {
                        div().id(("planned-edit", ix)).py_2().child(format!(
                            "{}: {}\n{}: {:?}",
                            ix + 1,
                            statement.sql,
                            tr(locale, Message::BoundParameters),
                            statement
                                .parameters
                                .iter()
                                .map(|p| &p.value)
                                .collect::<Vec<_>>()
                        ))
                    })),
            );
        }
        root
    }
}
