use super::*;
use dbide_ui_spike::navigator::{Data, Key, Relation, Scope};
use gpui_component::menu::{DropdownMenu, PopupMenuItem};

const VISIBLE_ROWS: usize = 1000;
struct TreeRows {
    rows: Vec<TreeRow>,
    clipped: bool,
    query: String,
}
struct TreeRow {
    key: Key,
    label: String,
    depth: usize,
    relation: Option<Relation>,
    field: bool,
    unavailable: bool,
}
impl Workbench {
    pub(super) fn navigator_request(&self, key: &Key) -> Option<ConnectionRequest> {
        let mut profile = self.navigator.profile(key)?;
        profile.name = self
            .workspace
            .profiles
            .iter()
            .find(|p| p.id == key.profile_id)?
            .name
            .clone();
        let candidates = self
            .documents
            .iter()
            .filter(|d| d.model.id == self.active_document)
            .chain(
                self.documents
                    .iter()
                    .filter(|d| d.model.id != self.active_document),
            )
            .filter_map(|d| d.session.request());
        let password = dbide_ui_spike::session_credentials::SessionCredentials::matching_password(
            &profile, candidates,
        )
        .or_else(|| self.passwords.get(&profile))
        .unwrap_or_default();
        Some(ConnectionRequest { profile, password })
    }
    pub(super) fn navigator_expand(&mut self, key: Key, refresh: bool, cx: &mut Context<Self>) {
        self.navigator.sync_profiles(&self.workspace.profiles);
        let key = if refresh && matches!(key.scope, Scope::Databases) {
            match self.navigator.refresh_connection(&key.profile_id) {
                Ok(key) => key,
                Err(error) => {
                    self.navigator.notice = Some(error);
                    cx.notify();
                    return;
                }
            }
        } else {
            key
        };
        if let Some(request) = self.navigator_request(&key) {
            if let Err(error) = self.navigator.expand(key, request, refresh) {
                self.navigator.notice = Some(error);
            }
        } else {
            self.navigator.notice = Some("Connection changed. Reopen this branch.".into());
        }
        cx.notify();
    }
    fn navigator_toggle(&mut self, key: Key, cx: &mut Context<Self>) {
        if self.navigator.branch(&key).is_some() {
            self.navigator.collapse(&key);
            cx.notify();
        } else {
            self.navigator_expand(key, false, cx);
        }
    }
    fn navigator_rows(
        &self,
        key: Key,
        label: String,
        depth: usize,
        relation: Option<Relation>,
        unavailable: bool,
        tree: &mut TreeRows,
    ) {
        if tree.clipped {
            return;
        }
        if !tree.query.is_empty()
            && relation.is_some()
            && !label.to_lowercase().contains(&tree.query)
        {
            let matches=self.navigator.snapshot(&key).is_some_and(|snapshot|matches!(&snapshot.data,Data::Columns(v) if v.iter().any(|c|c.name.to_lowercase().contains(&tree.query)||c.data_type.to_lowercase().contains(&tree.query))));
            if !matches {
                return;
            }
        }
        if tree.rows.len() >= VISIBLE_ROWS {
            tree.clipped = true;
            return;
        }
        tree.rows.push(TreeRow {
            key: key.clone(),
            label,
            depth,
            relation,
            field: false,
            unavailable,
        });
        let Some(snapshot) = self.navigator.snapshot(&key) else {
            return;
        };
        let engine = self.navigator.engine(&key).unwrap_or_default();
        match &snapshot.data {
            Data::Databases(databases) => {
                for database in databases {
                    let scope = match engine {
                        DatabaseEngine::PostgreSql => Scope::Schemas,
                        DatabaseEngine::MySql => Scope::Objects(database.name.clone()),
                        DatabaseEngine::Sqlite => Scope::Objects("main".into()),
                    };
                    self.navigator_rows(
                        key.child(&database.name, scope),
                        database.name.clone(),
                        depth + 1,
                        None,
                        !database.accessible,
                        tree,
                    );
                    if tree.clipped {
                        break;
                    }
                }
            }
            Data::Schemas(schemas) => {
                for schema in schemas {
                    self.navigator_rows(
                        key.child(&key.database, Scope::Objects(schema.name.clone())),
                        schema.name.clone(),
                        depth + 1,
                        None,
                        false,
                        tree,
                    );
                    if tree.clipped {
                        break;
                    }
                }
            }
            Data::Objects(objects) => {
                for object in objects {
                    let relation = Relation {
                        schema: object.schema.clone(),
                        name: object.name.clone(),
                        kind: object.kind.clone(),
                    };
                    self.navigator_rows(
                        key.child(&key.database, Scope::Columns(relation.clone())),
                        format!(
                            "{} {}",
                            if object.kind == "table" { "▦" } else { "◫" },
                            object.name
                        ),
                        depth + 1,
                        Some(relation),
                        false,
                        tree,
                    );
                    if tree.clipped {
                        break;
                    }
                }
            }
            Data::Details { .. } => {}
            Data::Columns(columns) => {
                for column in columns {
                    if !tree.query.is_empty()
                        && !column.name.to_lowercase().contains(&tree.query)
                        && !column.data_type.to_lowercase().contains(&tree.query)
                    {
                        continue;
                    }
                    if tree.rows.len() >= VISIBLE_ROWS {
                        tree.clipped = true;
                        break;
                    }
                    tree.rows.push(TreeRow {
                        key: key.clone(),
                        label: format!(
                            "{}{}{} · {} · {}{}{}",
                            column.name,
                            if column.primary_key_ordinal.is_some() {
                                " PK"
                            } else {
                                ""
                            },
                            if column.foreign_key { " FK" } else { "" },
                            column.data_type,
                            if column.nullable { "NULL" } else { "NOT NULL" },
                            column
                                .default
                                .as_ref()
                                .map(|d| format!(" · DEFAULT {d}"))
                                .unwrap_or_default(),
                            column
                                .extra
                                .as_ref()
                                .map(|d| format!(" · {d}"))
                                .unwrap_or_default()
                        ),
                        depth: depth + 1,
                        relation: None,
                        field: true,
                        unavailable: false,
                    });
                }
            }
        }
    }
    pub(super) fn database_explorer(&self, cx: &mut Context<Self>) -> Div {
        let ko = self.locale == Locale::Ko;
        let query = self.object_search.read(cx).value().trim().to_lowercase();
        let mut tree = TreeRows {
            rows: vec![],
            clipped: false,
            query,
        };
        for profile in &self.workspace.profiles {
            if tree.clipped {
                break;
            }
            if let Some(key) = self.navigator.root(&profile.id) {
                let engine = match profile.engine {
                    DatabaseEngine::PostgreSql => "PG",
                    DatabaseEngine::MySql => "MySQL",
                    DatabaseEngine::Sqlite => "SQLite",
                };
                self.navigator_rows(
                    key,
                    format!("{} · {engine}", profile.name),
                    0,
                    None,
                    false,
                    &mut tree,
                );
            }
        }
        // Filtering never fetches or expands. Preserve ancestor rows so branch actions
        // stay discoverable; fields and relation rows are filtered only over loaded data.
        let stats = self.navigator.stats();
        let mut list = div()
            .id("schema-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2();
        for (ix, row) in tree.rows.into_iter().enumerate() {
            let branch = self.navigator.branch(&row.key);
            let snapshot = branch.and_then(|b| b.snapshot.as_ref());
            if row.field {
                let label = row.label;
                list = list.child(
                    div()
                        .id(("nav-field", ix))
                        .role(Role::Label)
                        .aria_label(label.clone())
                        .ml(px((row.depth * 8) as f32))
                        .h(px(22.))
                        .text_ellipsis()
                        .py_1()
                        .text_size(px(10.))
                        .text_color(rgb(MUTED))
                        .child(label),
                );
                continue;
            }
            let opened = branch.is_some();
            let key = row.key.clone();
            let refresh = row.key.clone();
            let is_root = matches!(row.key.scope, Scope::Databases);
            let profile_id = row.key.profile_id.clone();
            let metadata = snapshot
                .map(|snapshot| format!(" ({})", snapshot.len()))
                .unwrap_or_default();
            let label = format!(
                "{} {}{}{}",
                if opened { "▾" } else { "▸" },
                row.label,
                metadata,
                if row.unavailable {
                    if ko {
                        " · 접근 불가"
                    } else {
                        " · unavailable"
                    }
                } else {
                    ""
                }
            );
            let mut group = div()
                .id(("nav-group", ix))
                .role(Role::Group)
                .aria_label(label.clone())
                .aria_expanded(opened)
                .ml(px((row.depth * 8) as f32))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new(("nav-toggle", ix))
                                .ghost()
                                .small()
                                .flex_1()
                                .min_w_0()
                                .justify_start()
                                .accessibility_label(label.clone())
                                .tooltip(if let Some(snapshot) = snapshot {
                                    format!(
                                        "{} · {} {} · {}s{}",
                                        row.label,
                                        snapshot.len(),
                                        if ko { "불러옴" } else { "loaded" },
                                        snapshot.age().as_secs(),
                                        if snapshot.truncated {
                                            if ko {
                                                " · 일부 목록"
                                            } else {
                                                " · partial list"
                                            }
                                        } else {
                                            ""
                                        }
                                    )
                                } else {
                                    row.label.clone()
                                })
                                .label(label)
                                .disabled(row.unavailable)
                                .on_click(cx.listener(move |s, _, _, cx| {
                                    s.navigator_toggle(key.clone(), cx)
                                })),
                        )
                        .when(branch.is_some_and(|b| b.loading), |el| {
                            el.child(
                                Self::nav_text(("nav-loading", ix), "…")
                                    .role(Role::Status)
                                    .aria_label(if ko { "불러오는 중" } else { "Loading" }),
                            )
                        })
                        .when(snapshot.is_some_and(|s| s.stale() || s.truncated), |el| {
                            let snapshot = snapshot.unwrap();
                            el.child(
                                Self::nav_text(
                                    ("nav-stale", ix),
                                    &format!(
                                        "{}{}",
                                        if snapshot.truncated {
                                            if ko { "일부 " } else { "partial " }
                                        } else {
                                            ""
                                        },
                                        if snapshot.stale() {
                                            format!(
                                                "{} {}s",
                                                if ko { "만료" } else { "stale" },
                                                snapshot.age().as_secs()
                                            )
                                        } else {
                                            String::new()
                                        }
                                    ),
                                )
                                .text_color(rgb(0xd4ad78)),
                            )
                        })
                        .when(opened, |el| {
                            el.child(
                                Button::new(("nav-refresh", ix))
                                    .ghost()
                                    .small()
                                    .label("↻")
                                    .accessibility_label(format!(
                                        "{} {}",
                                        self.ui_label("Refresh"),
                                        row.label
                                    ))
                                    .tooltip(if ko {
                                        "이 목록 새로고침"
                                    } else {
                                        "Refresh this list"
                                    })
                                    .disabled(branch.is_some_and(|b| b.loading))
                                    .on_click(cx.listener(move |s, _, _, cx| {
                                        s.navigator_expand(refresh.clone(), true, cx)
                                    })),
                            )
                        })
                        .when_some(row.relation.as_ref(), |el, relation| {
                            el.child(self.navigator_object_menu(ix, &row.key, relation, cx))
                        })
                        .when(is_root, |el| {
                            el.child(
                                Button::new(("nav-edit", ix))
                                    .ghost()
                                    .small()
                                    .label("✎")
                                    .accessibility_label(format!(
                                        "{} {}",
                                        self.ui_label("Edit"),
                                        row.label
                                    ))
                                    .tooltip(format!("{} {}", self.ui_label("Edit"), row.label))
                                    .disabled(self.busy() || self.profile_write_busy)
                                    .on_click(cx.listener(move |s, _, w, cx| {
                                        if let Some(profile) = s
                                            .workspace
                                            .profiles
                                            .iter()
                                            .find(|p| p.id == profile_id)
                                            .cloned()
                                        {
                                            let password =
                                                s.passwords.get(&profile).unwrap_or_default();
                                            s.form =
                                                Some(ProfileForm::new(profile, &password, w, cx));
                                            s.form_error = None;
                                            cx.notify();
                                        }
                                    })),
                            )
                        }),
                );
            if let Some(error) = branch.and_then(|b| b.error.as_ref()) {
                group = group.child(Self::nav_text(("nav-error", ix), error).role(Role::Alert));
            }
            list = list.child(group);
        }
        if tree.clipped {
            list=list.child(Self::nav_text("nav-view-limit",if ko {"화면 한도 1,000행. 가지를 접거나 필터를 줄이세요. 불러온 전체 목록을 표시하지 않습니다."} else {"View limit: 1,000 rows. Collapse branches. This view is a subset of loaded metadata."}));
        }
        if self.workspace.profiles.is_empty() {
            list = list.child(Self::nav_text(
                "nav-empty",
                if ko {
                    "새 연결을 저장한 뒤 펼쳐서 탐색하세요."
                } else {
                    "Save a connection, then expand it to browse."
                },
            ));
        }
        let mut root =
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .text_color(rgb(MUTED))
                        .text_size(px(10.))
                        .child(self.ui_label("DATABASE EXPLORER")),
                )
                .child(div().px_2().pb_2().child(
                    Input::new(&self.object_search).small().aria_label(if ko {
                        "펼친 스냅샷만 필터"
                    } else {
                        "Filter expanded snapshots only"
                    }),
                ))
                .child(Self::nav_text(
                    "nav-filter-scope",
                    if ko {
                        "펼친 목록만 검색 · SQL 탭 연결은 변경하지 않음"
                    } else {
                        "Filter expanded lists · browsing never rebinds SQL tabs"
                    },
                ))
                .child(list)
                .child(
                    div()
                        .px_2()
                        .py_2()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_1()
                        .child(Self::nav_text(
                            "nav-budget",
                            &format!(
                                "{:.1}/16 MiB · {}/2 · {} {}",
                                stats.accounted_bytes as f64 / 1048576.,
                                self.navigator.running(),
                                self.navigator.pending(),
                                if ko { "대기" } else { "queued" }
                            ),
                        ))
                        .child(
                            Button::new("nav-clear")
                                .ghost()
                                .small()
                                .label(if ko {
                                    "캐시 비우기"
                                } else {
                                    "Clear cache"
                                })
                                .on_click(cx.listener(|s, _, _, cx| {
                                    s.navigator.clear();
                                    cx.notify();
                                })),
                        ),
                );
        if let Some(error) = &self.navigator.notice {
            root = root
                .child(Self::nav_text("nav-notice", error).role(Role::Alert))
                .child(
                    Button::new("nav-dismiss")
                        .ghost()
                        .small()
                        .label(self.ui_label("Dismiss"))
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.navigator.notice = None;
                            cx.notify();
                        })),
                );
        }
        root
    }
    fn navigator_object_menu(
        &self,
        ix: usize,
        key: &Key,
        relation: &Relation,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ready = self.navigator.snapshot(key).is_some();
        let opening = self.pending_table_open.is_some()
            && self.navigator.engine(key) == Some(DatabaseEngine::PostgreSql)
            && relation.kind == "table";
        let weak = cx.weak_entity();
        let key = key.clone();
        let relation = relation.clone();
        let structure = self.ui_label("Structure").to_owned();
        let data = self.ui_label("Data").to_owned();
        let sql = self.ui_label("SQL").to_owned();
        let caption = format!(
            "{} · {} / {}.{}",
            if self.locale == Locale::Ko {
                "객체 작업"
            } else {
                "Object actions"
            },
            key.database,
            relation.schema,
            relation.name
        );
        Button::new(("nav-object-menu", ix))
            .ghost()
            .small()
            .label("⋯")
            .accessibility_label(caption.clone())
            .tooltip(caption)
            .dropdown_menu(move |menu, _, _| {
                let structure_weak = weak.clone();
                let data_weak = weak.clone();
                let sql_weak = weak.clone();
                let structure_key = key.clone();
                let data_key = key.clone();
                let sql_key = key.clone();
                let structure_relation = relation.clone();
                let data_relation = relation.clone();
                let sql_relation = relation.clone();
                menu.label(format!(
                    "{} / {}.{}",
                    key.database, relation.schema, relation.name
                ))
                .item(
                    PopupMenuItem::new(structure.clone())
                        .disabled(!ready)
                        .on_click(move |_, _, cx| {
                            let _ = structure_weak.update(cx, |s, cx| {
                                s.open_branch_structure(&structure_key, &structure_relation, cx)
                            });
                        }),
                )
                .item(PopupMenuItem::new(data.clone()).disabled(opening).on_click(
                    move |_, w, cx| {
                        let _ = data_weak.update(cx, |s, cx| {
                            s.open_branch_console(&data_key, &data_relation, true, w, cx)
                        });
                    },
                ))
                .item(PopupMenuItem::new(sql.clone()).on_click(move |_, w, cx| {
                    let _ = sql_weak.update(cx, |s, cx| {
                        s.open_branch_console(&sql_key, &sql_relation, false, w, cx)
                    });
                }))
            })
            .into_any_element()
    }
    fn nav_text(id: impl Into<ElementId>, text: &str) -> Stateful<Div> {
        div()
            .id(id)
            .role(Role::Label)
            .aria_label(text.to_owned())
            .px_2()
            .py_1()
            .text_size(px(10.))
            .text_color(rgb(MUTED))
            .child(text.to_owned())
    }
    fn open_branch_structure(&mut self, key: &Key, relation: &Relation, cx: &mut Context<Self>) {
        let (Some(request), Some(snapshot)) =
            (self.navigator_request(key), self.navigator.snapshot(key))
        else {
            self.navigator.notice = Some("Reload this branch before opening structure.".into());
            cx.notify();
            return;
        };
        let panel = cx.new(|cx| {
            object_inspector::ObjectInspector::from_snapshot(
                request,
                key.clone(),
                relation.clone(),
                snapshot,
                self.locale,
                cx,
            )
        });
        self._subscriptions
            .push(cx.subscribe(&panel, |s, source, event, cx| {
                if s.structure
                    .as_ref()
                    .is_none_or(|current| current.entity_id() != source.entity_id())
                {
                    return;
                }
                let object_inspector::InspectorEvent::LoadDetails(key) = event;
                s.navigator_expand(key.clone(), true, cx);
                s.sync_navigator_inspector(cx);
            }));
        self.structure = Some(panel);
        self.sync_navigator_inspector(cx);
        self.form = None;
        cx.notify();
    }
    pub(super) fn sync_navigator_inspector(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.structure.clone() else {
            return;
        };
        let key = panel.read(cx).detail_key.clone();
        if !self.navigator.authorized(&key) {
            panel.update(cx,|p,cx|p.set_details(None,Some("Connection changed. Close this snapshot and reopen the object under its current connection before loading details.".into()),false,cx));
            return;
        }
        if let Some(branch) = self.navigator.branch(&key) {
            let snapshot = branch.snapshot.clone();
            let error = branch.error.clone();
            let loading = branch.loading;
            panel.update(cx, |p, cx| p.set_details(snapshot, error, loading, cx));
        }
    }
    fn open_branch_console(
        &mut self,
        key: &Key,
        relation: &Relation,
        run: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.navigator_request(key) else {
            self.navigator.notice = Some("Connection changed. Reopen this branch.".into());
            cx.notify();
            return;
        };
        let sql = dbide_ui_spike::workbench_model::table_query(
            request.profile.engine,
            &relation.schema,
            &relation.name,
        );
        if run && request.profile.engine == DatabaseEngine::PostgreSql && relation.kind == "table" {
            self.begin_table_open(key.clone(), request, relation.clone(), window, cx);
            return;
        }
        if self.documents.len() >= 32 {
            self.show_sql_tab_limit(cx);
            return;
        }
        self.new_document(
            sql.clone(),
            format!(
                "{}-{}.sql",
                relation.name,
                if run { "data" } else { "query" }
            ),
            false,
            window,
            cx,
        );
        let route = self.console().route();
        self.bind_console(route, request, window, cx);
        if run {
            self.submit_sql(sql, cx);
        }
    }
    // Existing read-only table fallback keeps its captured browser target.
    pub(super) fn open_data_query(
        &mut self,
        sql: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.documents.len() >= 32 {
            self.show_sql_tab_limit(cx);
            return;
        }
        self.new_document(sql.clone(), title, false, window, cx);
        if let Some(request) = self.configured.clone() {
            let route = self.console().route();
            self.bind_console(route, request, window, cx);
        }
        self.submit_sql(sql, cx);
    }
}
