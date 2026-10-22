use super::*;
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
impl Workbench {
    pub(super) fn document_tabs(&self, cx: &mut Context<Self>) -> Div {
        div()
            .h(px(38.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                self.button("toggle-catalog", "☰")
                    .tooltip(if self.locale == Locale::Ko {
                        "탐색기 · ⌘B"
                    } else {
                        "Catalog · ⌘B"
                    })
                    .on_click(cx.listener(|s, _, w, cx| s.toggle_catalog(&ToggleCatalog, w, cx))),
            )
            .child(
                div()
                    .id("workspace-tabs")
                    .track_scroll(&self.tab_scroll)
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .children(self.documents.iter().map(|doc| {
                        let id = doc.model.id;
                        let active = self.active_table_tab.is_none()
                            && self.tools.is_none()
                            && id == self.active_document;
                        let title = format!(
                            "{}{}",
                            doc.model.title,
                            if doc.model.dirty(&doc.editor.read(cx).value()) {
                                " ●"
                            } else {
                                ""
                            }
                        );
                        div()
                            .h(px(36.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .border_b_2()
                            .border_color(rgb(if active { ACCENT } else { PANEL }))
                            .when(active, |el| el.bg(rgb(BG)))
                            .child(
                                Button::new(("document", id as usize))
                                    .label(title)
                                    .small()
                                    .ghost()
                                    .on_click(
                                        cx.listener(move |s, _, _, cx| s.switch_document(id, cx)),
                                    ),
                            )
                            .child(
                                Button::new(("close-document", id as usize))
                                    .accessibility_label(format!(
                                        "{} {}",
                                        tr(self.locale, Message::Close),
                                        doc.model.title
                                    ))
                                    .label("×")
                                    .small()
                                    .ghost()
                                    .disabled(
                                        doc.session.busy() || doc.session.may_have_transaction(),
                                    )
                                    .on_click(
                                        cx.listener(move |s, _, w, cx| s.close_document(id, w, cx)),
                                    ),
                            )
                    }))
                    .children(self.table_tab_entries(cx)),
            )
            .child(
                div()
                    .px_2()
                    .flex()
                    .gap_1()
                    .child(
                        self.button("new-sql", "+")
                            .tooltip(if self.locale == Locale::Ko {
                                "새 SQL 콘솔 · ⌘T"
                            } else {
                                "New SQL console · ⌘T"
                            })
                            .on_click(cx.listener(|s, _, w, cx| {
                                s.new_document(String::new(), "untitled.sql".into(), true, w, cx)
                            })),
                    )
                    .child(self.file_command_menu()),
            )
    }
    pub(super) fn query_toolbar(&self, cx: &mut Context<Self>) -> Div {
        let session = self.console();
        let route = session.route();
        let wire = session.transaction_state;
        let busy = session.busy();
        let may_tx = session.may_have_transaction();
        let cancel = session.cancel_target();
        let can_run = self.can_run(cx);
        div()
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .px_3()
            .py_1()
            .flex()
            .items_center()
            .flex_wrap()
            .gap_2()
            .child(
                self.button(
                    "run",
                    if session.phase == SessionPhase::Disconnected {
                        "Connect & Run"
                    } else {
                        "Run script"
                    },
                )
                .primary()
                .tooltip("⌘Enter")
                .disabled(!can_run)
                .on_click(cx.listener(|s, _, _, cx| s.submit_query(cx))),
            )
            .child(
                self.button("run-current", "Run current")
                    .tooltip("⌘⇧Enter")
                    .disabled(!can_run)
                    .on_click(cx.listener(|s, _, w, cx| s.run_current_action(&RunCurrent, w, cx))),
            )
            .when(cancel.is_some(), |el| {
                el.child(
                    self.button("cancel", "Cancel")
                        .tooltip("⌘.")
                        .on_click(cx.listener(move |s, _, _, cx| {
                            if let Some(target) = cancel {
                                s.cancel_console(route, target, cx);
                            }
                        })),
                )
            })
            .child(self.sql_command_menu(cx))
            .child(div().flex_1())
            .when(session.phase == SessionPhase::Connected, |el| {
                el.child(self.commit_policy_menu(cx))
            })
            .when(may_tx, |el| {
                el.when(wire == ServerTransactionState::Active, |el| {
                    el.child(
                        self.button("commit-tx", "Commit")
                            .disabled(busy || session.phase != SessionPhase::Connected)
                            .on_click(cx.listener(|s, _, _, cx| s.finish_transaction(true, cx))),
                    )
                })
                .child(
                    self.button("rollback-tx", "Rollback")
                        .disabled(busy || session.phase != SessionPhase::Connected)
                        .on_click(cx.listener(|s, _, _, cx| s.finish_transaction(false, cx))),
                )
                .child(
                    div()
                        .id("wire-transaction-state")
                        .role(Role::Status)
                        .text_size(px(11.))
                        .text_color(rgb(0xd4ad78))
                        .child(format!(
                            "{}: {}",
                            tr(self.locale, Message::Transaction),
                            self.tx_label(wire)
                        )),
                )
            })
    }
    fn sql_command_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let ko = self.locale == Locale::Ko;
        let weak = cx.weak_entity();
        let metadata_locked = self.console().busy() || self.console().target().is_none();
        Button::new("sql-command-menu")
            .small()
            .ghost()
            .label(if ko {
                "SQL 도구 ▾"
            } else {
                "SQL tools ▾"
            })
            .dropdown_menu(move |menu, _, _| {
                let inspect = weak.clone();
                menu.item(
                    PopupMenuItem::new(if ko {
                        "선택 영역 실행"
                    } else {
                        "Run selection"
                    })
                    .action(Box::new(RunSelection)),
                )
                .separator()
                .item(
                    PopupMenuItem::new(if ko { "SQL 서식" } else { "Format SQL" })
                        .action(Box::new(FormatSql)),
                )
                .item(
                    PopupMenuItem::new(if ko { "자동 완성" } else { "Complete SQL" })
                        .action(Box::new(CompleteSql)),
                )
                .item(
                    PopupMenuItem::new(if ko { "SQL 검사" } else { "Inspect SQL" }).on_click(
                        move |_, _, cx| {
                            let _ = inspect.update(cx, |s, cx| s.analyze_sql(cx));
                        },
                    ),
                )
                .separator()
                .item(
                    PopupMenuItem::new(if ko {
                        "메타데이터 새로고침"
                    } else {
                        "Refresh metadata"
                    })
                    .disabled(metadata_locked)
                    .action(Box::new(RefreshCompletionMetadata)),
                )
            })
            .into_any_element()
    }
    fn commit_policy_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let session = self.console();
        let route = session.route();
        let policy = session.commit_policy;
        let locked = session.busy()
            || session.may_have_transaction()
            || session.phase != SessionPhase::Connected;
        let weak = cx.weak_entity();
        let ko = self.locale == Locale::Ko;
        let label = match (ko, policy) {
            (true, CommitPolicy::Auto) => "자동 커밋 ▾",
            (true, CommitPolicy::Manual) => "수동 커밋 ▾",
            (false, CommitPolicy::Auto) => "Auto commit ▾",
            (false, CommitPolicy::Manual) => "Manual commit ▾",
        };
        Button::new("commit-policy-menu")
            .tooltip(if locked {
                if ko {
                    "진행 중인 작업과 트랜잭션을 마친 뒤 변경하세요."
                } else {
                    "Finish the current operation and transaction before changing policy."
                }
            } else {
                if ko {
                    "이 SQL 콘솔의 커밋 방식"
                } else {
                    "Commit policy for this SQL console"
                }
            })
            .small()
            .ghost()
            .label(label)
            .disabled(locked)
            .dropdown_menu(move |menu, _, _| {
                let auto = weak.clone();
                let manual = weak.clone();
                menu.item(
                    PopupMenuItem::new(if ko { "자동 커밋" } else { "Auto commit" })
                        .checked(policy == CommitPolicy::Auto)
                        .disabled(locked)
                        .on_click(move |_, _, cx| {
                            let _ = auto.update(cx, |s, cx| {
                                if s.console().route() == route {
                                    s.change_policy(CommitPolicy::Auto, cx);
                                }
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new(if ko { "수동 커밋" } else { "Manual commit" })
                        .checked(policy == CommitPolicy::Manual)
                        .disabled(locked)
                        .on_click(move |_, _, cx| {
                            let _ = manual.update(cx, |s, cx| {
                                if s.console().route() == route {
                                    s.change_policy(CommitPolicy::Manual, cx);
                                }
                            });
                        }),
                )
            })
            .into_any_element()
    }
    fn file_command_menu(&self) -> AnyElement {
        let locale = self.locale;
        let active = self.sql_view_active();
        Button::new("file-command-menu")
            .small()
            .ghost()
            .label(if locale == Locale::Ko {
                "파일 ▾"
            } else {
                "File ▾"
            })
            .dropdown_menu(move |menu, _, _| {
                menu.item(
                    PopupMenuItem::new(tr(locale, Message::Open)).action(Box::new(OpenSqlFile)),
                )
                .item(
                    PopupMenuItem::new(tr(locale, Message::Save))
                        .disabled(!active)
                        .action(Box::new(SaveSqlFile)),
                )
                .item(
                    PopupMenuItem::new(tr(locale, Message::SaveAs))
                        .disabled(!active)
                        .action(Box::new(SaveSqlAs)),
                )
            })
            .into_any_element()
    }
    pub(super) fn sidebar_tools_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let ko = self.locale == Locale::Ko;
        let cache_bytes = self.navigator.stats().accounted_bytes;
        let weak = cx.weak_entity();
        let locked = self.configuration_operation.is_some() || self.pending_table_open.is_some();
        Button::new("workspace-tools-menu")
            .small()
            .ghost()
            .label(if ko { "도구 ▾" } else { "Tools ▾" })
            .dropdown_menu(move |menu, _, _| {
                let tools = weak.clone();
                let diagnostics = weak.clone();
                let cache = weak.clone();
                menu.item(
                    PopupMenuItem::new(if ko {
                        "객체 찾기…"
                    } else {
                        "Find object…"
                    })
                    .action(Box::new(FindDatabaseObject)),
                )
                .separator()
                .item(
                    PopupMenuItem::new(if ko {
                        "데이터베이스 도구"
                    } else {
                        "Database tools"
                    })
                    .disabled(locked)
                    .on_click(move |_, w, cx| {
                        let _ = tools.update(cx, |s, cx| s.open_tools(w, cx));
                    }),
                )
                .separator()
                .label(format!(
                    "{} · {:.1} MiB",
                    if ko {
                        "메모리 캐시"
                    } else {
                        "Memory cache"
                    },
                    cache_bytes as f64 / 1048576.
                ))
                .item(
                    PopupMenuItem::new(if ko {
                        "메타데이터 캐시 비우기"
                    } else {
                        "Clear metadata cache"
                    })
                    .on_click(move |_, _, cx| {
                        let _ = cache.update(cx, |s, cx| {
                            s.navigator.clear();
                            cx.notify();
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new(if ko {
                        "성능 진단"
                    } else {
                        "Performance diagnostics"
                    })
                    .on_click(move |_, _, cx| {
                        let _ = diagnostics.update(cx, |s, cx| {
                            s.show_tools = !s.show_tools;
                            cx.notify();
                        });
                    }),
                )
                .separator()
                .item(
                    PopupMenuItem::new(tr(
                        if ko { Locale::Ko } else { Locale::En },
                        Message::Preferences,
                    ))
                    .action(Box::new(OpenPreferences)),
                )
            })
            .into_any_element()
    }
    pub(super) fn sql_tools_panel(&self, cx: &mut Context<Self>) -> Div {
        let revision = self.current_revision();
        div().w(px(350.)).flex_shrink_0().bg(rgb(PANEL)).border_l_1().border_color(rgb(BORDER)).flex().flex_col()
            .child(div().p_4().flex().items_center().justify_between().child(div().font_weight(FontWeight::SEMIBOLD).child("SQL assistance"))
                .child(self.button("close-sql-tools","Close").on_click(cx.listener(|s,_,_,cx|{s.show_sql_tools=false;s.completions.clear();cx.notify();}))))
            .child(div().id("sql-tools-scroll").flex_1().min_h_0().overflow_y_scroll().px_4().flex().flex_col().gap_3()
                .child(div().text_size(px(10.)).text_color(rgb(MUTED)).child(format!("{} · tab {} · revision {} · {} catalog tables",dbide_ui_spike::workbench_model::engine_name(self.engine()),self.active_document,revision,self.console().schema.len())))
                .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(self.console().schema.iter().take(12).map(|o|format!("{}.{}",o.schema,o.name)).collect::<Vec<_>>().join(" · ")))
                .child(div().font_weight(FontWeight::MEDIUM).child("Completions · ⌘Space"))
                .when(self.completions.is_empty(),|el|el.child(div().text_color(rgb(MUTED)).text_size(px(11.)).child("Place the cursor in SQL and request completion. Refresh database objects to supply table and column names.")))
                .children(self.completions.iter().take(100).enumerate().map(|(ix,c)|{
                    Button::new(("completion",ix)).label(format!("{} · {:?}",c.label,c.kind)).small().on_click(cx.listener(move|s,_,w,cx|s.apply_completion(ix,w,cx)))
                }))
                .child(div().h(px(1.)).bg(rgb(BORDER)))
                .child(div().font_weight(FontWeight::MEDIUM).child("Diagnostics"))
                .when(self.sql_diagnostics.is_empty(),|el|el.child(div().text_size(px(11.)).text_color(rgb(MUTED)).child("Inspect SQL to check supported syntax and catalog bindings. Analysis never gates Run script.")))
                .children(self.sql_diagnostics.iter().enumerate().map(|(ix,d)|{
                    let start=d.range.start;
                    div().id(("sql-diagnostic",ix)).role(Role::Status).text_size(px(11.)).text_color(rgb(0xf0b797)).child(format!("{:?} · bytes {}–{} · {}",d.kind,start,d.range.end,d.message))
                }))
                .child(div().h(px(1.)).bg(rgb(BORDER)))
                .child(div().font_weight(FontWeight::MEDIUM).child("Rename alias / CTE"))
                .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child("Cursor must identify a uniquely bound document-local alias or CTE. This never renames database objects."))
                .child(Input::new(&self.rename_name).aria_label("New alias or CTE name").small())
                .child(div().flex().gap_2()
                    .child(self.button("preview-rename","Preview").on_click(cx.listener(|s,_,_,cx|s.preview_rename(cx))))
                    .child(self.button("apply-rename","Apply preview").disabled(self.rename_preview.is_none()).on_click(cx.listener(|s,_,w,cx|s.apply_rename(w,cx)))))
                .when_some(self.rename_preview.as_ref(),|el,owned|el.child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(format!("Preview tab {} / revision {} · {} edits · source hash checked on Apply",owned.document_id,owned.preview.revision,owned.preview.edits.len())))
                    .children(owned.preview.edits.iter().enumerate().map(|(ix,edit)|div().id(("rename-edit",ix)).font_family("Menlo").text_size(px(10.)).child(format!("{}..{} → {}",edit.range.start,edit.range.end,edit.replacement)))))
                .child(div().h(px(12.))))
    }
    pub(super) fn transfer_panel(
        &self,
        preview: &dbide_data::DataPreview,
        cx: &mut Context<Self>,
    ) -> Div {
        div().w(px(350.)).flex_shrink_0().bg(rgb(PANEL)).border_l_1().border_color(rgb(BORDER)).flex().flex_col()
            .child(div().p_4().flex().items_center().justify_between().child(div().font_weight(FontWeight::SEMIBOLD).child("Import preview"))
                .child(self.button("close-import","Close").on_click(cx.listener(|s,_,_,cx|{s.transfer_preview=None;cx.notify();}))))
            .child(div().px_4().text_color(rgb(ACCENT)).child("Preview only · no database writes"))
            .child(div().px_4().py_3().text_color(rgb(MUTED)).child(format!("{} rows · {} columns{}",preview.data.rows.len(),preview.data.columns.len(),if preview.truncated{" · preview truncated"}else{""})))
            .child(div().px_4().text_size(px(11.)).text_color(rgb(MUTED)).child("CSV uses explicit \\N as NULL; JSON preserves typed values. First 20 rows shown below."))
            .child(div().id("import-preview-data").flex_1().min_h_0().overflow_y_scroll().p_4().flex().flex_col().gap_2()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(preview.data.columns.iter().map(|c|c.name.clone()).collect::<Vec<_>>().join(" | ")))
                .children(preview.data.rows.iter().take(20).enumerate().map(|(ix,row)|div().id(("import-row",ix)).font_family("Menlo").text_size(px(10.)).child(row.iter().map(|c|format!("{c:?}")).collect::<Vec<_>>().join(" | ")))))
    }
}
impl Workbench {
    pub(super) fn completion_popup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ko = self.locale == Locale::Ko;
        let first = self.completion_selected.saturating_sub(3);
        let freshness = self.completion_freshness_label();
        let panel =
            div()
                .id("completion-popup")
                .role(Role::ListBox)
                .aria_label(if ko {
                    "SQL 자동 완성 후보"
                } else {
                    "SQL completion candidates"
                })
                .w(px(430.))
                .bg(rgb(PANEL))
                .border_1()
                .border_color(rgb(0x465675))
                .rounded_md()
                .shadow_lg()
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .items_center()
                        .child(
                            div()
                                .text_color(rgb(MUTED))
                                .text_size(px(10.))
                                .child(if ko {
                                    "SQL 자동 완성 · ↑↓ 선택 · Enter / Tab 적용 · Esc 닫기"
                                } else {
                                    "SQL completion · ↑↓ select · Enter / Tab accept · Esc dismiss"
                                }),
                        )
                        .child(
                            self.button("dismiss-completions", "×")
                                .on_click(cx.listener(|s, _, _, cx| {
                                    s.dismiss_completion(cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .text_size(px(10.))
                        .child(format!(
                            "{} · {} / {}",
                            freshness,
                            self.completion_selected + 1,
                            self.completions.len()
                        )),
                )
                .children(self.completions.iter().enumerate().skip(first).take(6).map(
                    |(ix, c)| {
                        let kind = match (&c.kind, ko) {
                            (dbide_sql::CompletionKind::Keyword, true) => "키워드",
                            (dbide_sql::CompletionKind::Schema, true) => "스키마",
                            (dbide_sql::CompletionKind::Table, true) => "테이블",
                            (dbide_sql::CompletionKind::Column, true) => "컬럼",
                            (dbide_sql::CompletionKind::Keyword, false) => "Keyword",
                            (dbide_sql::CompletionKind::Schema, false) => "Schema",
                            (dbide_sql::CompletionKind::Table, false) => "Table",
                            (dbide_sql::CompletionKind::Column, false) => "Column",
                        };
                        div()
                            .id(("completion-option", ix))
                            .role(Role::ListBoxOption)
                            .aria_selected(ix == self.completion_selected)
                            .child(
                                Button::new(("popup-completion", ix))
                                    .label(format!("{} · {}", c.label, kind))
                                    .small()
                                    .when(ix == self.completion_selected, |b| b.primary())
                                    .on_click(cx.listener(move |s, _, w, cx| {
                                        s.apply_completion(ix, w, cx)
                                    })),
                            )
                    },
                ));
        deferred(completion_model::CaretPopup::new(
            self.editor.downgrade(),
            panel,
        ))
        .with_priority(2)
    }
}
impl Workbench {
    pub(super) fn connection_form_panel(&self, cx: &mut Context<Self>) -> Div {
        let form = self.form.as_ref().expect("form panel requires form");
        let busy = self.busy() || self.credential_busy || self.profile_write_busy;
        let sqlite = form.engine == DatabaseEngine::Sqlite;
        let editing_saved = self
            .configured
            .as_ref()
            .is_some_and(|r| r.profile.id == form.original.id);
        let mut fields = div()
            .id("profile-fields")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_5()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(self.ui_label("Engine")),
            )
            .child(
                div().flex().gap_1().children(
                    [
                        (DatabaseEngine::PostgreSql, "PostgreSQL"),
                        (DatabaseEngine::Sqlite, "SQLite"),
                        (DatabaseEngine::MySql, "MySQL"),
                    ]
                    .into_iter()
                    .map(|(engine, label)| {
                        self.button(label, label)
                            .when(form.engine == engine, |b| b.primary())
                            .disabled(busy)
                            .on_click(cx.listener(move |s, _, w, cx| {
                                if let Some(f) = &mut s.form {
                                    f.set_engine(engine, w, cx);
                                }
                                s.form_error = None;
                                cx.notify();
                            }))
                    }),
                ),
            )
            .child(form.field(0, self.ui_label("Connection name"), busy));
        if sqlite {
            fields=fields.child(form.field(8,self.ui_label("SQLite database file"),busy))
                .child(div().flex().gap_2()
                    .child(self.button("choose-sqlite-file","Open file…").disabled(busy).on_click(cx.listener(|s,_,w,cx|s.choose_sqlite_path(false,w,cx))))
                    .child(self.button("choose-new-sqlite","New path…").disabled(busy).on_click(cx.listener(|s,_,w,cx|s.choose_sqlite_path(true,w,cx)))))
                .child(Checkbox::new("create-sqlite").label(self.ui_label("Allow file creation on Test / Connect")).checked(form.sqlite_create).disabled(busy||form.read_only).on_click(cx.listener(|s,value,_,cx|{if let Some(f)=&mut s.form {f.sqlite_create = *value;}cx.notify();})))
                .child(div().text_size(px(10.)).text_color(rgb(MUTED)).child(self.ui_label("Choosing a path creates nothing. With file creation enabled, Test or Connect may create a missing database file.")));
        } else {
            fields=fields.child(div().flex().gap_3().child(div().flex_1().min_w_0().child(form.field(1,self.ui_label("Host"),busy))).child(div().w(px(76.)).child(form.field(2,self.ui_label("Port"),busy))))
                .child(form.field(3,self.ui_label("Database"),busy)).child(form.field(4,self.ui_label("Username"),busy)).child(form.field(5,self.ui_label("Password"),busy))
                .child(Checkbox::new("remember-password").label(self.ui_label("Remember password in Keychain on Save")).checked(form.remember_password).disabled(busy).on_click(cx.listener(|s,value,_,cx|{if let Some(f)=&mut s.form {f.remember_password = *value;}cx.notify();})))
                .child(div().flex().gap_2()
                    .child(self.button("load-password","Load Keychain").disabled(busy||self.credential_busy).on_click(cx.listener(|s,_,w,cx|s.load_credential(false,w,cx))))
                    .child(self.button("forget-password","Forget stored password").disabled(busy||self.credential_busy).on_click(cx.listener(|s,_,w,cx|s.load_credential(true,w,cx)))))
                .child(div().flex().flex_col().gap_1().child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(self.ui_label("TLS mode")))
                    .child(div().flex().gap_1().children([(TlsMode::Disable,"Disable"),(TlsMode::Require,"Require"),(TlsMode::VerifyFull,"Verify full")].into_iter().map(|(tls,label)|{
                        self.button(label,label).when(form.tls==tls,|b|b.primary()).disabled(busy).on_click(cx.listener(move|s,_,_,cx|{if let Some(f)=&mut s.form{f.tls=tls;}cx.notify();}))
                    }))))
                .when(form.tls!=TlsMode::Disable,|el|el.child(form.field(7,self.ui_label("CA certificate path (optional)"),busy)))
                .child(div().text_size(px(10.)).text_color(rgb(MUTED)).child(self.ui_label("Require and Verify full both validate the server certificate and hostname. Passwords stay in this session unless Remember is explicitly checked on Save.")));
        }
        fields=fields.child(form.field(6,self.ui_label("Connection timeout (seconds)"),busy))
            .child(Checkbox::new("read-only-profile").label(self.ui_label(if sqlite{"Open SQLite read-only"}else{"Read-only session default"})).checked(form.read_only).disabled(busy).on_click(cx.listener(|s,value,_,cx|{if let Some(f)=&mut s.form{f.read_only = *value;if *value&&f.engine==DatabaseEngine::Sqlite{f.sqlite_create=false;}}cx.notify();})))
            .child(div().text_size(px(10.)).text_color(rgb(MUTED)).child(self.ui_label(if sqlite{"SQLite opens the file in enforced read-only mode; file creation is disabled."}else{"Advisory session default. Database privileges remain the permission boundary; this is not an authorization restriction."})))
            .when_some(self.form_error.clone(),|el,error|el.child(div().id("form-feedback").role(Role::Status).aria_label(error.clone()).text_color(rgb(0xf0b797)).child(error)));
        let footer = div()
            .px_5()
            .py_4()
            .border_t_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        self.button("test-profile", "Test")
                            .disabled(busy || self.credential_busy)
                            .on_click(cx.listener(|s, _, _, cx| s.test_profile(cx))),
                    )
                    .child(
                        self.button("save-profile", "Save")
                            .disabled(busy || self.credential_busy)
                            .on_click(cx.listener(|s, _, _, cx| s.save_profile(false, cx))),
                    )
                    .child(
                        self.button("save-connect", "Save & connect")
                            .primary()
                            .disabled(
                                busy || self.credential_busy || self.transaction_may_be_open(),
                            )
                            .on_click(cx.listener(|s, _, _, cx| s.save_profile(true, cx))),
                    ),
            )
            .when(editing_saved, |el| {
                el.child(
                    self.button("delete-profile", "Remove profile")
                        .disabled(busy || self.transaction_may_be_open())
                        .on_click(cx.listener(|s, _, _, cx| s.remove_profile(cx))),
                )
            });
        div()
            .w(px(350.))
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .border_l_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .child(
                div()
                    .px_5()
                    .py_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.ui_label("Connection settings")),
                    )
                    .child(self.button("close-form", "Close").disabled(busy).on_click(
                        cx.listener(|s, _, _, cx| {
                            s.form = None;
                            cx.notify();
                        }),
                    )),
            )
            .child(fields)
            .child(footer)
    }
    pub(super) fn captured_tools_view(
        &self,
        tools: &Entity<dbide_tools_ui::ToolsPanel>,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family(".AppleSystemUIFont")
            .text_size(px(12.))
            .on_action(cx.listener(Self::quit_action))
            .on_action(cx.listener(Self::report_metrics))
            .on_action(cx.listener(Self::close_sql_action))
            .child(
                div()
                    .h(px(48.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().flex_1().min_w_0().text_ellipsis().child(format!(
                        "{} · {}",
                        self.ui_label("Tools"),
                        self.tools_label
                    )))
                    .child(self.button("close-database-tools", "Close tools").on_click(
                        cx.listener(|s, _, _, cx| {
                            s.tools = None;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().flex_1().min_w_0().child(tools.clone())),
            )
    }
}
impl Workbench {
    pub(super) fn diagnostic_controls(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.busy();
        let context = self
            .diagnostic
            .as_ref()
            .map_or(DiagnosticContext::Other, |d| d.context);
        let controls = div().flex().gap_2();
        match context {
            DiagnosticContext::Connection => controls
                .child(
                    self.button("retry", "Retry connection")
                        .disabled(busy || self.configured.is_none())
                        .on_click(cx.listener(|s, _, _, cx| s.connect(cx))),
                )
                .child(
                    self.button("test-diagnostic", "Test connection")
                        .disabled(busy || self.configured.is_none())
                        .on_click(cx.listener(|s, _, _, cx| s.test_profile(cx))),
                )
                .child(
                    self.button("edit-diagnostic", "Edit settings")
                        .disabled(busy)
                        .on_click(cx.listener(|s, _, w, cx| s.edit_profile(false, w, cx))),
                ),
            DiagnosticContext::Schema => controls.child(
                self.button("retry-schema", "Retry schema refresh")
                    .disabled(busy || self.connection != ConnectionState::Connected)
                    .on_click(cx.listener(|s, _, _, cx| s.refresh_schema(cx))),
            ),
            DiagnosticContext::Query => {
                let owner = self
                    .diagnostic
                    .as_ref()
                    .and_then(|d| d.owner.as_ref())
                    .map(|o| o.document_id);
                controls
                    .child(
                        self.button("show-failed-sql", "Show SQL tab")
                            .disabled(owner.is_none())
                            .on_click(cx.listener(move |s, _, _, cx| {
                                if let Some(id) = owner {
                                    s.switch_document(id, cx);
                                }
                            })),
                    )
                    .child(
                        self.button("rollback-failed-sql", "Rollback")
                            .disabled(busy || !self.transaction_may_be_open())
                            .on_click(cx.listener(|s, _, _, cx| s.finish_transaction(false, cx))),
                    )
            }
            DiagnosticContext::Other | DiagnosticContext::Capacity => controls.child(
                self.button("dismiss-diagnostic", "Dismiss")
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.diagnostic = None;
                        cx.notify();
                    })),
            ),
        }
    }
}
