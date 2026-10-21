//! Host orchestration for the isolated, generated-SQL-only table reader.
use super::*;
use dbide_ui_spike::{
    navigator::{Key, Relation},
    table_open_model::{OpenPhase, PendingTableOpen},
};
pub(super) struct TableOpenFeedback {
    key: Key,
    relation: Relation,
    profile: ConnectionProfile,
    pub(super) message: String,
    pub(super) neutral: bool,
}
impl TableOpenFeedback {
    fn from_pending(pending: &PendingTableOpen, message: String, neutral: bool) -> Self {
        Self {
            key: pending.key().clone(),
            relation: pending.relation().clone(),
            profile: pending.request().profile.clone(),
            message,
            neutral,
        }
    }
}
impl Workbench {
    pub(super) fn begin_table_open(
        &mut self,
        key: Key,
        request: ConnectionRequest,
        relation: Relation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending_table_open.is_some() {
            return;
        }
        let profile = request.profile.clone();
        let fail = |s: &mut Self, message: String| {
            s.table_open_feedback = Some(TableOpenFeedback {
                key: key.clone(),
                relation: relation.clone(),
                profile: profile.clone(),
                message,
                neutral: false,
            });
        };
        if !self.navigator.authorized(&key) {
            fail(
                self,
                if self.locale == Locale::Ko {
                    "연결 정보가 변경되었습니다. 현재 탐색기 가지에서 다시 여세요."
                } else {
                    "Connection changed. Reopen Data from the current navigator branch."
                }
                .into(),
            );
            cx.notify();
            return;
        }
        if self.busy() || self.profile_write_busy || self.table_editor.is_some() {
            fail(self,if self.locale==Locale::Ko{"진행 중인 테이블 연결 작업을 마치거나 현재 데이터 뷰를 닫은 뒤 다시 시도하세요. SQL 콘솔은 변경하지 않았습니다."}else{"Finish the table connection operation or close the current data view before retrying. SQL consoles are unchanged."}.into());
            cx.notify();
            return;
        }
        let mut pending =
            match PendingTableOpen::new(key.clone(), request.clone(), relation.clone()) {
                Ok(pending) => pending,
                Err(error) => {
                    fail(self, error);
                    cx.notify();
                    return;
                }
            };
        if self.connection == ConnectionState::Connected
            && self
                .configured
                .as_ref()
                .is_some_and(|current| current.profile == request.profile)
        {
            self.table_open_feedback = None;
            self.open_table_editor(relation.schema, relation.name, window, cx);
            return;
        }
        if self.transaction_may_be_open() {
            fail(self,if self.locale==Locale::Ko{"테이블 전용 연결의 트랜잭션을 먼저 롤백하거나 명시적으로 연결을 끊으세요. SQL 콘솔의 트랜잭션은 건드리지 않습니다."}else{"Rollback or explicitly disconnect the table reader's transaction before changing its target. SQL-console transactions are untouched."}.into());
            cx.notify();
            return;
        }
        if self.configure(request, cx) {
            let operation = self
                .configuration_operation
                .expect("configuration accepted");
            pending.configure(operation);
            self.pending_table_open = Some(pending);
            self.table_open_feedback = None;
        } else {
            fail(self,if self.locale==Locale::Ko{"테이블 전용 연결 요청을 제출하지 못했습니다. 연결 상태를 확인한 뒤 직접 다시 시도하세요."}else{"Table connection preparation was not submitted. Review its state, then retry explicitly."}.into());
        }
        cx.notify();
    }
    /// Retire ONLY the legacy table bridge. Clear its UI binding before ignoring
    /// late acknowledgements; no old target can authorize a later table query.
    fn retire_table_reader(&mut self, cx: &mut Context<Self>) {
        self.configuration_operation = None;
        self.pending_configuration = None;
        self.connect_after_configuration = false;
        self.connection_operation = None;
        self.policy_operation = None;
        self.before_test = None;
        self.active_operation = None;
        self.active_execution = None;
        self.table_operation = None;
        self.pending_owner = None;
        self.pending_sql = None;
        self.configured = None;
        self.last_execution = None;
        self.transaction_state = ServerTransactionState::Unknown;
        self.connection_revision = self.connection_revision.saturating_add(1);
        match self.bridge.as_ref().map(DatabaseBridge::disconnect) {
            Some(Ok(operation)) => {
                self.table_disconnect_operation = Some(operation);
                self.connection = ConnectionState::Disconnecting;
            }
            Some(Err(error)) => {
                self.table_disconnect_operation = None;
                self.connection = ConnectionState::Disconnecting;
                if let Some(feedback) = &mut self.table_open_feedback {
                    feedback
                        .message
                        .push_str(&format!(" · Disconnect not submitted: {error:?}"));
                }
            }
            None => {
                self.table_disconnect_operation = None;
                self.connection = ConnectionState::Disconnected;
            }
        }
        cx.notify();
    }
    pub(super) fn abort_table_open(
        &mut self,
        message: String,
        neutral: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(mut pending) = self.pending_table_open.take() {
            pending.close();
            self.table_open_feedback =
                Some(TableOpenFeedback::from_pending(&pending, message, neutral));
            self.retire_table_reader(cx);
        }
    }
    pub(super) fn check_table_open_context(&mut self, cx: &mut Context<Self>) {
        if self
            .pending_table_open
            .as_ref()
            .is_some_and(|p| !self.navigator.authorized(p.key()))
        {
            self.abort_table_open(if self.locale==Locale::Ko{"탐색기 연결 정보가 변경되어 데이터 열기를 취소했습니다. SQL은 제출하지 않았습니다."}else{"Navigator context changed; table opening was cancelled before SQL submission."}.into(),false,cx);
        }
    }
    pub(super) fn table_open_event(&mut self, event: &BridgeEvent, cx: &mut Context<Self>) -> bool {
        if let BridgeEvent::Error {
            operation: Some(operation),
            message,
        } = event
            && self.table_disconnect_operation == Some(*operation)
        {
            self.table_disconnect_operation = None;
            if let Some(feedback) = &mut self.table_open_feedback {
                feedback.message = format!(
                    "{} · {message}",
                    if self.locale == Locale::Ko {
                        "테이블 연결 종료를 확인하지 못했습니다. 다시 연결 끊기를 시도하거나 앱을 다시 여세요."
                    } else {
                        "Table disconnect could not be confirmed. Retry Disconnect or reopen the app."
                    }
                );
            }
            cx.notify();
            return true;
        }
        let Some(pending) = &self.pending_table_open else {
            return false;
        };
        let error = match event {
            BridgeEvent::ConnectionFailed {
                operation,
                diagnostic,
            } if pending.owns(*operation) => Some(format!(
                "{} · {} · {}",
                dbide_ui_spike::workbench_model::connection_target(&diagnostic.target),
                diagnostic.message,
                diagnostic.hint
            )),
            BridgeEvent::Error { operation, message }
                if operation.is_none() || operation.is_some_and(|op| pending.owns(op)) =>
            {
                Some(message.clone())
            }
            _ => None,
        };
        if let Some(error) = error {
            if matches!(pending.phase(), OpenPhase::Configuring(_)) {
                // Configure rejection retains the old native session. Do not
                // silently disconnect it (especially after an unknown outcome).
                let mut pending = self.pending_table_open.take().unwrap();
                pending.close();
                self.table_open_feedback =
                    Some(TableOpenFeedback::from_pending(&pending, error, false));
                self.configuration_operation = None;
                self.pending_configuration = None;
                self.connect_after_configuration = false;
                cx.notify();
                return true;
            }
            self.abort_table_open(error, false, cx);
            return true;
        }
        false
    }
    pub(super) fn connect_pending_table(
        &mut self,
        configured: dbide_bridge::OperationId,
        cx: &mut Context<Self>,
    ) {
        if !self
            .pending_table_open
            .as_ref()
            .is_some_and(|p| p.waiting_configured(configured))
        {
            return;
        }
        self.connect(cx);
        if let Some(operation) = self.connection_operation {
            if let Some(pending) = &mut self.pending_table_open {
                pending.connect(configured, operation);
            }
        } else {
            self.abort_table_open(
                if self.locale == Locale::Ko {
                    "테이블 연결을 시작하지 못했습니다. 직접 다시 시도하세요."
                } else {
                    "Table Connect was not submitted. Retry explicitly."
                }
                .into(),
                false,
                cx,
            );
        }
    }
    pub(super) fn table_reader_disconnected(&mut self, cx: &mut Context<Self>) {
        if let Some(mut pending) = self.pending_table_open.take() {
            pending.close();
            self.table_open_feedback = Some(TableOpenFeedback::from_pending(
                &pending,
                if self.locale == Locale::Ko {
                    "테이블 연결이 종료되어 읽기를 시작하지 않았습니다. 직접 다시 시도하세요."
                } else {
                    "Table connection closed before reading. Retry explicitly."
                }
                .into(),
                false,
            ));
        }
        self.configuration_operation = None;
        self.pending_configuration = None;
        self.connection_operation = None;
        self.policy_operation = None;
        self.connect_after_configuration = false;
        self.configured = None;
        cx.notify();
    }
    pub(super) fn observe_pending_table(
        &mut self,
        operation: dbide_bridge::OperationId,
        state: ServerTransactionState,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pending) = &mut self.pending_table_open else {
            return false;
        };
        match pending.observed(operation, state) {
            Ok(handled) => handled,
            Err(error) => {
                self.abort_table_open(error, false, cx);
                true
            }
        }
    }
    pub(super) fn finish_pending_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .pending_table_open
            .as_ref()
            .is_some_and(|p| p.phase() == OpenPhase::Ready)
            || window.has_active_prompt()
        {
            return;
        }
        let valid = self.pending_table_open.as_ref().is_some_and(|p| {
            p.ready_for(
                self.navigator.authorized(p.key()),
                self.configured.as_ref().map(|r| &r.profile),
                self.connection == ConnectionState::Connected,
                self.busy(),
                self.transaction_state,
            )
        }) && self.table_editor.is_none()
            && self.tools.is_none()
            && !self.recovery.close_after;
        if !valid {
            self.abort_table_open(if self.locale==Locale::Ko{"테이블 연결 상태가 변경되어 읽기를 시작하지 않았습니다. 직접 다시 시도하세요."}else{"Table connection/view context changed; no table SQL was submitted. Retry explicitly."}.into(),false,cx);
            return;
        }
        let pending = self.pending_table_open.take().unwrap();
        self.table_open_feedback = None;
        self.open_table_editor(
            pending.relation().schema.clone(),
            pending.relation().name.clone(),
            window,
            cx,
        );
    }
    fn rollback_table_reader(&mut self, cx: &mut Context<Self>) {
        if self.busy()
            || self.connection != ConnectionState::Connected
            || !self.transaction_may_be_open()
        {
            return;
        }
        // Never use finish_transaction here: without a TableEditor that generic
        // dispatcher routes to the active SQL console. This bridge alone is owned.
        match self.bridge.as_ref().map(DatabaseBridge::rollback) {
            Some(Ok(operation)) => {
                self.active_operation = Some(operation);
                self.active_execution = None;
                self.table_operation = Some(TableOperation::Control);
                self.pending_owner = None;
                self.pending_sql = None;
                self.connection = ConnectionState::Executing;
            }
            Some(Err(error)) => {
                if let Some(feedback) = &mut self.table_open_feedback {
                    feedback.message = format!("Table rollback not submitted: {error:?}");
                }
            }
            None => {}
        }
        cx.notify();
    }
    pub(super) fn table_open_banner(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let ko = self.locale == Locale::Ko;
        let (profile, relation, message, neutral) = if let Some(pending) = &self.pending_table_open
        {
            let phase = match pending.phase() {
                OpenPhase::Unsubmitted | OpenPhase::Configuring(_) => {
                    if ko {
                        "테이블 전용 연결 정보 준비 중…"
                    } else {
                        "Preparing the table reader's connection…"
                    }
                }
                OpenPhase::Connecting(_) => {
                    if ko {
                        "테이블 전용 연결 중…"
                    } else {
                        "Connecting the table reader…"
                    }
                }
                OpenPhase::Observing(_) => {
                    if ko {
                        "테이블 연결의 실제 트랜잭션 상태 확인 중…"
                    } else {
                        "Observing the table connection's transaction state…"
                    }
                }
                OpenPhase::Ready => {
                    if ko {
                        "테이블 뷰를 여는 중…"
                    } else {
                        "Opening the table view…"
                    }
                }
                OpenPhase::Closed => {
                    if ko {
                        "테이블 열기 종료"
                    } else {
                        "Table opening closed"
                    }
                }
            };
            (&pending.request().profile, pending.relation(), phase, true)
        } else {
            let feedback = self
                .table_open_feedback
                .as_ref()
                .expect("table opening feedback");
            (
                &feedback.profile,
                &feedback.relation,
                feedback.message.as_str(),
                feedback.neutral,
            )
        };
        let label = format!(
            "{} · {}.{} · {} · {}",
            profile.name,
            relation.schema,
            relation.name,
            dbide_ui_spike::workbench_model::profile_target(profile),
            message
        );
        let mut banner=div().id("table-open-status").role(if neutral{Role::Status}else{Role::Alert}).aria_label(label.clone()).flex_shrink_0().px_4().py_2().border_b_1().border_color(rgb(BORDER)).bg(rgb(PANEL)).child(div().font_weight(FontWeight::SEMIBOLD).child(if ko{"테이블 데이터"}else{"Table data"})).child(div().text_size(px(11.)).child(label)).child(div().text_size(px(10.)).text_color(rgb(MUTED)).child(if ko{"이 작업은 테이블 전용 연결만 사용합니다. SQL 탭의 연결과 트랜잭션은 바꾸지 않습니다."}else{"This operation owns only the table connection. SQL tabs and their transactions are unchanged."}));
        let mut actions = div().mt_1().flex().flex_wrap().gap_2();
        if self.pending_table_open.is_some() {
            actions = actions.child(
                Button::new("cancel-table-open")
                    .small()
                    .label(if ko {
                        "테이블 열기 취소"
                    } else {
                        "Cancel table opening"
                    })
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.abort_table_open(
                            if s.locale == Locale::Ko {
                                "테이블 열기를 취소했습니다. SQL은 제출하지 않았습니다."
                            } else {
                                "Table opening cancelled; no table SQL was submitted."
                            }
                            .into(),
                            true,
                            cx,
                        )
                    })),
            );
        } else if let Some(feedback) = &self.table_open_feedback {
            let key = feedback.key.clone();
            let relation = feedback.relation.clone();
            actions = actions
                .child(
                    Button::new("retry-table-open")
                        .small()
                        .label(if ko { "다시 시도" } else { "Retry" })
                        .disabled(self.busy() || !self.navigator.authorized(&key))
                        .on_click(cx.listener(move |s, _, w, cx| {
                            if let Some(request) = s.navigator_request(&key) {
                                s.begin_table_open(key.clone(), request, relation.clone(), w, cx);
                            }
                        })),
                )
                .child(
                    Button::new("rollback-table-reader")
                        .small()
                        .label(if ko {
                            "테이블 연결 롤백"
                        } else {
                            "Rollback table connection"
                        })
                        .disabled(self.busy() || !self.transaction_may_be_open())
                        .on_click(cx.listener(|s, _, _, cx| s.rollback_table_reader(cx))),
                )
                .child(
                    Button::new("disconnect-table-reader")
                        .small()
                        .label(if ko {
                            "테이블 연결 끊기"
                        } else {
                            "Disconnect table connection"
                        })
                        .disabled(
                            self.table_disconnect_operation.is_some()
                                || self.active_operation.is_some(),
                        )
                        .on_click(cx.listener(|s, _, _, cx| s.retire_table_reader(cx))),
                )
                .child(
                    Button::new("dismiss-table-open")
                        .small()
                        .label(self.ui_label("Dismiss"))
                        .disabled(self.table_disconnect_operation.is_some())
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.table_open_feedback = None;
                            cx.notify();
                        })),
                );
            banner = banner.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "{}: {} · {}",
                        if ko {
                            "테이블 연결"
                        } else {
                            "Table connection"
                        },
                        self.ui_label(self.connection.label()),
                        self.tx_label(self.transaction_state)
                    )),
            );
        }
        banner.child(actions)
    }
}
