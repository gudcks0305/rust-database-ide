use super::*;
use table_editor::{TableEditor, TableEditorEvent, TableToken};
#[derive(Clone, Copy)]
pub(super) enum TableOperation {
    Load,
    Apply,
    Control,
}
impl Workbench {
    pub(super) fn open_table_editor(
        &mut self,
        schema: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() || self.connection != ConnectionState::Connected {
            return;
        }
        let Some(request) = self.configured.clone() else {
            return;
        };
        if request.profile.engine != DatabaseEngine::PostgreSql {
            return;
        }
        let revision = self.connection_revision;
        let editor = cx.new(|cx| {
            TableEditor::new_with_locale(request, schema, name, revision, self.locale, window, cx)
        });
        self._subscriptions
            .push(cx.subscribe_in(&editor, window, |s, source, event, w, cx| {
                s.handle_table_event(source.entity_id(), event, w, cx)
            }));
        self.table_editor = Some(editor);
        cx.notify();
    }
    fn table_binding_valid(&self, token: &TableToken, apply: bool) -> bool {
        self.configured.as_ref().is_some_and(|r| {
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
                self.table_editor = None;
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
                self.table_operation = Some(if apply {
                    TableOperation::Apply
                } else {
                    TableOperation::Load
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
        if self.active_operation.is_some()
            || window.has_active_prompt()
            || self
                .table_editor
                .as_ref()
                .is_some_and(|e| e.read(cx).is_busy())
        {
            return;
        }
        if self
            .table_editor
            .as_ref()
            .is_some_and(|e| e.read(cx).has_unsaved(cx))
        {
            let expected = self
                .table_editor
                .as_ref()
                .map(|e| (e.entity_id(), e.read(cx).edit_revision()));
            let receiver=window.prompt(PromptLevel::Warning,"Discard local table edits?",Some("Closing discards the local staging and form buffers. It does not resolve a server transaction or an unknown prior execution outcome."),&["Keep editing","Discard and close"],cx);
            cx.spawn_in(window, async move |view, cx| {
                if receiver.await == Ok(1) {
                    let _ = view.update_in(cx, |s, _, cx| {
                        if s.active_operation.is_none()
                            && s.table_editor.as_ref().is_some_and(|e| {
                                expected == Some((e.entity_id(), e.read(cx).edit_revision()))
                                    && !e.read(cx).is_busy()
                            })
                        {
                            s.table_editor = None;
                        }
                        cx.notify();
                    });
                }
            })
            .detach();
        } else {
            self.table_editor = None;
            cx.notify();
        }
    }
    pub(super) fn table_report(
        &mut self,
        report: dbide_pg_spike::ExecutionReport,
        kind: TableOperation,
        cx: &mut Context<Self>,
    ) {
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
            && matches!(kind, TableOperation::Control)
            && let Some(feedback) = &mut self.table_open_feedback
        {
            feedback.message = if report.outcome == dbide_pg_spike::Outcome::Completed {
                if self.locale==Locale::Ko{"테이블 연결의 트랜잭션 제어가 완료되었습니다. 데이터 열기는 직접 다시 시도하세요."}else{"Table transaction control completed. Retry Data explicitly."}.into()
            } else {
                report.terminal_error.as_ref().map(|e|e.message.clone()).unwrap_or_else(||if self.locale==Locale::Ko{"테이블 연결 트랜잭션 제어를 확인하지 못했습니다. 실제 상태를 확인한 뒤 연결을 끊거나 직접 다시 시도하세요."}else{"Table transaction control was not confirmed. Inspect its wire state, then disconnect or retry explicitly."}.into())
            };
            feedback.neutral = report.outcome == dbide_pg_spike::Outcome::Completed;
        }
        if let Some(editor) = self.table_editor.clone() {
            editor.update(cx, |editor, cx| match kind {
                TableOperation::Load => {
                    editor.accept_report(report, self.connection_revision, false, cx)
                }
                TableOperation::Apply => {
                    editor.accept_report(report, self.connection_revision, true, cx)
                }
                TableOperation::Control => editor.accept_control(report, cx),
            });
        }
        cx.notify();
    }
    pub(super) fn table_transaction(&mut self, commit: bool, cx: &mut Context<Self>) {
        if self.busy() || self.connection != ConnectionState::Connected {
            return;
        }
        self.finish_transaction(commit, cx);
        if self.active_operation.is_some() {
            self.pending_owner = None;
            self.table_operation = Some(TableOperation::Control);
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
        let busy = self.busy() || editor.read(cx).is_busy();
        let token = editor.read(cx).token();
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
                    .h(px(48.))
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .id("table-context-status")
                            .role(Role::Status)
                            .aria_label(format!(
                                "{} · {} · {} · {} · {}: {}",
                                tr(self.locale, Message::TableData),
                                editor.read(cx).label(),
                                token.profile.name,
                                dbide_ui_spike::workbench_model::profile_target(&token.profile),
                                tr(self.locale, Message::Transaction),
                                self.tx_label(self.transaction_state)
                            ))
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .child(format!(
                                "{} · {} · {} · {} · {}: {}",
                                tr(self.locale, Message::TableData),
                                editor.read(cx).label(),
                                token.profile.name,
                                dbide_ui_spike::workbench_model::profile_target(&token.profile),
                                tr(self.locale, Message::Transaction),
                                self.tx_label(self.transaction_state)
                            )),
                    )
                    .child(
                        self.button("table-commit", "Commit")
                            .disabled(
                                busy || self.transaction_state != ServerTransactionState::Active,
                            )
                            .on_click(cx.listener(|s, _, _, cx| s.table_transaction(true, cx))),
                    )
                    .child(
                        self.button("table-rollback", "Rollback")
                            .disabled(busy || !self.transaction_may_be_open())
                            .on_click(cx.listener(|s, _, _, cx| s.table_transaction(false, cx))),
                    )
                    .child(
                        self.button("cancel-table-operation", "Cancel")
                            .disabled(self.connection != ConnectionState::Executing)
                            .on_click(cx.listener(move |s, _, _, cx| s.cancel(cancel, cx))),
                    )
                    .child(
                        self.button("close-table-editor", tr(self.locale, Message::CloseTable))
                            .disabled(self.active_operation.is_some() || editor.read(cx).is_busy())
                            .on_click(cx.listener(|s, _, w, cx| s.close_table_editor(w, cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().flex_1().min_w_0().child(editor.clone()))
                    .when_some(self.preferences.as_ref(), |el, prefs| {
                        el.child(self.preferences_panel(prefs, cx))
                    }),
            )
    }
}
