use super::*;
use dbide_ui_spike::navigator::{Relation, Scope};
use gpui_component::{WindowExt, notification::Notification};
use std::collections::BTreeMap;
impl Workbench {
    pub(super) fn cancel_object_reveal(&mut self) {
        if let Some((key, _)) = self.pending_object_reveal.take() {
            self.navigator.collapse(&key);
        }
    }
    pub(super) fn run_selection_action(
        &mut self,
        _: &RunSelection,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sql_view_active() {
            return;
        }
        let editor = self.editor.read(cx);
        let source = editor.value();
        let Some(sql) =
            dbide_ui_spike::selection_execution::selected_sql(&source, editor.selected_range())
                .map(str::to_owned)
        else {
            w.push_notification(
                Notification::info(if self.locale == Locale::Ko {
                    "실행할 SQL을 먼저 선택하세요. 다른 문장은 실행하지 않았습니다."
                } else {
                    "Select SQL to execute. No surrounding statements were submitted."
                }),
                cx,
            );
            return;
        };
        self.submit_sql(sql, cx);
    }
    pub(super) fn find_database_object(
        &mut self,
        _: &FindDatabaseObject,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self
            .store
            .as_ref()
            .and_then(|s| s.path().parent())
            .map(|p| p.join("catalog-cache"))
        else {
            return;
        };
        let profiles = self
            .workspace
            .profiles
            .iter()
            .filter_map(|p| {
                let target = dbide_profiles::StoredExecutionTarget::capture(
                    p,
                    if p.engine == DatabaseEngine::Sqlite {
                        "main"
                    } else {
                        &p.database
                    },
                )
                .ok()?;
                Some(dbide_catalog_store::SearchProfile {
                    profile_id: p.id.clone(),
                    fingerprint: target.endpoint_fingerprint,
                    profile_name: p.name.clone(),
                    engine: dbide_ui_spike::workbench_model::engine_name(p.engine).into(),
                    database: None,
                })
            })
            .collect();
        let panel =
            cx.new(|cx| object_finder::ObjectFinder::new(root, profiles, self.locale, w, cx));
        self.finder_subscription = Some(cx.subscribe_in(&panel, w, |s, source, event, w, cx| {
            if s.object_finder
                .as_ref()
                .is_none_or(|p| p.entity_id() != source.entity_id())
            {
                return;
            }
            match event {
                object_finder::FinderEvent::Close => {
                    s.object_finder = None;
                    cx.notify();
                }
                object_finder::FinderEvent::Selected(found) => s.reveal_found_object(found, w, cx),
            }
        }));
        self.object_finder = Some(panel);
        self.stream_export_visible = false;
        self.preferences = None;
        self.value_inspector = None;
        self.range_copy = None;
        self.show_history = false;
        self.cancel_object_reveal();
        self.form = None;
        self.dismiss_completion(cx);
        cx.notify();
    }
    fn reveal_found_object(
        &mut self,
        found: &dbide_catalog_store::SearchMatch,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigator.sync_profiles(&self.workspace.profiles);
        let Some(root) = self
            .navigator
            .root(&found.profile.profile_id)
            .filter(|k| k.fingerprint == found.profile.fingerprint)
        else {
            w.push_notification(
                Notification::warning(if self.locale == Locale::Ko {
                    "연결 정보가 변경되었습니다. 검색을 다시 여세요."
                } else {
                    "Connection changed. Reopen object search."
                }),
                cx,
            );
            return;
        };
        let relation = Relation {
            schema: found.relation.schema.clone(),
            name: found.relation.name.clone(),
            kind: found.relation.kind.clone(),
        };
        let key = root.child(&found.database, Scope::Columns(relation.clone()));
        if self.navigator_request(&key).is_none() {
            return;
        }
        self.object_finder = None;
        self.structure = None;
        self.form = None;
        self.show_sql_tools = false;
        self.transfer_preview = None;
        self.catalog_collapsed = false;
        self.pending_object_reveal = Some((key.clone(), relation.clone()));
        self.navigator_expand(root.clone(), false, cx);
        self.navigator_expand(root.child(&found.database, Scope::Schemas), false, cx);
        self.navigator_expand(
            root.child(&found.database, Scope::Objects(relation.schema.clone())),
            false,
            cx,
        );
        self.navigator_expand(key, false, cx);
        self.finish_object_reveal(cx);
        cx.notify();
    }
    pub(super) fn finish_object_reveal(&mut self, cx: &mut Context<Self>) {
        let Some((key, relation)) = self.pending_object_reveal.clone() else {
            return;
        };
        if !self.navigator.authorized(&key) {
            self.pending_object_reveal = None;
            self.navigator.notice = Some(
                if self.locale == Locale::Ko {
                    "연결이 변경되어 객체 열기를 취소했습니다."
                } else {
                    "Object opening cancelled because its connection changed."
                }
                .into(),
            );
            return;
        }
        if let Some(branch) = self.navigator.branch(&key) {
            if let Some(error) = branch.error.clone() {
                self.pending_object_reveal = None;
                self.navigator.notice = Some(error);
                cx.notify();
            } else if branch.snapshot.is_some() {
                self.pending_object_reveal = None;
                self.open_branch_structure(&key, &relation, cx);
            }
        }
    }
    pub(super) fn object_reveal_panel(&self, cx: &mut Context<Self>) -> Div {
        let (_, relation) = self.pending_object_reveal.as_ref().expect("pending reveal");
        div()
            .w(px(420.))
            .flex_shrink_0()
            .p_4()
            .bg(rgb(PANEL))
            .flex()
            .flex_col()
            .gap_3()
            .child(if self.locale == Locale::Ko {
                "객체 정보 불러오는 중…"
            } else {
                "Loading object metadata…"
            })
            .child(format!("{}.{}", relation.schema, relation.name))
            .child(
                self.button("cancel-object-reveal", "Cancel")
                    .on_click(cx.listener(|s, _, _, cx| {
                        if let Some((key, _)) = s.pending_object_reveal.take() {
                            s.navigator.collapse(&key);
                        }
                        cx.notify();
                    })),
            )
    }
    fn history_profile_names(&self) -> BTreeMap<String, String> {
        self.workspace
            .profiles
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect()
    }
    pub(super) fn open_query_history(
        &mut self,
        _: &OpenQueryHistory,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show_history {
            self.show_history = false;
            cx.notify();
            return;
        }
        let panel = cx.new(|cx| {
            history_panel::HistoryPanel::new(
                self.locale,
                self.workspace.history.clone(),
                self.history_profile_names(),
                w,
                cx,
            )
        });
        self.history_subscription = Some(cx.subscribe_in(&panel, w, |s, source, event, w, cx| {
            if s.history_panel
                .as_ref()
                .is_none_or(|p| p.entity_id() != source.entity_id())
            {
                return;
            }
            match event {
                history_panel::HistoryPanelEvent::Close => {
                    s.show_history = false;
                    cx.notify();
                }
                history_panel::HistoryPanelEvent::Open(entry) => s.open_history_entry(entry, w, cx),
            }
        }));
        self.history_panel = Some(panel);
        self.show_history = true;
        self.stream_export_visible = false;
        self.preferences = None;
        self.form = None;
        self.structure = None;
        self.value_inspector = None;
        self.range_copy = None;
        self.object_finder = None;
        self.cancel_object_reveal();
        self.show_sql_tools = false;
        self.transfer_preview = None;
        self.dismiss_completion(cx);
        cx.notify();
    }
    pub(super) fn sync_history_panel(&self, cx: &mut Context<Self>) {
        if self.show_history
            && let Some(panel) = &self.history_panel
        {
            panel.update(cx, |panel, cx| {
                panel.update(
                    self.workspace.history.clone(),
                    self.history_profile_names(),
                    self.locale,
                    cx,
                )
            });
        }
    }
    fn open_history_entry(&mut self, entry: &HistoryEntry, w: &mut Window, cx: &mut Context<Self>) {
        if self.documents.len() >= 32 {
            self.show_sql_tab_limit(cx);
            return;
        }
        self.new_document(entry.sql.clone(), "history.sql".into(), false, w, cx);
        let request = entry
            .execution_target
            .as_ref()
            .and_then(|t| {
                self.workspace
                    .profiles
                    .iter()
                    .find(|p| p.id == t.profile_id)
                    .and_then(|p| t.resolve(p).ok())
            })
            .map(|profile| ConnectionRequest {
                profile,
                password: String::new(),
            });
        if let Some(doc) = self
            .documents
            .iter_mut()
            .find(|d| d.model.id == self.active_document)
        {
            doc.required_profile_id = Some(entry.profile_id.clone());
            doc.session = ConsoleSession::new(doc.model.id);
            if let Some(request) = request {
                let _ = doc.session.bind(doc.session.route(), request);
            }
            let database = doc
                .session
                .target()
                .map(|p| p.database.clone())
                .unwrap_or_default();
            doc.database_draft
                .update(cx, |input, cx| input.set_value(database, w, cx));
            doc.password_draft
                .update(cx, |input, cx| input.set_value(String::new(), w, cx));
        }
        self.show_history = false;
        self.status = "History opened without execution".into();
        cx.notify();
    }
    pub(super) fn history_view(&self, cx: &mut Context<Self>) -> Div {
        let mut view = div()
            .size_full()
            .min_h_0()
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .border_l_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col();
        view = view.child(
            div()
                .px_3()
                .py_2()
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .child(if self.workspace.persist_history {
                    if self.locale == Locale::Ko {
                        "이 기기에 기록 저장 중 · SQL에 민감한 값이 포함될 수 있음"
                    } else {
                        "Saved on this device · SQL may contain sensitive values"
                    }
                } else if self.locale == Locale::Ko {
                    "이번 실행의 기록만 표시 · 파일 저장 안 함"
                } else {
                    "Session history only · not saved to disk"
                })
                .child(
                    self.button(
                        "persist-history",
                        if self.workspace.persist_history {
                            if self.locale == Locale::Ko {
                                "기록 저장 중지"
                            } else {
                                "Stop saving history"
                            }
                        } else {
                            if self.locale == Locale::Ko {
                                "기기에 기록 저장"
                            } else {
                                "Save history locally"
                            }
                        },
                    )
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.workspace.persist_history = !s.workspace.persist_history;
                        s.persist_profile_mutation(
                            ProfileMutation::PersistHistory(
                                s.workspace.persist_history,
                                s.workspace.history.clone(),
                            ),
                            cx,
                        );
                        cx.notify();
                    })),
                ),
        );
        if let Some(panel) = &self.history_panel {
            view = view.child(div().flex_1().min_h_0().child(panel.clone()));
        }
        view
    }
}
