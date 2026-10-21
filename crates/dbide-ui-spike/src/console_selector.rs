use super::*;
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
impl Workbench {
    pub(super) fn console_context_bar(&self, cx: &mut Context<Self>) -> Div {
        let session = self.console();
        let route = session.route();
        let target = session.target().cloned();
        let locked = !session.can_change_target();
        let busy = session.busy();
        let weak = cx.weak_entity();
        let profiles = self.workspace.profiles.clone();
        let browse = self.configured.clone();
        let locale = self.locale;
        let connection_label = target
            .as_ref()
            .map(|p| format!("{} ▾", p.name))
            .unwrap_or_else(|| format!("{} ▾", tr(locale, Message::SelectConnection)));
        let connection = Button::new("console-connection-selector")
            .small()
            .label(connection_label)
            .disabled(locked)
            .tooltip("Pinned to this SQL console only. Other consoles and browser are unchanged.")
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.label(tr(locale, Message::ConsoleContext));
                for profile in &profiles {
                    let profile = profile.clone();
                    let weak = weak.clone();
                    let password = browse
                        .as_ref()
                        .filter(|r| {
                            dbide_profiles::StoredExecutionTarget::capture(
                                &r.profile,
                                if r.profile.engine == DatabaseEngine::Sqlite {
                                    "main"
                                } else {
                                    &r.profile.database
                                },
                            )
                            .ok()
                            .zip(
                                dbide_profiles::StoredExecutionTarget::capture(
                                    &profile,
                                    if profile.engine == DatabaseEngine::Sqlite {
                                        "main"
                                    } else {
                                        &profile.database
                                    },
                                )
                                .ok(),
                            )
                            .is_some_and(|(a, b)| {
                                a.endpoint_fingerprint == b.endpoint_fingerprint
                                    && a.profile_id == b.profile_id
                            })
                        })
                        .map(|r| r.password.clone())
                        .unwrap_or_default();
                    menu = menu.item(PopupMenuItem::new(profile.name.clone()).on_click(
                        move |_, w, cx| {
                            let _ = weak.update(cx, |s, cx| {
                                s.bind_console(
                                    route,
                                    ConnectionRequest {
                                        profile: profile.clone(),
                                        password: password.clone(),
                                    },
                                    w,
                                    cx,
                                );
                            });
                        },
                    ));
                }
                menu
            });
        let weak = cx.weak_entity();
        let databases = session.databases.clone();
        let truncated = session.databases_truncated;
        let database_target = target.clone();
        let request = session.request();
        let database_label = target
            .as_ref()
            .map(|p| {
                format!(
                    "{}: {} ▾",
                    if locale == Locale::Ko {
                        "연결 시작 DB"
                    } else {
                        "Configured database"
                    },
                    if p.engine == DatabaseEngine::Sqlite {
                        "main"
                    } else {
                        &p.database
                    }
                )
            })
            .unwrap_or_else(|| format!("{} ▾", tr(locale, Message::SelectDatabase)));
        let database = Button::new("console-database-selector")
            .small()
            .label(database_label)
            .disabled(
                target.is_none()
                    || locked
                    || target
                        .as_ref()
                        .is_some_and(|p| p.engine == DatabaseEngine::Sqlite),
            )
            .tooltip(if locale==Locale::Ko{"세션을 열 때 사용하는 데이터베이스입니다. SQL USE와 세션 설정 변경은 이 선택기에 반영되지 않습니다."}else{"Configured database used to open the session. SQL USE and session setting changes are not reflected here."})
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.label(if truncated {
                    "Advertised databases · subset limit reached"
                } else {
                    "Advertised databases"
                });
                for database in &databases {
                    let weak = weak.clone();
                    let mut request = request.clone().unwrap();
                    request.profile.database = database.name.clone();
                    menu = menu.item(
                        PopupMenuItem::new(database.name.clone())
                            .disabled(!database.accessible)
                            .on_click(move |_, w, cx| {
                                let _ = weak.update(cx, |s, cx| {
                                    s.bind_console(route, request.clone(), w, cx)
                                });
                            }),
                    );
                }
                let refresh = weak.clone();
                menu = menu.item(
                    PopupMenuItem::new(tr(locale, Message::RefreshDatabases)).on_click(
                        move |_, _, cx| {
                            let _ =
                                refresh.update(cx, |s, cx| s.discover_console_databases(route, cx));
                        },
                    ),
                );
                let manual = weak.clone();
                menu = menu.item(PopupMenuItem::new("Enter database name…").on_click(
                    move |_, _, cx| {
                        let _ = manual.update(cx, |s, cx| {
                            s.console_database_editor = Some(route);
                            cx.notify();
                        });
                    },
                ));
                let _ = &database_target;
                menu
            });
        let doc = self
            .documents
            .iter()
            .find(|d| d.model.id == self.active_document)
            .unwrap();
        let mut bar = div()
            .px_3()
            .py_2()
            .bg(rgb(0x20252d))
            .border_b_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(connection)
                    .child(database)
                    .when(
                        session.phase == SessionPhase::Disconnected
                            && target
                                .as_ref()
                                .is_some_and(|p| p.engine != DatabaseEngine::Sqlite),
                        |el| {
                            el.child(
                                div().w(px(170.)).child(
                                    Input::new(&doc.password_draft)
                                        .small()
                                        .aria_label(tr(locale, Message::Password))
                                        .disabled(busy),
                                ),
                            )
                        },
                    )
                    .child(
                        self.button("connect-console", "Connect console")
                            .disabled(
                                target.is_none()
                                    || busy
                                    || session.phase == SessionPhase::Connected,
                            )
                            .on_click(cx.listener(|s, _, _, cx| s.connect_console(cx))),
                    )
                    .child(
                        self.button("disconnect-console", "Disconnect console")
                            .disabled(!session.has_worker())
                            .on_click(
                                cx.listener(move |s, _, _, cx| s.disconnect_console(route, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("console-pinned-target")
                    .role(Role::Status)
                    .aria_label(self.console_context_label())
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child(self.console_context_label()),
            );
        if self.console_database_editor == Some(route) {
            bar = bar.child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(10.))
                            .child("Database draft · not applied"),
                    )
                    .child(
                        div().w(px(220.)).child(
                            Input::new(&doc.database_draft)
                                .small()
                                .aria_label("Database name for this console"),
                        ),
                    )
                    .child(
                        self.button("apply-console-database", "Use database")
                            .disabled(locked)
                            .on_click(cx.listener(move |s, _, w, cx| {
                                let request = s
                                    .documents
                                    .iter()
                                    .find(|d| d.session.route() == route)
                                    .and_then(|d| {
                                        d.session.request().map(|mut r| {
                                            r.profile.database =
                                                d.database_draft.read(cx).value().to_string();
                                            r
                                        })
                                    });
                                if let Some(request) = request {
                                    s.bind_console(route, request, w, cx);
                                }
                                s.console_database_editor = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        self.button("cancel-console-database", "Cancel")
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.console_database_editor = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        bar
    }
    pub(super) fn console_context_label(&self) -> String {
        let session = self.console();
        let phase = match session.phase {
            SessionPhase::Disconnected => tr(self.locale, Message::NotConnected),
            SessionPhase::Connected => tr(self.locale, Message::Connected),
            SessionPhase::Connecting => tr(self.locale, Message::Connecting),
            SessionPhase::Disconnecting => tr(self.locale, Message::Disconnecting),
            SessionPhase::Executing => {
                if self.locale == Locale::Ko {
                    "실행 중…"
                } else {
                    "Executing…"
                }
            }
            SessionPhase::Cancelling => tr(self.locale, Message::CancelRequested),
        };
        match session.target() {
            Some(profile) => format!(
                "{} {} · {} · {} · {} · {}: {}",
                tr(self.locale, Message::ConsoleContext),
                session.route().console_id,
                dbide_ui_spike::workbench_model::engine_name(profile.engine),
                dbide_ui_spike::workbench_model::profile_target(profile),
                phase,
                if self.locale == Locale::Ko {
                    "트랜잭션"
                } else {
                    "Transaction"
                },
                self.tx_label(session.transaction_state)
            ),
            None => tr(self.locale, Message::ContextUnavailable).into(),
        }
    }
    pub(super) fn discover_console_databases(
        &mut self,
        route: ConsoleRoute,
        cx: &mut Context<Self>,
    ) {
        let route = if self.console().route() == route {
            if let Err(error) = self.prepare_console_password(cx) {
                self.status = error;
                cx.notify();
                return;
            }
            self.console().route()
        } else {
            route
        };
        let slot = self
            .documents
            .iter()
            .filter(|d| d.session.has_worker())
            .count()
            < MAX_CONSOLE_WORKERS;
        let result = self
            .documents
            .iter_mut()
            .find(|d| d.model.id == route.console_id)
            .map(|d| {
                d.session
                    .discover_databases(route, slot || d.session.has_worker())
            });
        if let Some(Err(error)) = result {
            self.status = error;
        }
        cx.notify();
    }
}
