use super::*;
impl Workbench {
    pub(super) fn document_tabs(&self, cx: &mut Context<Self>) -> Div {
        div().h(px(42.)).flex_shrink_0().flex().items_center().bg(rgb(PANEL)).border_b_1().border_color(rgb(BORDER))
        .child(div().id("sql-tabs").flex_1().min_w_0().overflow_x_scroll().flex().gap_1().px_2().children(self.documents.iter().map(|doc|{
            let id=doc.model.id;let title=format!("{}{}",doc.model.title,if doc.model.dirty(&doc.editor.read(cx).value()){" ●"}else{""});
            div().flex().items_center().border_b_2().border_color(rgb(if id==self.active_document{ACCENT}else{PANEL}))
                .child(Button::new(("document",id as usize)).label(title).small().when(id==self.active_document,|b|b.primary()).on_click(cx.listener(move|s,_,_,cx|s.switch_document(id,cx))))
                .child(Button::new(("close-document",id as usize)).label("×").small().disabled(doc.session.busy()||doc.session.may_have_transaction()).on_click(cx.listener(move|s,_,w,cx|s.close_document(id,w,cx))))
        })))
        .child(div().px_2().flex().gap_1()
            .child(self.button("new-sql","New").on_click(cx.listener(|s,_,w,cx|s.new_document(String::new(),"untitled.sql".into(),true,w,cx))))
            .child(self.button("open-sql","Open…").on_click(cx.listener(|s,_,w,cx|s.open_sql(w,cx))))
            .child(self.button("save-current-sql","Save").on_click(cx.listener(|s,_,w,cx|s.save_document(s.active_document,false,w,cx))))
            .child(self.button("save-sql","Save as…").on_click(cx.listener(|s,_,w,cx|s.save_document_as(s.active_document,false,w,cx))))
            .child(self.button("sql-recovery",if self.recovery.enabled{"Recovery: On"}else{"Recovery: Off"})
                .tooltip("SQL recovery saves local drafts (may contain sensitive values) in a private file. It never auto-saves source .sql files, connects, or runs SQL.")
                .disabled(self.recovery.loading||self.recovery.preference_busy||(self.recovery.blocked&&!self.recovery.enabled)).on_click(cx.listener(|s,_,w,cx|s.toggle_recovery(w,cx)))))
    }
    pub(super) fn query_toolbar(&self, cx: &mut Context<Self>) -> Div {
        let session = self.console();
        let busy = session.busy();
        let phase = session.phase;
        let policy = session.commit_policy;
        let wire = session.transaction_state;
        let may_tx = session.may_have_transaction();
        let route = session.route();
        let can_run = self.can_run(cx);
        let cancel = session.cancel_target();
        div()
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        self.button(
                            "run",
                            if phase == SessionPhase::Disconnected {
                                "Connect & Run"
                            } else {
                                "Run script"
                            },
                        )
                        .primary()
                        .disabled(!can_run)
                        .on_click(cx.listener(|s, _, _, cx| s.submit_query(cx))),
                    )
                    .child(
                        self.button("run-current", "Run current")
                            .disabled(!can_run)
                            .on_click(
                                cx.listener(|s, _, w, cx| s.run_current_action(&RunCurrent, w, cx)),
                            ),
                    )
                    .child(
                        self.button("cancel", "Cancel")
                            .disabled(session.active_operation.is_none())
                            .on_click(cx.listener(move |s, _, _, cx| {
                                if let Some(target) = cancel {
                                    s.cancel_console(route, target, cx);
                                }
                            })),
                    )
                    .child(div().w(px(1.)).h(px(20.)).bg(rgb(BORDER)))
                    .child(self.button("format-sql", "Format").on_click(
                        cx.listener(|s, _, w, cx| s.format_sql_action(&FormatSql, w, cx)),
                    ))
                    .child(self.button("complete-sql", "Complete").on_click(
                        cx.listener(|s, _, w, cx| s.complete_sql_action(&CompleteSql, w, cx)),
                    ))
                    .child(
                        self.button("analyze-sql", "Inspect SQL")
                            .on_click(cx.listener(|s, _, _, cx| s.analyze_sql(cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        self.button("auto-policy", "Auto")
                            .when(policy == CommitPolicy::Auto, |b| b.primary())
                            .disabled(busy || may_tx || phase != SessionPhase::Connected)
                            .on_click(
                                cx.listener(|s, _, _, cx| s.change_policy(CommitPolicy::Auto, cx)),
                            ),
                    )
                    .child(
                        self.button("manual-policy", "Manual")
                            .when(policy == CommitPolicy::Manual, |b| b.primary())
                            .disabled(busy || may_tx || phase != SessionPhase::Connected)
                            .on_click(
                                cx.listener(|s, _, _, cx| {
                                    s.change_policy(CommitPolicy::Manual, cx)
                                }),
                            ),
                    )
                    .child(
                        self.button("commit-tx", "Commit")
                            .disabled(
                                busy || phase != SessionPhase::Connected
                                    || wire != ServerTransactionState::Active,
                            )
                            .on_click(cx.listener(|s, _, _, cx| s.finish_transaction(true, cx))),
                    )
                    .child(
                        self.button("rollback-tx", "Rollback")
                            .disabled(busy || phase != SessionPhase::Connected || !may_tx)
                            .on_click(cx.listener(|s, _, _, cx| s.finish_transaction(false, cx))),
                    )
                    .child(
                        div()
                            .id("wire-transaction-state")
                            .role(Role::Status)
                            .aria_label(format!(
                                "{}: {}",
                                if self.locale == Locale::Ko {
                                    "현재 트랜잭션"
                                } else {
                                    "Current transaction"
                                },
                                self.tx_label(wire)
                            ))
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child(format!(
                                "{}: {}",
                                if self.locale == Locale::Ko {
                                    "현재 트랜잭션"
                                } else {
                                    "Transaction"
                                },
                                self.tx_label(wire)
                            )),
                    )
                    .child(
                        div()
                            .id("document-connection-binding")
                            .role(Role::Label)
                            .aria_label(self.document_connection_label())
                            .ml_2()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .text_size(px(10.))
                            .text_color(rgb(if self.document_profile_matches() {
                                MUTED
                            } else {
                                0xf0b797
                            }))
                            .child(self.document_connection_label()),
                    ),
            )
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
    pub(super) fn completion_popup(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("completion-popup")
            .role(Role::ListBox)
            .aria_label("SQL completion candidates")
            .absolute()
            .top(px(40.))
            .left(px(48.))
            .w(px(350.))
            .max_h(px(168.))
            .overflow_y_scroll()
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
                            .child("SQL completion · catalog / scope"),
                    )
                    .child(
                        self.button("dismiss-completions", "×")
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.completions.clear();
                                cx.notify();
                            })),
                    ),
            )
            .children(self.completions.iter().take(20).enumerate().map(|(ix, c)| {
                Button::new(("popup-completion", ix))
                    .label(format!("{} · {:?}", c.label, c.kind))
                    .small()
                    .on_click(cx.listener(move |s, _, w, cx| s.apply_completion(ix, w, cx)))
            }))
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
                    .child(div().flex_1().min_w_0().child(tools.clone()))
                    .when_some(self.preferences.as_ref(), |el, prefs| {
                        el.child(self.preferences_panel(prefs, cx))
                    }),
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
