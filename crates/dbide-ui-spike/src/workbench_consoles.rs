use super::*;
use dbide_ui_spike::console_session::DiagnosticKind;

fn startup_default_request(
    recovery_loading: bool,
    selected_profile_id: Option<&str>,
    profiles: &[ConnectionProfile],
    session: &ConsoleSession,
    required_profile_id: Option<&str>,
) -> Option<ConnectionRequest> {
    if !recovery_loading || session.target().is_some() || required_profile_id.is_some() {
        return None;
    }
    let selected = selected_profile_id?;
    let profile = profiles
        .iter()
        .find(|profile| profile.id == selected)?
        .clone();
    Some(ConnectionRequest {
        profile,
        password: String::new(),
    })
}

impl Workbench {
    /// Bind only the initial draft, before recovery captures its initial stamp.
    /// Recovery then replaces it with each document's exact target, including
    /// an unbound target when its saved profile cannot be resolved.
    pub(super) fn initialize_default_console_target(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.fixture_smoke || self.documents.len() != 1 {
            return;
        }
        let doc = &self.documents[0];
        let Some(request) = startup_default_request(
            self.recovery.loading,
            self.workspace.selected_profile_id.as_deref(),
            &self.workspace.profiles,
            &doc.session,
            doc.required_profile_id.as_deref(),
        ) else {
            return;
        };
        self.bind_console(doc.session.route(), request, window, cx);
    }

    pub(super) fn console(&self) -> &ConsoleSession {
        &self
            .documents
            .iter()
            .find(|d| d.model.id == self.active_document)
            .expect("active SQL document")
            .session
    }
    fn console_mut(&mut self) -> &mut ConsoleSession {
        &mut self
            .documents
            .iter_mut()
            .find(|d| d.model.id == self.active_document)
            .expect("active SQL document")
            .session
    }
    pub(super) fn console_slot_available(&self) -> bool {
        self.console().has_worker()
            || self
                .documents
                .iter()
                .filter(|d| d.session.has_worker())
                .count()
                < MAX_CONSOLE_WORKERS
    }
    pub(super) fn bind_console(
        &mut self,
        route: ConsoleRoute,
        request: ConnectionRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(doc) = self
            .documents
            .iter_mut()
            .find(|d| d.model.id == route.console_id)
        {
            match doc.session.bind(route, request) {
                Ok(()) => {
                    if let Some(request) = doc.session.request() {
                        doc.database_draft.update(cx, |input, cx| {
                            input.set_value(request.profile.database, window, cx)
                        });
                        doc.password_draft.update(cx, |input, cx| {
                            input.set_value(request.password, window, cx)
                        });
                    }
                    doc.required_profile_id = None;
                    self.status =
                        "Console target changed; other consoles and browser are unchanged".into();
                }
                Err(error) => self.status = error,
            }
        }
        self.completions.clear();
        self.sql_diagnostics.clear();
        cx.notify();
    }
    pub(super) fn prepare_console_password(&mut self, cx: &App) -> Result<(), String> {
        let doc = self
            .documents
            .iter_mut()
            .find(|d| d.model.id == self.active_document)
            .expect("active console");
        if doc.session.phase != SessionPhase::Disconnected {
            return Ok(());
        }
        if let Some(mut request) = doc.session.request() {
            let password = doc.password_draft.read(cx).value().to_string();
            if request.password != password {
                request.password = password;
                doc.session.bind(doc.session.route(), request)?;
            }
        }
        Ok(())
    }
    pub(super) fn connect_console(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.prepare_console_password(cx) {
            self.status = error;
            cx.notify();
            return;
        }
        let slot = self.console_slot_available();
        let route = self.console().route();
        let result = self.console_mut().connect(route, slot);
        self.console_submission(result, cx);
    }
    pub(super) fn disconnect_console(&mut self, route: ConsoleRoute, cx: &mut Context<Self>) {
        let result = self
            .documents
            .iter_mut()
            .find(|d| d.model.id == route.console_id)
            .map(|d| d.session.disconnect(route));
        if let Some(result) = result {
            self.console_submission(result, cx);
        }
    }
    fn console_submission(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        if let Err(error) = result {
            self.status = error.clone();
            self.console_mut().diagnostic=Some(dbide_ui_spike::console_session::SessionDiagnostic{title:"Console action not submitted".into(),message:error,hint:"Nothing was submitted. Review the console state or explicitly disconnect another console if the session limit was reached.".into(),kind:DiagnosticKind::Query});
        } else {
            self.status = self.console().status.clone();
        }
        cx.notify();
    }
    pub(super) fn submit_console_sql(&mut self, sql: String, cx: &mut Context<Self>) {
        if let Err(error) = self.prepare_console_password(cx) {
            self.status = error;
            cx.notify();
            return;
        }
        if !self.can_run(cx) {
            self.status = if self.console().target().is_none() {
                "Select a connection and database in this SQL tab first"
            } else {
                "This console is busy or the query is unavailable"
            }
            .into();
            cx.notify();
            return;
        }
        let Some(owner) = self.current_owner() else {
            return;
        };
        let slot = self.console_slot_available();
        let route = self.console().route();
        let result = self.console_mut().execute(route, sql, owner, slot);
        self.console_submission(result, cx);
    }
    pub(super) fn cancel_console(
        &mut self,
        route: ConsoleRoute,
        target: CancelTarget,
        cx: &mut Context<Self>,
    ) {
        let result = self
            .documents
            .iter_mut()
            .find(|d| d.model.id == route.console_id)
            .map(|d| d.session.cancel(route, target));
        if let Some(result) = result {
            self.console_submission(result, cx);
        }
    }
    pub(super) fn console_policy(&mut self, policy: CommitPolicy, cx: &mut Context<Self>) {
        let route = self.console().route();
        let result = self.console_mut().set_policy(route, policy);
        self.console_submission(result, cx);
    }
    pub(super) fn console_transaction(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(owner) = self.current_owner() else {
            return;
        };
        let route = self.console().route();
        let result = self.console_mut().transaction(route, commit, owner);
        self.console_submission(result, cx);
    }
    pub(super) fn poll_console_sessions(&mut self, cx: &mut Context<Self>) {
        if self.documents.is_empty() {
            return;
        }
        let count = self.documents.len();
        let mut effects = vec![];
        let mut changed = false;
        let mut active_changed = false;
        // A fixed global admission budget, round-robin across visible AND inactive consoles.
        for offset in 0..64 {
            let ix = (self.console_poll_cursor + offset) % count;
            if let Some(events) = self.documents[ix].session.poll_one() {
                effects.extend(events);
                if self.documents[ix].model.id == self.active_document {
                    active_changed = true;
                }
                changed = true;
            }
        }
        self.console_poll_cursor = (self.console_poll_cursor + 1) % count;
        for effect in effects {
            let ConsoleEffect::Finished {
                owner,
                sql,
                report,
                target,
            } = effect;
            let (pages, summary) = split_report(report);
            let detail = format!(
                "{:?} · {} retained rows · transaction at completion {:?}{}",
                summary.outcome,
                summary.retained_rows,
                summary.transaction_state,
                if summary.truncated {
                    " · retained results truncated"
                } else {
                    ""
                }
            );
            let snapshot = ResultSnapshot {
                pages: pages.into_iter().map(Rc::new).collect(),
                selected_page: 0,
                summary: Some(summary.clone()),
                details: detail,
                owner: Some(owner.clone()),
            };
            if let Some(doc) = self
                .documents
                .iter_mut()
                .find(|d| d.model.id == owner.document_id)
            {
                doc.results = snapshot.clone();
            }
            if owner.document_id == self.active_document {
                self.restore_results(snapshot, cx);
            }
            if let Some(sql) = sql {
                let entry = HistoryEntry {
                    profile_id: owner.profile_id.clone(),
                    execution_target: target,
                    sql,
                    executed_at_unix_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                    outcome: format!("{:?}", summary.outcome),
                    row_count: Some(summary.retained_rows as u64),
                };
                let _ = self.workspace.record_history(entry.clone());
                self.sync_history_panel(cx);
                if self.workspace.persist_history {
                    self.persist_profile_mutation(ProfileMutation::History(entry), cx);
                }
            }
        }
        if active_changed {
            self.status = self.console().status.clone();
        }
        if changed {
            cx.notify();
        }
    }
    pub(super) fn console_diagnostic(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let session = self.console();
        let diagnostic = session.diagnostic.as_ref().expect("console diagnostic");
        let route = session.route();
        let retry = diagnostic.kind == DiagnosticKind::Connection;
        let may_tx = session.may_have_transaction();
        let busy = session.busy();
        let title = self.ui_label(&diagnostic.title).to_string();
        let hint = if self.locale == Locale::Ko {
            match diagnostic.kind{DiagnosticKind::Connection=>"이 콘솔의 연결 정보와 세션 비밀번호를 확인한 후 직접 다시 시도하세요.",DiagnosticKind::Query if session.phase==SessionPhase::Disconnected=>"세션이 종료되었습니다. 다음 실행 때 새로 연결합니다. SQL은 자동으로 다시 실행하지 않습니다.",DiagnosticKind::Query=>"SQL을 확인하세요. 실패·미확인 트랜잭션에서는 롤백이 필요합니다. SQL은 자동으로 반복하지 않습니다.",DiagnosticKind::Metadata=>"메타데이터만 다시 불러오세요. SQL 실행과 트랜잭션 상태는 바꾸지 않습니다."}.to_string()
        } else {
            diagnostic.hint.clone()
        };
        div()
            .id("console-diagnostic")
            .role(Role::Alert)
            .aria_label(format!("{} · {} · {}", title, diagnostic.message, hint))
            .px_4()
            .py_3()
            .bg(rgb(0x302522))
            .border_y_1()
            .border_color(rgb(0x6d4635))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(
                div()
                    .mt_1()
                    .text_size(px(11.))
                    .child(diagnostic.message.clone()),
            )
            .child(
                div()
                    .mt_1()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(hint),
            )
            .child(
                div()
                    .mt_2()
                    .flex()
                    .gap_2()
                    .when(retry, |el| {
                        el.child(
                            self.button("retry-console-connect", "Retry console connection")
                                .disabled(busy)
                                .on_click(cx.listener(move |s, _, _, cx| {
                                    if s.console().route() == route {
                                        s.connect_console(cx);
                                    }
                                })),
                        )
                    })
                    .child(
                        self.button("console-error-rollback", "Rollback this console")
                            .disabled(busy || !may_tx)
                            .on_click(cx.listener(move |s, _, _, cx| {
                                if s.console().route() == route {
                                    s.console_transaction(false, cx);
                                }
                            })),
                    )
                    .child(
                        self.button("dismiss-console-error", "Dismiss")
                            .on_click(cx.listener(move |s, _, _, cx| {
                                if let Some(doc) =
                                    s.documents.iter_mut().find(|d| d.session.route() == route)
                                {
                                    doc.session.diagnostic = None;
                                }
                                cx.notify();
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn startup_selection_binds_exact_saved_profile_without_opening_a_worker() {
        let first = ConnectionProfile::fixture();
        let mut selected = first.clone();
        selected.id = "selected-second".into();
        selected.database = "selected_database".into();
        let mut session = ConsoleSession::new(1);
        let request = startup_default_request(
            true,
            Some(&selected.id),
            &[first, selected.clone()],
            &session,
            None,
        )
        .unwrap();
        assert!(request.password.is_empty());
        session.bind(session.route(), request).unwrap();
        assert_eq!(session.target(), Some(&selected));
        assert_eq!(session.phase, SessionPhase::Disconnected);
        assert!(!session.has_worker());
        assert!(!session.busy());
    }

    #[::core::prelude::v1::test]
    fn absent_selection_and_recovered_or_existing_targets_never_fall_back() {
        let profile = ConnectionProfile::fixture();
        let profiles = [profile.clone()];
        let mut session = ConsoleSession::new(1);
        assert!(startup_default_request(true, None, &profiles, &session, None).is_none());
        assert!(
            startup_default_request(true, Some("removed"), &profiles, &session, None).is_none()
        );
        assert!(
            startup_default_request(false, Some(&profile.id), &profiles, &session, None).is_none()
        );
        assert!(
            startup_default_request(
                true,
                Some(&profile.id),
                &profiles,
                &session,
                Some("removed")
            )
            .is_none()
        );
        session
            .bind(
                session.route(),
                ConnectionRequest {
                    profile: profile.clone(),
                    password: String::new(),
                },
            )
            .unwrap();
        assert!(
            startup_default_request(true, Some(&profile.id), &profiles, &session, None).is_none()
        );
    }
}
