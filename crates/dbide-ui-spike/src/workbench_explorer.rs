use super::*;
use dbide_ui_spike::navigator::{Data, Key, Relation, Scope};
use gpui_component::{
    Icon,
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit_assets::IconName;

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
    detail: String,
    annotation: String,
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
            self.navigator.notice = Some(
                if self.locale == Locale::Ko {
                    "연결이 변경되었습니다. 이 목록을 다시 여세요."
                } else {
                    "Connection changed. Reopen this branch."
                }
                .into(),
            );
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
            detail: String::new(),
            annotation: String::new(),
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
                        object.name.clone(),
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
                        label: column.name.clone(),
                        detail: format!(
                            "{} · {}{}{}",
                            column.name,
                            column.data_type,
                            if column.primary_key_ordinal.is_some() {
                                " · PK"
                            } else {
                                ""
                            },
                            if column.foreign_key { " · FK" } else { "" }
                        ),
                        annotation: if column.primary_key_ordinal.is_some() {
                            "PK".into()
                        } else if column.foreign_key {
                            "FK".into()
                        } else {
                            column.data_type.clone()
                        },
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
            let identity = match &row.key.scope {
                Scope::Databases => row.label.clone(),
                Scope::Schemas => format!("{} / {}", row.key.profile_id, row.key.database),
                Scope::Objects(schema) => {
                    format!("{} / {} / {schema}", row.key.profile_id, row.key.database)
                }
                Scope::Columns(relation) | Scope::Details(relation) => format!(
                    "{} / {} / {}.{}",
                    row.key.profile_id, row.key.database, relation.schema, relation.name
                ),
            };
            if row.field {
                let caption = format!("{identity} / {}", row.detail);
                list = list.child(
                    div()
                        .id(("nav-field", ix))
                        .role(Role::Label)
                        .aria_label(caption.clone())
                        .ml(px((row.depth.min(4) * 10 + 18) as f32))
                        .h(px(25.))
                        .pr_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(div().min_w_0().flex_1().text_ellipsis().child(row.label))
                        .child(
                            div()
                                .max_w(px(90.))
                                .text_ellipsis()
                                .text_size(px(10.))
                                .child(row.annotation),
                        )
                        .tooltip(move |_, cx| {
                            cx.new(|_| gpui_component::tooltip::Tooltip::new(caption.clone()))
                                .into()
                        }),
                );
                continue;
            }
            let opened = branch.is_some();
            let key = row.key.clone();
            let refresh = row.key.clone();
            let is_root = matches!(row.key.scope, Scope::Databases);
            let profile_id = row.key.profile_id.clone();
            let selected = self.structure.as_ref().is_some_and(|panel| {
                let target = &panel.read(cx).detail_key;
                target.profile_id == row.key.profile_id && target.database == row.key.database
                    && target.generation == row.key.generation && target.fingerprint == row.key.fingerprint
                    && matches!((&target.scope, &row.relation), (Scope::Details(a), Some(b)) if a == b)
            });
            let icon = match &row.key.scope {
                Scope::Databases => IconName::HardDrive,
                Scope::Schemas => IconName::FolderClosed,
                Scope::Objects(_) => {
                    if opened {
                        IconName::FolderOpen
                    } else {
                        IconName::Folder
                    }
                }
                Scope::Columns(relation) | Scope::Details(relation) => {
                    if relation.kind == "table" {
                        IconName::LayoutDashboard
                    } else {
                        IconName::FileText
                    }
                }
            };
            let caption = format!(
                "{}{}{}",
                identity,
                if row.unavailable {
                    if ko {
                        " · 접근 불가"
                    } else {
                        " · unavailable"
                    }
                } else {
                    ""
                },
                snapshot
                    .map(|snapshot| format!(
                        " · {} {} · {}s{}{}",
                        snapshot.len(),
                        if ko { "불러옴" } else { "loaded" },
                        snapshot.age().as_secs(),
                        if snapshot.historical {
                            if ko {
                                " · 파일 캐시 · 이전 수집본"
                            } else {
                                " · file cache · historical"
                            }
                        } else if snapshot.stale() {
                            if ko { " · 만료됨" } else { " · expired" }
                        } else {
                            ""
                        },
                        if snapshot.truncated {
                            if ko {
                                " · 일부 목록"
                            } else {
                                " · partial list"
                            }
                        } else {
                            ""
                        }
                    ))
                    .unwrap_or_default()
            );
            let mut group = div()
                .id(("nav-group", ix))
                .role(Role::Group)
                .aria_label(caption.clone())
                .aria_expanded(opened)
                .child(
                    div()
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .gap_1()
                        .rounded(px(4.))
                        .pl(px((row.depth.min(3) * 10) as f32))
                        .when(selected, |el| el.bg(rgb(0x25334b)))
                        .hover(|el| el.bg(rgb(0x252d39)))
                        .child(
                            Button::new(("nav-toggle", ix))
                                .ghost()
                                .small()
                                .h(px(28.))
                                .flex_1()
                                .min_w_0()
                                .justify_start()
                                .px_1()
                                .accessibility_label(caption.clone())
                                .tooltip(caption)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .w_full()
                                        .min_w_0()
                                        .gap_1()
                                        .child(
                                            div()
                                                .w(px(10.))
                                                .flex_shrink_0()
                                                .text_color(rgb(MUTED))
                                                .text_size(px(10.))
                                                .child(
                                                    Icon::new(if opened {
                                                        IconName::ChevronDown
                                                    } else {
                                                        IconName::ChevronRight
                                                    })
                                                    .size(px(10.)),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .w(px(15.))
                                                .flex_shrink_0()
                                                .text_color(rgb(if selected {
                                                    ACCENT
                                                } else {
                                                    MUTED
                                                }))
                                                .child(Icon::new(icon).size(px(14.))),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_ellipsis()
                                                .text_size(px(12.))
                                                .text_color(rgb(if row.unavailable {
                                                    MUTED
                                                } else {
                                                    TEXT
                                                }))
                                                .child(row.label.clone()),
                                        )
                                        .when_some(snapshot, |el, snapshot| {
                                            el.child(
                                                div()
                                                    .flex_shrink_0()
                                                    .text_size(px(10.))
                                                    .text_color(rgb(MUTED))
                                                    .child(snapshot.len().to_string()),
                                            )
                                        }),
                                )
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
                            el.child(div().text_color(rgb(0xd4ad78)).text_size(px(10.)).child(
                                if snapshot.is_some_and(|s| s.historical) {
                                    if ko { "저장본" } else { "saved" }
                                } else {
                                    "•"
                                },
                            ))
                        })
                        .when(opened, |el| {
                            el.child(
                                Button::new(("nav-refresh", ix))
                                    .ghost()
                                    .small()
                                    .w(px(22.))
                                    .icon(IconName::RefreshCw)
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
                                    .w(px(22.))
                                    .icon(IconName::Settings2)
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
            if opened
                && branch.is_some_and(|b| !b.loading && b.error.is_none())
                && snapshot.is_some_and(|s| s.is_empty())
            {
                group = group.child(
                    Self::nav_text(
                        ("nav-branch-empty", ix),
                        if ko { "객체 없음" } else { "No objects" },
                    )
                    .ml(px((row.depth.min(3) * 10 + 26) as f32)),
                );
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
                        "불러온 객체에서 검색"
                    } else {
                        "Search loaded objects"
                    },
                ))
                .child(list)
                .when(
                    self.navigator.running() > 0 || self.navigator.pending() > 0,
                    |el| {
                        el.child(
                            div()
                                .px_2()
                                .py_2()
                                .border_t_1()
                                .border_color(rgb(BORDER))
                                .child(Self::nav_text(
                                    "nav-budget",
                                    &format!(
                                        "{:.1} MiB · {} {} · {} {}",
                                        stats.accounted_bytes as f64 / 1048576.,
                                        self.navigator.running(),
                                        if ko { "조회 중" } else { "loading" },
                                        self.navigator.pending(),
                                        if ko { "대기" } else { "queued" }
                                    ),
                                )),
                        )
                    },
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
            .icon(IconName::Ellipsis)
            .w(px(22.))
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
            self.navigator.notice = Some(
                if self.locale == Locale::Ko {
                    "구조를 열기 전에 이 목록을 새로고침하세요."
                } else {
                    "Reload this branch before opening structure."
                }
                .into(),
            );
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
                let object_inspector::InspectorEvent::LoadDetails(key, refresh) = event;
                s.navigator_expand(key.clone(), *refresh, cx);
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
            let message = if self.locale == Locale::Ko {
                "연결이 변경되었습니다. 현재 연결에서 객체를 다시 열어 상세 정보를 불러오세요."
            } else {
                "Connection changed. Reopen the object under its current connection before loading details."
            };
            panel.update(cx, |p, cx| {
                p.set_details(None, Some(message.into()), false, cx)
            });
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
            self.navigator.notice = Some(
                if self.locale == Locale::Ko {
                    "연결이 변경되었습니다. 이 목록을 다시 여세요."
                } else {
                    "Connection changed. Reopen this branch."
                }
                .into(),
            );
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
