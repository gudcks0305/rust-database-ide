use super::*;
use dbide_ui_spike::navigator::{Data, Key, Lease, Relation, Scope};

pub(super) enum InspectorEvent {
    LoadDetails(Key),
}
/// Two immutable cache leases, not duplicate column/table payloads. Holding the
/// drawer open continues charging its snapshots after cache invalidation.
pub(super) struct ObjectInspector {
    pub detail_key: Key,
    relation: Relation,
    target: String,
    columns: Lease,
    details: Option<Lease>,
    detail_error: Option<String>,
    detail_loading: bool,
    pg: bool,
    locale: Locale,
}
impl EventEmitter<InspectorEvent> for ObjectInspector {}
impl ObjectInspector {
    pub fn from_snapshot(
        request: ConnectionRequest,
        key: Key,
        relation: Relation,
        columns: Lease,
        locale: Locale,
        _: &mut Context<Self>,
    ) -> Self {
        Self {
            detail_key: key.child(&key.database, Scope::Details(relation.clone())),
            relation,
            target: format!(
                "{} · {}",
                request.profile.name,
                dbide_ui_spike::workbench_model::profile_target(&request.profile)
            ),
            columns,
            details: None,
            detail_error: None,
            detail_loading: false,
            pg: request.profile.engine == DatabaseEngine::PostgreSql,
            locale,
        }
    }
    pub fn set_details(
        &mut self,
        details: Option<Lease>,
        error: Option<String>,
        loading: bool,
        cx: &mut Context<Self>,
    ) {
        self.details = details;
        self.detail_error = error;
        self.detail_loading = loading;
        cx.notify();
    }
    fn columns(&self) -> &[dbide_bridge::SchemaColumn] {
        match &self.columns.data {
            Data::Columns(v) => v,
            _ => &[],
        }
    }
    fn text(id: impl Into<ElementId>, text: String) -> Stateful<Div> {
        div()
            .id(id)
            .role(Role::Label)
            .aria_label(text.clone())
            .child(text)
    }
}
impl Render for ObjectInspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ko = self.locale == Locale::Ko;
        let mut root = div()
            .id("object-structure-scroll")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .text_color(rgb(TEXT))
            .text_size(px(11.))
            .child(
                Self::text("structure-target", self.target.clone())
                    .text_size(px(10.))
                    .text_color(rgb(MUTED)),
            )
            .child(
                Self::text(
                    "structure-name",
                    format!("{}.{}", self.relation.schema, self.relation.name),
                )
                .mt_2()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                Self::text(
                    "structure-capture-status",
                    if ko {
                        format!(
                            "열 스냅샷 · {}초 전{}{}",
                            self.columns.age().as_secs(),
                            if self.columns.stale() {
                                " · 만료됨: 탐색기에서 새로고침"
                            } else {
                                ""
                            },
                            if self.columns.truncated {
                                " · 일부 열만 반환됨"
                            } else {
                                ""
                            }
                        )
                    } else {
                        format!(
                            "Column snapshot · {}s old{}{}",
                            self.columns.age().as_secs(),
                            if self.columns.stale() {
                                " · expired: refresh the navigator"
                            } else {
                                ""
                            },
                            if self.columns.truncated {
                                " · partial columns"
                            } else {
                                ""
                            }
                        )
                    },
                )
                .mt_2()
                .text_size(px(10.))
                .text_color(rgb(MUTED)),
            )
            .child(
                Button::new("copy-structure-fields")
                    .label(if ko { "필드 복사" } else { "Copy fields" })
                    .small()
                    .ghost()
                    .on_click(cx.listener(|s, _, _, cx| {
                        let mut text = "Name\tType\tNULL\tKey\tDefault\tExtra\n".to_string();
                        for c in s.columns() {
                            text.push_str(&format!(
                                "{}\t{}\t{}\t{}{}\t{}\t{}\n",
                                c.name,
                                c.data_type,
                                c.nullable,
                                if c.primary_key_ordinal.is_some() {
                                    "PK "
                                } else {
                                    ""
                                },
                                if c.foreign_key { "FK" } else { "" },
                                c.default.as_deref().unwrap_or(""),
                                c.extra.as_deref().unwrap_or("")
                            ));
                        }
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    })),
            );
        let cell = |text: String, width: f32| div().w(px(width)).flex_shrink_0().px_2().child(text);
        let mut columns = div().min_w(px(650.)).child(
            div()
                .h(px(26.))
                .flex()
                .items_center()
                .bg(rgb(BG))
                .text_color(rgb(MUTED))
                .child(cell(if ko { "이름" } else { "Name" }.into(), 130.))
                .child(cell(if ko { "자료형" } else { "Type" }.into(), 150.))
                .child(cell("NULL".into(), 55.))
                .child(cell(if ko { "키" } else { "Key" }.into(), 55.))
                .child(cell(
                    if ko {
                        "기본값 / 속성"
                    } else {
                        "Default / extra"
                    }
                    .into(),
                    260.,
                )),
        );
        for (ix, c) in self.columns().iter().enumerate() {
            let key = format!(
                "{}{}",
                if c.primary_key_ordinal.is_some() {
                    "PK "
                } else {
                    ""
                },
                if c.foreign_key { "FK" } else { "" }
            );
            let default = c.default.as_deref().unwrap_or("—");
            let extra = c.extra.as_deref().unwrap_or("");
            let label = format!(
                "{} · {} · {} · {} · DEFAULT {} · {}",
                c.name,
                c.data_type,
                if c.nullable { "NULL" } else { "NOT NULL" },
                key,
                default,
                extra
            );
            columns = columns.child(
                Self::text(("structure-column", ix), String::new())
                    .aria_label(label)
                    .min_h(px(30.))
                    .py_1()
                    .flex()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(cell(c.name.clone(), 130.))
                    .child(cell(c.data_type.clone(), 150.))
                    .child(cell(if c.nullable { "yes" } else { "no" }.into(), 55.))
                    .child(cell(key, 55.))
                    .child(cell(
                        format!(
                            "{default}{}{}",
                            if extra.is_empty() { "" } else { " · " },
                            extra
                        ),
                        260.,
                    )),
            );
        }
        root = root.child(
            div()
                .id("structure-columns-horizontal")
                .mt_3()
                .w_full()
                .overflow_x_scroll()
                .child(columns),
        );
        if self.pg {
            root = root.child(
                Button::new("load-structure-details")
                    .small()
                    .mt_3()
                    .label(if ko {
                        "인덱스·FK 대상·뷰 정의 불러오기"
                    } else {
                        "Load indexes / FK targets / view definition"
                    })
                    .disabled(self.detail_loading)
                    .on_click(cx.listener(|s, _, _, cx| {
                        cx.emit(InspectorEvent::LoadDetails(s.detail_key.clone()))
                    })),
            );
        }
        let status = if self.detail_loading {
            if ko {
                "상세 메타데이터 불러오는 중…"
            } else {
                "Loading detailed metadata…"
            }
        } else if self.details.is_none() {
            if ko {
                "상세 인덱스·FK 대상·뷰 정의는 아직 불러오지 않았습니다."
            } else {
                "Detailed indexes, FK targets and view definition are not loaded."
            }
        } else {
            if ko {
                "상세 정보는 별도의 읽기 전용 시점 스냅샷입니다. 열 목록과 수집 시점이 다를 수 있습니다."
            } else {
                "Details are a separate read-only snapshot; their capture time may differ from the columns above."
            }
        };
        root = root.child(
            Self::text("structure-detail-status", status.into())
                .mt_2()
                .text_color(rgb(MUTED)),
        );
        if let Some(error) = &self.detail_error {
            root = root.child(
                Self::text("structure-detail-error", error.clone())
                    .mt_2()
                    .text_color(rgb(0xe6ad80)),
            );
        }
        if let Some(snapshot) = &self.details {
            root = root.child(
                Self::text(
                    "structure-detail-age",
                    format!(
                        "{}s{}",
                        snapshot.age().as_secs(),
                        if snapshot.stale() {
                            if ko {
                                " · 만료된 상세 스냅샷"
                            } else {
                                " · expired detail snapshot"
                            }
                        } else {
                            ""
                        }
                    ),
                )
                .mt_2()
                .text_color(rgb(MUTED)),
            );
            if let Data::Details { table, view } = &snapshot.data {
                if let Some(table) = table {
                    root = root.child(
                        Self::text(
                            "structure-indexes-heading",
                            if ko { "인덱스" } else { "Indexes" }.into(),
                        )
                        .mt_4()
                        .font_weight(FontWeight::SEMIBOLD),
                    );
                    if table.indexes.is_empty() {
                        root = root.child(
                            Self::text(
                                "structure-no-indexes",
                                if ko {
                                    "이 상세 스냅샷에 인덱스 없음"
                                } else {
                                    "No indexes in this detail snapshot"
                                }
                                .into(),
                            )
                            .mt_2(),
                        );
                    }
                    for (ix, index) in table.indexes.iter().enumerate() {
                        let text = format!(
                            "{} · {}{} ({}){}",
                            index.name,
                            index.method,
                            if index.unique { " · UNIQUE" } else { "" },
                            index.columns.join(", "),
                            index
                                .predicate
                                .as_ref()
                                .map(|p| format!(" WHERE {p}"))
                                .unwrap_or_default()
                        );
                        root = root.child(Self::text(("structure-index", ix), text).mt_2());
                    }
                    for (ix, fk) in table.foreign_keys.iter().enumerate() {
                        root = root.child(
                            Self::text(
                                ("structure-fk", ix),
                                format!(
                                    "FK {} ({}) → {} ({}) · UPDATE {} · DELETE {}",
                                    fk.name.as_deref().unwrap_or(""),
                                    fk.columns.join(", "),
                                    fk.references.sql(),
                                    fk.referenced_columns.join(", "),
                                    fk.on_update.sql(),
                                    fk.on_delete.sql()
                                ),
                            )
                            .mt_2(),
                        );
                    }
                }
                if let Some(view) = view {
                    root = root.child(
                        Self::text("structure-view-definition", view.definition.clone())
                            .mt_4()
                            .font_family("Menlo"),
                    );
                }
            }
        }
        root
    }
}
