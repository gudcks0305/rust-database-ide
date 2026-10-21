use super::*;
use table_editor::{TableEditor, TableEditorEvent, TableToken};
#[derive(Clone, Copy)]
pub(super) enum TableOperationKind {
    Load,
    Apply,
    Control,
}
#[derive(Clone, Copy)]
pub(super) struct TableOperation {
    pub(super) operation: dbide_bridge::OperationId,
    pub(super) owner: Option<EntityId>,
    pub(super) kind: TableOperationKind,
}
impl Workbench {
    pub(super) fn open_table_editor(
        &mut self,
        key: dbide_ui_spike::navigator::Key,
        request: ConnectionRequest,
        relation: dbide_ui_spike::navigator::Relation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.table_reader_busy(cx)
            || self.connection != ConnectionState::Connected
            || self.transaction_state != ServerTransactionState::Idle
        {
            return;
        }
        if request.profile.engine != DatabaseEngine::PostgreSql
            || self
                .configured
                .as_ref()
                .is_none_or(|current| current.profile != request.profile)
        {
            return;
        }
        let revision = self.connection_revision;
        self.disable_table_read_controls(cx);
        if let Some(id) = self.pending_table_tab.take() {
            let Some(tab) = self.table_tabs.iter_mut().find(|tab| tab.id == id) else {
                return;
            };
            // Assign the reader only after the exact branch/target/Idle handshake.
            // No prior capture/query may still be in flight at this point.
            tab.key = key;
            tab.request = request.clone();
            let editor = tab.editor.clone();
            self.table_editor = Some(editor.clone());
            editor.update(cx, |editor, cx| {
                editor.set_read_controls_enabled(true, cx);
                editor.rebind_readonly(request, revision, window, cx);
            });
            cx.notify();
            return;
        }
        if !table_tabs::model::can_add(self.table_tabs.len()) {
            return;
        }
        let schema = relation.schema.clone();
        let name = relation.name.clone();
        let page_size = self.app_settings.table_page_size;
        let editor = cx.new(|cx| {
            let mut editor = TableEditor::new_with_locale(
                request,
                schema,
                name,
                revision,
                self.locale,
                window,
                cx,
            );
            editor.set_page_size_default(usize::from(page_size), cx);
            editor
        });
        let request = self.configured.clone().expect("table reader configured");
        let id = self.next_table_tab_id;
        self.next_table_tab_id = self.next_table_tab_id.saturating_add(1);
        self._subscriptions
            .push(cx.subscribe_in(&editor, window, |s, source, event, w, cx| {
                s.handle_table_event(source.entity_id(), event, w, cx)
            }));
        self.table_tabs.push(table_tabs::TableTab {
            id,
            key,
            request,
            relation,
            editor: editor.clone(),
        });
        self.table_editor = Some(editor);
        self.active_table_tab = Some(id);
        self.tools = None;
        cx.notify();
    }
    fn table_binding_valid(&self, token: &TableToken, apply: bool) -> bool {
        self.table_tabs.iter().any(|tab| {
            self.table_editor
                .as_ref()
                .is_some_and(|owner| owner.entity_id() == tab.editor.entity_id())
                && self.navigator.authorized(&tab.key)
        }) && self.configured.as_ref().is_some_and(|r| {
            token.matches(
                &TableToken {
                    profile: r.profile.clone(),
                    connection_revision: self.connection_revision,
                    row_execution: self.last_execution,
                },
                apply,
            )
        }) && self.connection == ConnectionState::Connected
            && !self.busy()
    }
    fn handle_table_event(
        &mut self,
        source: EntityId,
        event: &TableEditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .table_editor
            .as_ref()
            .is_none_or(|e| e.entity_id() != source)
        {
            return;
        }
        if let TableEditorEvent::ReadFallback { token, sql } = event {
            if self.table_binding_valid(token, false) {
                if let Some(editor) = self.table_editor.clone() {
                    editor.update(cx, |editor, cx| editor.fail("Paged table reading is unavailable for this relation. Open the generated SQL query to inspect it.".into(), cx));
                }
                self.active_table_tab = None;
                self.open_data_query(sql.clone(), "table-data.sql".into(), window, cx);
            } else if let Some(editor) = self.table_editor.clone() {
                editor.update(cx, |e, cx| {
                    e.fail(
                        "Connection context changed. Reopen Data under the intended connection."
                            .into(),
                        cx,
                    )
                });
            }
            return;
        }
        let (token, apply) = match event {
            TableEditorEvent::ReadFallback { .. } => unreachable!(),
            TableEditorEvent::Load { token, .. } => (token, false),
            TableEditorEvent::Apply { token, .. } => (token, true),
        };
        if apply {
            if let Some(editor) = self.table_editor.clone() {
                editor.update(cx, |e, cx| {
                    e.fail(
                        "Row editing validation pending. No SQL was submitted.".into(),
                        cx,
                    )
                });
            }
            return;
        }
        if !self.table_binding_valid(token, apply) {
            if let Some(editor) = self.table_editor.clone() {
                editor.update(cx,|e,cx|e.fail("Connection/profile/execution context changed or is busy. Close, reconnect if needed, and reload the table; no SQL was submitted.".into(),cx));
            }
            return;
        }
        let result = if let Some(bridge) = &self.bridge {
            match event {
                TableEditorEvent::ReadFallback { .. } => unreachable!(),
                TableEditorEvent::Load { sql, .. } => bridge.execute(sql.clone()),
                TableEditorEvent::Apply {
                    guard, statements, ..
                } => bridge.apply_guarded_batch(guard.clone(), statements.clone()),
            }
        } else {
            return;
        };
        match result {
            Ok(operation) => {
                self.active_operation = Some(operation);
                self.table_operation = Some(TableOperation {
                    operation,
                    owner: Some(source),
                    kind: if apply {
                        TableOperationKind::Apply
                    } else {
                        TableOperationKind::Load
                    },
                });
                self.pending_owner = None;
                self.pending_sql = None;
                self.connection = ConnectionState::Executing;
            }
            Err(error) => {
                if let Some(editor) = self.table_editor.clone() {
                    editor.update(cx, |e, cx| {
                        e.fail(format!("Table operation was not submitted: {error:?}"), cx)
                    });
                }
            }
        }
        cx.notify();
    }
    pub(super) fn close_table_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_table_tab {
            self.close_table_tab(id, window, cx);
        }
    }
    pub(super) fn table_report(
        &mut self,
        report: dbide_pg_spike::ExecutionReport,
        captured: TableOperation,
        cx: &mut Context<Self>,
    ) {
        if self.active_operation != Some(captured.operation) {
            return;
        }
        let recipient = self
            .table_tabs
            .iter()
            .find(|tab| Some(tab.editor.entity_id()) == captured.owner)
            .map(|tab| tab.editor.clone());
        let deliver = table_tabs::model::accepts_report(
            captured.operation,
            self.active_operation,
            captured.owner,
            recipient.as_ref().map(Entity::entity_id),
        );
        self.active_operation = None;
        self.active_execution = None;
        self.commit_policy = report.commit_policy;
        self.transaction_state = report.transaction_state;
        self.connection = if report.connection_retired {
            self.connection_revision = self.connection_revision.saturating_add(1);
            self.last_execution = None;
            ConnectionState::Disconnected
        } else {
            ConnectionState::Connected
        };
        self.status = format!(
            "Last table operation · {:?} · transaction {:?}",
            report.outcome, report.transaction_state
        );
        if report.outcome == dbide_pg_spike::Outcome::Completed
            && self.diagnostic.as_ref().is_some_and(|d| {
                d.context == DiagnosticContext::Query
                    && d.owner.as_ref().is_some_and(|owner| {
                        self.configured.as_ref().is_some_and(|r| {
                            r.profile.id == owner.profile_id
                                && dbide_ui_spike::workbench_model::profile_target(&r.profile)
                                    == owner.target
                        })
                    })
            })
        {
            self.diagnostic = None;
        }
        if self.table_editor.is_none()
            && matches!(captured.kind, TableOperationKind::Control)
            && let Some(feedback) = &mut self.table_open_feedback
        {
            feedback.message = if report.outcome == dbide_pg_spike::Outcome::Completed {
                if self.locale==Locale::Ko{"테이블 연결의 트랜잭션 제어가 완료되었습니다. 데이터 열기는 직접 다시 시도하세요."}else{"Table transaction control completed. Retry Data explicitly."}.into()
            } else {
                report.terminal_error.as_ref().map(|e|e.message.clone()).unwrap_or_else(||if self.locale==Locale::Ko{"테이블 연결 트랜잭션 제어를 확인하지 못했습니다. 실제 상태를 확인한 뒤 연결을 끊거나 직접 다시 시도하세요."}else{"Table transaction control was not confirmed. Inspect its wire state, then disconnect or retry explicitly."}.into())
            };
            feedback.neutral = report.outcome == dbide_pg_spike::Outcome::Completed;
        }
        if deliver && let Some(editor) = recipient {
            editor.update(cx, |editor, cx| match captured.kind {
                TableOperationKind::Load => {
                    editor.accept_report(report, self.connection_revision, false, cx)
                }
                TableOperationKind::Apply => {
                    editor.accept_report(report, self.connection_revision, true, cx)
                }
                TableOperationKind::Control => editor.accept_control(report, cx),
            });
        }
        cx.notify();
    }
    pub(super) fn table_transaction(&mut self, commit: bool, cx: &mut Context<Self>) {
        if self.busy()
            || self.connection != ConnectionState::Connected
            || self.active_table_entity().as_ref().map(Entity::entity_id)
                != self.table_editor.as_ref().map(Entity::entity_id)
        {
            return;
        }
        self.finish_transaction(commit, cx);
        if let Some(operation) = self.active_operation {
            self.pending_owner = None;
            self.table_operation = Some(TableOperation {
                operation,
                owner: self.table_editor.as_ref().map(Entity::entity_id),
                kind: TableOperationKind::Control,
            });
        }
    }
    fn close_table_shortcut(&mut self, _: &CloseSqlTab, w: &mut Window, cx: &mut Context<Self>) {
        if self.preferences.take().is_some() {
            cx.notify();
            return;
        }
        self.close_table_editor(w, cx);
    }
    pub(super) fn table_editor_view(
        &self,
        editor: &Entity<TableEditor>,
        cx: &mut Context<Self>,
    ) -> Div {
        let owns_reader = self
            .table_editor
            .as_ref()
            .is_some_and(|owner| owner.entity_id() == editor.entity_id());
        let busy = self.busy() || editor.read(cx).is_busy() || !owns_reader;
        let transaction = if owns_reader {
            self.transaction_state
        } else {
            ServerTransactionState::Unknown
        };
        let token = editor.read(cx).token();
        let context = format!(
            "{} · {} · {} · {} · {}: {}",
            tr(self.locale, Message::TableData),
            editor.read(cx).label(),
            token.profile.name,
            dbide_ui_spike::workbench_model::profile_target(&token.profile),
            tr(self.locale, Message::Transaction),
            self.tx_label(transaction)
        );
        let caption = self
            .table_tabs
            .iter()
            .find(|tab| tab.editor.entity_id() == editor.entity_id())
            .map(|tab| format!("{}.{}", token.profile.database, tab.relation.name))
            .unwrap_or_else(|| editor.read(cx).label());
        let cancel = self
            .active_execution
            .map(CancelTarget::Execution)
            .or_else(|| self.active_operation.map(CancelTarget::Operation));
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family(".AppleSystemUIFont")
            .text_size(px(12.))
            .on_action(cx.listener(Self::close_table_shortcut))
            .on_action(cx.listener(Self::quit_action))
            .child(
                div()
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .id("table-context-status")
                            .role(Role::Status)
                            .aria_label(context.clone())
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .child(caption)
                            .tooltip(move |_, cx| cx.new(|_| gpui_component::tooltip::Tooltip::new(context.clone())).into()),
                    )
                    .when(owns_reader && self.transaction_may_be_open(), |el| {
                        el.when(self.transaction_state == ServerTransactionState::Active, |el| el.child(self.button("table-commit", "Commit")
                            .disabled(
                                busy || self.transaction_state != ServerTransactionState::Active,
                            )
                            .on_click(cx.listener(|s, _, _, cx| s.table_transaction(true, cx)))))
                        .child(
                        self.button("table-rollback", "Rollback")
                            .disabled(busy || !self.transaction_may_be_open())
                            .on_click(cx.listener(|s, _, _, cx| s.table_transaction(false, cx))),
                        )
                    })
                    .when(owns_reader && matches!(self.connection, ConnectionState::Executing | ConnectionState::Cancelling), |el| el.child(
                        self.button("cancel-table-operation", "Cancel")
                            .disabled(!owns_reader || self.connection != ConnectionState::Executing)
                            .on_click(cx.listener(move |s, _, _, cx| s.cancel(cancel, cx))),
                    ))
                    .child(
                        self.button("close-table-editor", tr(self.locale, Message::CloseTable))
                            .disabled(self.active_operation.is_some() || editor.read(cx).is_busy())
                            .on_click(cx.listener(|s, _, w, cx| s.close_table_editor(w, cx))),
                    ),
            )
            .when(!owns_reader, |el| el.child(div().px_4().py_2().text_color(rgb(MUTED)).child(if self.locale == Locale::Ko { "저장된 테이블 화면입니다. 이 탭을 다시 선택하면 테이블 전용 연결을 확인한 뒤 읽기를 활성화합니다." } else { "Retained table snapshot. Select this tab to verify its reader connection and enable reads." })))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().flex_1().min_w_0().child(editor.clone()))
,
            )
    }
}
