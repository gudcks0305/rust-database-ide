use super::*;
impl Workbench {
    pub(super) fn ui_label<'a>(&self, label: &'a str) -> &'a str {
        let key = match label {
            "Preferences" => Message::Preferences,
            "Connections" => Message::Connections,
            "+ New" => Message::NewConnection,
            "Edit" => Message::EditConnection,
            "Test" => Message::TestConnection,
            "Save profile" => Message::SaveConnection,
            "Remove" => Message::RemoveConnection,
            "Connect" | "Connect console" => Message::Connect,
            "Disconnect" | "Disconnect console" => Message::Disconnect,
            "Connection name" => Message::ConnectionName,
            "Host" => Message::Host,
            "Port" => Message::Port,
            "Database" => Message::Database,
            "Username" => Message::Username,
            "Password" => Message::Password,
            "SQLite database file" => Message::DatabaseFile,
            "Connection timeout (seconds)" => Message::ConnectionTimeout,
            "Close" => Message::Close,
            "New" => Message::NewConsole,
            "Open…" => Message::Open,
            "Save" => Message::Save,
            "Cancel" => Message::Cancel,
            "Run script" => Message::Run,

            "Refresh" => Message::Refresh,
            "History" | "Query history" => Message::QueryHistory,
            "Copy cell" => Message::Copy,
            "TLS mode" => Message::TlsMode,
            "Select connection" => Message::SelectConnection,
            "Select database" => Message::SelectDatabase,
            "Database IDE" => Message::AppName,
            "Local workspace" => Message::LocalWorkspace,
            "DATABASE EXPLORER" => Message::Explorer,
            "Disconnected" => Message::NotConnected,
            "Connecting…" => Message::Connecting,
            "Connected" => Message::Connected,
            "Disconnecting…" => Message::Disconnecting,
            "Query results" => Message::Results,
            _ => {
                return if self.locale == Locale::Ko {
                    match label {
                        "Run current" => "현재 구문 실행",
                        "Connect & Run" => "연결 후 실행",
                        "Remember password in Keychain on Save" => {
                            "저장할 때 Keychain에 비밀번호 기억"
                        }
                        "Load Keychain" => "Keychain에서 불러오기",
                        "Forget stored password" => "저장된 비밀번호 삭제",
                        "Read-only session default" => "읽기 전용 세션 기본값",
                        "Allow file creation on Test / Connect" => "테스트·연결 시 파일 생성 허용",
                        "Choosing a path creates nothing. With file creation enabled, Test or Connect may create a missing database file." => {
                            "경로를 선택하는 것만으로 파일을 만들지 않습니다. 파일 생성을 허용하면 테스트·연결 시 없는 DB 파일을 생성할 수 있습니다."
                        }
                        "Require and Verify full both validate the server certificate and hostname. Passwords stay in this session unless Remember is explicitly checked on Save." => {
                            "필수 TLS와 전체 검증은 모두 서버 인증서와 호스트 이름을 확인합니다. 저장할 때 기억을 선택하지 않으면 비밀번호는 이 세션에만 남습니다."
                        }
                        "SQLite opens the file in enforced read-only mode; file creation is disabled." => {
                            "SQLite 파일을 실제 읽기 전용 모드로 엽니다. 파일 생성은 사용하지 않습니다."
                        }
                        "Advisory session default. Database privileges remain the permission boundary; this is not an authorization restriction." => {
                            "세션의 권고 기본값입니다. 실제 접근 제한은 데이터베이스 권한이 결정하며, 이 설정 자체가 권한을 제한하지 않습니다."
                        }
                        "Open SQLite read-only" => "SQLite를 읽기 전용으로 열기",
                        "Disable" => "사용 안 함",
                        "Require" => "필수 TLS",
                        "Verify full" => "전체 검증",
                        "Console action not submitted" => "콘솔 요청을 제출하지 않았습니다",
                        "Query cancelled" => "쿼리가 취소되었습니다",
                        "Query failed" => "쿼리 실패",
                        "Connection failed" => "연결 실패",
                        "Metadata unavailable" => "메타데이터를 사용할 수 없습니다",
                        "Database list unavailable" => "데이터베이스 목록을 사용할 수 없습니다",
                        "Rollback this console" => "이 콘솔 롤백",
                        "Dismiss" => "안내 닫기",
                        "Retry console connection" => "이 콘솔 다시 연결",
                        "Import preview…" => "가져오기 미리보기…",
                        "Export CSV…" => "CSV 내보내기…",
                        "JSON…" => "JSON 내보내기…",
                        "Loading tables and views…" => "테이블과 뷰를 불러오는 중…",
                        "Refresh to load this database." => {
                            "새로고침을 눌러 이 데이터베이스를 탐색하세요."
                        }
                        "Connect, then Refresh to browse this database." => {
                            "연결한 후 새로고침을 눌러 데이터베이스를 탐색하세요."
                        }
                        "No matching schema, table or column." => {
                            "일치하는 스키마, 테이블, 열이 없습니다."
                        }
                        "No columns returned." => "반환된 열이 없습니다.",
                        "Console connected" => "이 콘솔에 연결되었습니다",
                        "Preparing this console's captured target…" => {
                            "이 콘솔의 연결 정보를 준비하는 중…"
                        }
                        "Preparing this console’s captured target…" => {
                            "이 콘솔의 연결 정보를 준비하는 중…"
                        }
                        "Query submitted on this console only" => "이 콘솔에서 쿼리를 실행했습니다",
                        "Format" => "SQL 서식",
                        "Connection settings" => "연결 설정",
                        "Save & connect" => "저장 후 연결",
                        "Remove profile" => "연결 삭제",
                        "Engine" => "엔진",
                        "Your results appear here" => "쿼리 결과가 여기에 표시됩니다",
                        "Query in progress" => "쿼리 실행 중…",
                        "Waiting for the database. You can cancel this execution." => {
                            "데이터베이스 응답을 기다리는 중입니다. 이 실행은 취소할 수 있습니다."
                        }
                        "Run the editor with ⌘Enter. A disconnected session reconnects first." => {
                            "⌘Enter로 실행하세요. 이 콘솔의 연결이 끊겨 있으면 먼저 연결합니다."
                        }
                        "Create a connection, then run your first query." => {
                            "이 SQL 탭의 연결과 데이터베이스를 선택한 후 실행하세요."
                        }
                        "Close this database workspace?" => "이 데이터베이스 작업 공간을 닫을까요?",
                        "Discard and close" => "버리고 닫기",
                        "SQL drafts will be retained locally without modifying source files. Open transactions are disconnected without committing. No staged row edit is applied by closing." => {
                            "SQL 초안은 원본 파일 변경 없이 로컬에 보관됩니다. 열린 트랜잭션은 커밋하지 않고 연결을 닫습니다. 행 변경은 적용하지 않습니다."
                        }
                        "Unsaved SQL and staged table edits will be discarded. Open transactions are disconnected without committing. No staged row edit is applied by closing." => {
                            "저장하지 않은 SQL과 행 편집은 버립니다. 열린 트랜잭션은 커밋하지 않고 연결을 닫습니다. 행 변경은 적용하지 않습니다."
                        }
                        "Complete" => "자동 완성",
                        "Inspect SQL" => "SQL 검사",
                        "Commit" => "커밋",
                        "Rollback" => "롤백",
                        "Auto" => "자동 커밋",
                        "Manual" => "수동 커밋",
                        "Save as…" => "다른 이름으로 저장…",
                        "Structure" => "구조",
                        "Data" => "데이터",
                        "SQL" => "SQL 초안",
                        "Saving…" => "저장 중…",
                        "CA certificate path (optional)" => "CA 인증서 경로 (선택)",
                        "Advanced" => "고급",
                        "Tools" => "데이터베이스 도구",
                        "Close tools" => "도구 닫기",
                        "Recovery: On" => "SQL 복구: 켜짐",
                        "Recovery: Off" => "SQL 복구: 꺼짐",
                        _ => label,
                    }
                } else {
                    label
                };
            }
        };
        tr(self.locale, key)
    }
    pub(super) fn tx_label(&self, wire: ServerTransactionState) -> &'static str {
        match (self.locale, wire) {
            (Locale::Ko, ServerTransactionState::Idle) => "대기",
            (Locale::Ko, ServerTransactionState::Active) => "진행",
            (Locale::Ko, ServerTransactionState::Failed) => "실패",
            (Locale::Ko, ServerTransactionState::Unknown) => "미확인",
            (_, ServerTransactionState::Idle) => "Idle",
            (_, ServerTransactionState::Active) => "Active",
            (_, ServerTransactionState::Failed) => "Failed",
            (_, ServerTransactionState::Unknown) => "Unknown",
        }
    }
    pub(super) fn active_console_status_label(&self) -> String {
        let session = self.console();
        if self.fixture_smoke {
            return self.ui_label(&self.status).to_string();
        }
        if session.target().is_none() {
            return tr(self.locale, Message::ContextUnavailable).into();
        }
        let phase = match session.phase {
            SessionPhase::Disconnected => tr(self.locale, Message::NotConnected),
            SessionPhase::Connected => tr(self.locale, Message::Connected),
            SessionPhase::Connecting => tr(self.locale, Message::Connecting),
            SessionPhase::Disconnecting => tr(self.locale, Message::Disconnecting),
            SessionPhase::Executing => {
                if self.locale == Locale::Ko {
                    "실행 중"
                } else {
                    "Executing"
                }
            }
            SessionPhase::Cancelling => tr(self.locale, Message::CancelRequested),
        };
        format!(
            "{} {} · {} · {}: {}",
            if self.locale == Locale::Ko {
                "콘솔"
            } else {
                "Console"
            },
            session.route().console_id,
            phase,
            if self.locale == Locale::Ko {
                "트랜잭션"
            } else {
                "Transaction"
            },
            self.tx_label(session.transaction_state)
        )
    }
    pub(super) fn recovery_status_label(&self) -> String {
        let ko = self.locale == Locale::Ko;
        match self.recovery.status.as_str() {
            "SQL recovery enabled · local 0600 file; original SQL files are not changed." => {
                format!(
                    "{} · {}",
                    tr(self.locale, Message::RecoveryOn),
                    if ko {
                        "로컬 0600 파일 · 원본 SQL 파일은 변경하지 않음"
                    } else {
                        "local 0600 file; original SQL files are not changed"
                    }
                )
            }
            "SQL recovery off · enable to retain local drafts across restarts." => format!(
                "{} · {}",
                tr(self.locale, Message::RecoveryOff),
                if ko {
                    "켜면 앱을 다시 열 때 로컬 초안을 복구합니다"
                } else {
                    "enable to retain local drafts across restarts"
                }
            ),
            "SQL drafts are saved locally in a 0600 file. SQL may contain sensitive values; source .sql files are not auto-saved." =>
            {
                format!(
                    "{} · {}",
                    tr(self.locale, Message::RecoveryOn),
                    if ko {
                        "로컬 0600 파일에 저장 · SQL에 민감한 값이 포함될 수 있음 · 원본 파일 자동 저장 없음"
                    } else {
                        "local 0600 file; SQL may contain sensitive values; source files are not auto-saved"
                    }
                )
            }
            "SQL recovery disabled. The prior recovery snapshot remains local and is not restored while disabled." =>
            {
                format!(
                    "{} · {}",
                    tr(self.locale, Message::RecoveryOff),
                    if ko {
                        "이전 복구 파일은 로컬에 유지되며 끈 동안 복원하지 않음"
                    } else {
                        "prior local snapshot retained; it is not restored while recovery is off"
                    }
                )
            }
            other => other.into(),
        }
    }
    pub(super) fn refresh_core_placeholders(&self, window: &mut Window, cx: &mut Context<Self>) {
        let search = tr(self.locale, Message::SearchObjects);
        if self
            .object_search
            .read(cx)
            .presentation()
            .placeholder()
            .as_ref()
            != search
        {
            self.object_search
                .update(cx, |input, cx| input.set_placeholder(search, window, cx));
        }
        let password = if self.locale == Locale::Ko {
            "세션 비밀번호"
        } else {
            "Session password"
        };
        for doc in &self.documents {
            if doc
                .password_draft
                .read(cx)
                .presentation()
                .placeholder()
                .as_ref()
                != password
            {
                doc.password_draft
                    .update(cx, |input, cx| input.set_placeholder(password, window, cx));
            }
        }
    }
    fn apply_app_settings(
        &mut self,
        settings: dbide_settings::AppSettings,
        cx: &mut Context<Self>,
    ) {
        self.locale = settings.locale;
        let dragging_layout = self.resize_drag.map(|_| {
            (
                self.app_settings.sidebar_width,
                self.app_settings.editor_height,
            )
        });
        self.app_settings = settings;
        if let Some((width, height)) = dragging_layout {
            self.app_settings.sidebar_width = width;
            self.app_settings.editor_height = height;
        }
        for doc in &self.documents {
            doc.editor.update(cx, |editor, cx| {
                editor.set_tab_size(
                    gpui_component::input::TabSize {
                        tab_size: settings.editor_tab_width as usize,
                        hard_tabs: false,
                    },
                    cx,
                )
            });
        }
        for tab in &self.table_tabs {
            tab.editor
                .update(cx, |editor, cx| editor.set_locale(settings.locale, cx));
        }
        if let Some(tools) = &self.tools {
            tools.update(cx, |panel, cx| panel.set_locale(settings.locale, cx));
        }
        if let Some(inspector) = &self.structure {
            inspector.update(cx, |panel, cx| panel.set_locale(settings.locale, cx));
        }
        if let Some(panel) = &self.project.panel {
            panel.update(cx, |panel, cx| panel.set_locale(settings.locale, cx));
        }
        if let Some(panel) = &self.result_find {
            panel.update(cx, |panel, cx| panel.set_locale(settings.locale, cx));
        }
        if let Some(panel) = &self.value_inspector {
            panel.update(cx, |panel, cx| panel.set_locale(settings.locale, cx));
        }
        if let Some(panel) = &self.stream_export {
            panel.update(cx, |panel, cx| panel.set_locale(settings.locale, cx));
        }
        if let Some(panel) = &self.object_finder {
            panel.update(cx, |panel, cx| panel.set_locale(settings.locale, cx));
        }
        self.sync_history_panel(cx);
        self.menu_state = None;
        self.settings_error = None;
        self.sync_preferences(cx);
    }
    pub(super) fn sync_preferences(&self, cx: &mut Context<Self>) {
        if let Some(panel) = &self.preferences {
            let available = !self.recovery.loading
                && !self.recovery.preference_busy
                && (!self.recovery.blocked || self.recovery.enabled);
            panel.update(cx, |panel, cx| {
                panel.set_settings(self.app_settings, cx);
                panel.set_recovery_state(self.recovery.enabled, available, cx);
            });
        }
    }
    pub(super) fn init_settings(&mut self, cx: &mut Context<Self>) {
        let path = self
            .store
            .as_ref()
            .map(|s| s.path().to_path_buf())
            .or_else(|| std::env::var_os("DBIDE_PROFILE_STORE").map(std::path::PathBuf::from))
            .or_else(|| ProfileStore::default_path().ok());
        let Some(path) = path else {
            return;
        };
        let store = dbide_settings::SettingsStore::for_profile_path(path);
        self.settings_store = Some(store.clone());
        self.settings_busy = true;
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { store.load() })
                .await;
            let _ = view.update(cx, |s, cx| {
                s.settings_busy = false;
                match result {
                    Ok(settings) => s.apply_app_settings(settings, cx),
                    Err(error) => s.settings_error = Some(error.to_string()),
                }
                s.start_settings_write(cx);
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn open_preferences_action(
        &mut self,
        _: &OpenPreferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stream_export_visible = false;
        let panel =
            cx.new(|cx| dbide_preferences_ui::PreferencesPanel::new(self.locale, window, cx));
        self._subscriptions
            .push(cx.subscribe_in(&panel, window, |s, source, event, w, cx| {
                if s.preferences
                    .as_ref()
                    .is_none_or(|p| p.entity_id() != source.entity_id())
                {
                    return;
                }
                use dbide_preferences_ui::PreferencesEvent as E;
                use dbide_settings::SettingsChange as C;
                let change = match event {
                    E::LocaleChosen(locale) => C::Locale(*locale),
                    E::FontSizeChosen(size) => C::EditorFontSize(*size),
                    E::TabWidthChosen(width) => C::EditorTabWidth(*width),
                    E::PageSizeChosen(size) => C::TablePageSize(*size),
                    E::RecoveryChosen(enabled) => {
                        if *enabled != s.recovery.enabled {
                            s.toggle_recovery(w, cx);
                        }
                        s.sync_preferences(cx);
                        return;
                    }
                };
                s.queue_settings_change(change, cx);
            }));
        self.preferences = Some(panel);
        self.sync_preferences(cx);
        cx.notify();
    }
    pub(super) fn queue_settings_change(
        &mut self,
        change: dbide_settings::SettingsChange,
        cx: &mut Context<Self>,
    ) {
        // Keep at most the newest queued mutation of each kind while I/O runs.
        self.settings_pending
            .retain(|old| std::mem::discriminant(old) != std::mem::discriminant(&change));
        self.settings_pending.push_back(change);
        self.start_settings_write(cx);
        cx.notify();
    }
    fn start_settings_write(&mut self, cx: &mut Context<Self>) {
        if self.settings_busy {
            return;
        }
        let Some(change) = self.settings_pending.pop_front() else {
            return;
        };
        let Some(store) = self.settings_store.clone() else {
            self.settings_error = Some("Settings storage unavailable".into());
            return;
        };
        self.settings_busy = true;
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { store.apply(change) })
                .await;
            let _ = view.update(cx, |s, cx| {
                s.settings_busy = false;
                match result {
                    Ok(settings) => s.apply_app_settings(settings, cx),
                    Err(error) => s.settings_error = Some(error.to_string()),
                }
                s.start_settings_write(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

impl Workbench {
    pub(super) fn preferences_panel(
        &self,
        preferences: &Entity<dbide_preferences_ui::PreferencesPanel>,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .w(px(380.))
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .border_l_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .child(
                div()
                    .p_3()
                    .flex()
                    .justify_between()
                    .child(self.ui_label("Preferences"))
                    .child(
                        self.button("close-preferences", "Close")
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.preferences = None;
                                cx.notify();
                            })),
                    ),
            )
            .when(self.settings_busy, |el| {
                el.child(
                    div()
                        .id("settings-persist-status")
                        .px_4()
                        .role(Role::Status)
                        .aria_label(self.ui_label("Saving…"))
                        .child(self.ui_label("Saving…")),
                )
            })
            .when_some(self.settings_error.as_ref(), |el, error| {
                el.child(div().px_4().text_color(rgb(0xf0b797)).child(error.clone()))
            })
            .child(div().flex_1().min_h_0().child(preferences.clone()))
    }
}
