use super::*;
use dbide_data::{Cell, Edit, InsertValue};
use dbide_ui_spike::table_edit_model::{PAGE_ROWS, Staging, VerifiedTable, cell_text};
use gpui_component::input::{Textarea, TextareaState};

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
pub struct TableEditor {
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
    error: Option<String>,
    status: String,
    search: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    _field_subscriptions: Vec<Subscription>,
}
impl EventEmitter<TableEditorEvent> for TableEditor {}
impl TableEditor {
    pub fn new(
        request: ConnectionRequest,
        schema: String,
        name: String,
        revision: u64,
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
            if let TableEvent::SelectCell(row, col) = event {
                grid.update(cx, |g, cx| {
                    g.delegate_mut().selected_cell = Some((*row, *col));
                    cx.notify();
                });
                s.select_row(*row, w, cx);
            }
        });
        let mut state = Self {
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
            error: None,
            status: "Capturing verified table metadata…".into(),
            search: cx
                .new(|cx| InputState::new(window, cx).placeholder("Find in this loaded page")),
            _subscriptions: vec![subscription],
            _field_subscriptions: vec![],
        };
        state.capture(false, window, cx);
        state
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
        self.status = format!(
            "Transaction control {:?} · current wire {:?}. Reload rows; staged edits will not be replayed.",
            report.outcome, report.transaction_state
        );
        self.error = report.terminal_error.map(|e| e.message);
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
            self.error =
                Some("Row editing validation pending. Apply is disabled in this release.".into());
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
            "Revalidating table identity and catalog before Apply…"
        } else {
            "Capturing complete live metadata…"
        }
        .into();
        let request = self.request.clone();
        let schema = self.schema.clone();
        let name = self.name.clone();
        let revision = self.staging.revision;
        self.capture_id = self.capture_id.wrapping_add(1);
        let capture_id = self.capture_id;
        cx.spawn_in(window,async move|view,cx|{
            let result=cx.background_executor().spawn(async move{
                let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e|e.to_string())?;
                runtime.block_on(dbide_workflows::capture_table_for_edit(&request,&schema,&name,dbide_workflows::CaptureLimits::default())).map_err(|e|e.to_string())
            }).await;
            let _=view.update_in(cx,|s,w,cx|{
                if s.capture_id!=capture_id||s.phase!=if apply{Phase::Validating}else{Phase::Capturing}{return;}
                let result=result.and_then(|capture|VerifiedTable::capture(&capture.snapshot,&s.schema,&s.name,&s.request.profile).map(|metadata|(metadata,capture.relation_oid,capture.metadata_fingerprint)));
                match result {
                    Ok((metadata,oid,fingerprint)) if apply=>{
                        if s.staging.revision!=revision||s.metadata.as_ref().is_none_or(|old|old.table!=metadata.table||old.spec!=metadata.spec)||oid!=s.relation_oid||fingerprint!=s.metadata_fingerprint {s.fail("Table metadata/identity changed. Reload and stage edits again; nothing was applied.".into(),cx);return;}
                        match s.staging.ready_plan().cloned(){Ok(plan)=>{
                            let statements=plan.statements.into_iter().map(|p|dbide_bridge::ParameterizedStatement{sql:p.sql,parameters:p.parameters.into_iter().map(|p|p.value).collect(),expected_affected_rows:Some(p.expected_affected_rows)}).collect();
                            s.phase=Phase::Applying;s.status="Applying explicit parameterized batch…".into();
                            cx.emit(TableEditorEvent::Apply{token:s.token.clone(),guard:dbide_bridge::TableIdentityGuard{schema:s.schema.clone(),table:s.name.clone(),oid:s.relation_oid,metadata_fingerprint:Some(s.metadata_fingerprint.clone())},statements});
                        },Err(e)=>s.fail(e,cx)}
                    },
                    Ok((metadata,oid,fingerprint))=>{
                        s.relation_oid=oid;s.metadata_fingerprint=fingerprint;s._field_subscriptions.clear();s.fields=metadata.table.columns.iter().map(|column|Field{input:cx.new(|cx|TextareaState::new(w,cx)),null:column.nullable,default:column.default.is_some()||column.generated.is_some()}).collect();
                        for field in &s.fields {s._field_subscriptions.push(cx.subscribe(&field.input,|s,_,event,cx|{if matches!(event,InputEvent::Change){s.form_revision=s.form_revision.wrapping_add(1);s.staging.preview=None;cx.notify();}}));}
                        let sql=metadata.select_sql(s.page);s.metadata=Some(metadata);s.phase=Phase::Reading;s.status="Reading a bounded page on the existing connection…".into();
                        cx.emit(TableEditorEvent::Load{token:s.token.clone(),sql});
                    },Err(e)=>{if apply{s.fail(e,cx);}else{s.status=format!("Catalog viewer unavailable: {e}. Opening a read-only SELECT instead.");cx.emit(TableEditorEvent::ReadFallback{token:s.token.clone(),sql:dbide_ui_spike::workbench_model::table_query(DatabaseEngine::PostgreSql,&s.schema,&s.name)});}},
                }cx.notify();
            });
        }).detach();
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
            self.status = format!(
                "{:?} · transaction {:?} · {}. Reload before any further edits; no batch is replayed.",
                summary.outcome,
                summary.transaction_state,
                if summary.outcome == dbide_pg_spike::Outcome::Completed {
                    if summary.transaction_state == ServerTransactionState::Idle {
                        "Commit acknowledged"
                    } else {
                        "Submitted into open transaction; not committed"
                    }
                } else {
                    "No successful batch acknowledgement"
                }
            );
            self.error = summary.terminal_error.clone();
            if summary.outcome == dbide_pg_spike::Outcome::Completed {
                self.staging = Staging::default();
                self.selected = None;
                self.insert_mode = false;
            }
            cx.notify();
            return;
        }
        if summary.outcome != dbide_pg_spike::Outcome::Completed || summary.connection_retired {
            self.fail(
                summary
                    .terminal_error
                    .unwrap_or_else(|| format!("Row read {:?}; reload required", summary.outcome)),
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
            self.fail("Read returned an unexpected table projection".into(), cx);
            return;
        };
        match metadata.decode_rows(&page.columns, &page.rows, summary.truncated) {
            Ok(rows) => self.rows = rows,
            Err(error) => {
                metadata.read_only = Some(error.clone());
                self.error = Some(error);
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
        self.status = format!(
            "Server page {} · {} loaded rows · table total unknown",
            self.page + 1,
            self.grid.read(cx).delegate().row_count()
        );
        cx.notify();
    }
    fn form_values(&self, cx: &App) -> Result<Vec<Cell>, String> {
        let metadata = self.metadata.as_ref().ok_or("Metadata unavailable")?;
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
            "Discard unstaged row form changes?",
            Some("Stage the current row to keep it in the local transaction preview."),
            &["Keep form", "Discard form changes"],
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
        self.status = "New row form · changes remain local until Stage, Preview and Apply".into();
        cx.notify();
    }
    fn stage(&mut self, delete: bool, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        let result = (|| -> Result<(), String> {
            let metadata = self.metadata.as_ref().ok_or("Metadata unavailable")?;
            let spec = metadata.spec.as_ref().ok_or("Table is read-only")?;
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
                let row = self.selected.ok_or("Select a row first")?;
                let original = self
                    .rows
                    .get(row)
                    .ok_or("Original row unavailable")?
                    .clone();
                let edit = if delete {
                    Edit::Delete { original }
                } else {
                    let replacement = self.form_values(cx)?;
                    for (ix, col) in metadata.table.columns.iter().enumerate() {
                        if col.generated.is_some() && original[ix] != replacement[ix] {
                            return Err(format!(
                                "Generated/identity column {} is read-only",
                                col.name
                            ));
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
                self.status = format!(
                    "{} staged row edits · no SQL executed",
                    self.staging.count()
                );
            }
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }
    fn preview(&mut self, cx: &mut Context<Self>) {
        if self.form_dirty(cx) {
            self.error = Some(
                "Stage the current row form or discard its changes before previewing the batch."
                    .into(),
            );
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
            .ok_or("Table is read-only".into())
            .and_then(|spec| self.staging.plan(spec));
        match result {
            Ok(()) => {
                self.error = None;
                self.status =
                    "Transaction preview ready. Apply is a separate explicit submit gesture."
                        .into();
            }
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }
    fn reload(&mut self, page: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() || window.has_active_prompt() {
            return;
        }
        if self.has_unsaved(cx) {
            let expected = self.edit_revision();
            let receiver=window.prompt(PromptLevel::Warning,"Discard local row edits and reload?",Some("Both staged edits and unstaged form changes will be discarded. Nothing is replayed automatically."),&["Keep edits","Discard and reload"],cx);
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
        self.status="Local buffers discarded. Discard submits no SQL; prior execution/transaction outcomes are unchanged.".into();
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
            self.status = "No match in the currently loaded page (client search only)".into();
        }
        cx.notify();
    }
    fn button(id: &'static str, label: &'static str) -> Button {
        Button::new(id).label(label).small()
    }
}
impl Render for TableEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edit_controls = false; // M1 viewer: row writes await the M2 execution validation gate.
        let editable = edit_controls && self.editable();
        let busy = self.busy();
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
                        "Insert new row".into()
                    } else {
                        self.selected
                            .map(|r| format!("Edit row {} on this page", r + 1))
                            .unwrap_or_else(|| "Select a row to edit".into())
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
                    div()
                        .text_color(rgb(0xf0b797))
                        .child(format!("Read-only: {reason}")),
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
                                    "{} · {}{}{}",
                                    column.name,
                                    column.data_type,
                                    if column.nullable {
                                        " · nullable"
                                    } else {
                                        " · NOT NULL"
                                    },
                                    if generated {
                                        " · generated/identity"
                                    } else {
                                        ""
                                    }
                                )),
                        )
                        .when_some(column.default.as_ref(), |el, value| {
                            el.child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(MUTED))
                                    .child(format!("Default: {value}")),
                            )
                        })
                        .when_some(column.generated.as_ref(), |el, value| {
                            el.child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(MUTED))
                                    .child(format!("Generated: {value}")),
                            )
                        })
                        .child(
                            Textarea::new(&f.input)
                                .aria_label(format!("Row value for {}", column.name))
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
                                            .label(format!("Default · {}", column.name))
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
                            "Stage insert"
                        } else {
                            "Stage update"
                        },
                    )
                    .primary()
                    .disabled(!editable || (!self.insert_mode && self.selected.is_none()))
                    .on_click(cx.listener(|s, _, _, cx| s.stage(false, cx))),
                )
                .child(
                    Self::button("stage-delete", "Stage delete")
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
                            Self::button("reload-table", "Reload")
                                .disabled(busy)
                                .on_click(cx.listener(|s, _, w, cx| s.reload(s.page, w, cx))),
                        )
                        .child(
                            Self::button("previous-page", "Previous page")
                                .disabled(busy || self.page == 0)
                                .on_click(cx.listener(|s, _, w, cx| {
                                    s.reload(s.page.saturating_sub(1), w, cx)
                                })),
                        )
                        .child(
                            Self::button("next-page", "Next page")
                                .disabled(
                                    busy || self.grid.read(cx).delegate().row_count() < PAGE_ROWS,
                                )
                                .on_click(cx.listener(|s, _, w, cx| {
                                    s.reload(s.page.saturating_add(1), w, cx)
                                })),
                        )
                        .when(edit_controls, |el| {
                            el.child(
                                Self::button("insert-table-row", "New row")
                                    .disabled(!editable)
                                    .on_click(cx.listener(|s, _, w, cx| s.begin_insert(w, cx))),
                            )
                        })
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&self.search)
                                    .aria_label("Find in currently loaded table page")
                                    .small(),
                            ),
                        )
                        .child(
                            Self::button("find-page", "Find")
                                .disabled(busy)
                                .on_click(cx.listener(|s, _, w, cx| s.find(w, cx))),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(
                            div().flex_1().min_w_0().child(
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
                        .aria_label(self.status.clone())
                        .child(self.status.clone()),
                );
        if let Some(error) = &self.error {
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
                    .child(div().child(format!("{} staged edits", self.staging.count())))
                    .child(
                        Self::button("preview-table-plan", "Preview transaction")
                            .disabled(!editable || !self.has_staged())
                            .on_click(cx.listener(|s, _, _, cx| s.preview(cx))),
                    )
                    .child(
                        Self::button("discard-staged", "Discard staged")
                            .disabled(busy || !self.has_unsaved(cx))
                            .on_click(cx.listener(|s, _, w, cx| s.discard_edits(w, cx))),
                    )
                    .child(
                        Self::button("apply-table-plan", "Apply preview")
                            .primary()
                            .disabled(true)
                            .on_click(cx.listener(|s, _, w, cx| s.capture(true, w, cx))),
                    ),
            )
        });
        root = root.child(div().px_4().py_1().text_size(px(10.)).text_color(rgb(MUTED)).child("Read-only table viewer · row editing validation pending · each page is a fresh bounded SELECT"));
        if edit_controls && let Some(plan) = preview {
            root=root.child(div().child("Row editing validation pending · Apply disabled").id("table-plan-preview").max_h(px(180.)).overflow_y_scroll().px_4().py_2().bg(rgb(PANEL)).text_size(px(10.)).font_family("Menlo")
            .child("Explicit guarded transaction · DEFAULT is evaluated by the database · original-cell predicates · exactly1 affected row per statement · server defaults may advance sequences even after rollback · value checks do not detect ABA changes")
            .children(plan.statements.iter().enumerate().map(|(ix,statement)|div().id(("planned-edit",ix)).py_2().child(format!("{}: {}\nBound parameters: {:?}",ix+1,statement.sql,statement.parameters.iter().map(|p|&p.value).collect::<Vec<_>>())))));
        }
        root
    }
}
