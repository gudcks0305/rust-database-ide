use super::*;
use dbide_ui_spike::navigator::{Data, Key, Lease, Relation, Scope};
use gpui_component::tooltip::Tooltip;

pub(super) enum InspectorEvent {
    LoadDetails(Key, bool),
}
/// Immutable cache leases keep the captured connection and payload accounting.
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
    pub(super) fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.locale = locale;
        cx.notify();
    }

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
    fn tr(&self, ko: &'static str, en: &'static str) -> &'static str {
        if self.locale == Locale::Ko { ko } else { en }
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
    fn cell(id: impl Into<ElementId>, text: String, width: f32) -> Stateful<Div> {
        let tooltip = text.clone();
        Self::text(id, text)
            .role(Role::Cell)
            .w(px(width))
            .flex_shrink_0()
            .px_3()
            .overflow_hidden()
            .text_ellipsis()
            .tooltip(move |_, cx| cx.new(|_| Tooltip::new(tooltip.clone())).into())
    }
    fn badge(text: String, accent: bool) -> Div {
        div()
            .px_2()
            .py(px(2.))
            .rounded(px(3.))
            .bg(rgb(if accent { 0x25334b } else { BG }))
            .text_color(rgb(if accent { ACCENT } else { MUTED }))
            .text_size(px(10.))
            .whitespace_nowrap()
            .child(text)
    }
    fn section(&self, id: &'static str, title: &str, count: usize) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .mt_5()
            .mb_2()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_owned()),
            )
            .child(Self::badge(count.to_string(), false))
    }
    fn empty(&self, id: &'static str, message: &str) -> Stateful<Div> {
        Self::text(id, message.to_owned())
            .py_3()
            .text_color(rgb(MUTED))
    }
    fn header(cells: Vec<(&'static str, f32)>) -> Div {
        div()
            .h(px(30.))
            .flex()
            .items_center()
            .bg(rgb(BG))
            .border_b_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(MUTED))
            .text_size(px(10.))
            .children(cells.into_iter().enumerate().map(|(ix, (label, width))| {
                Self::cell(("structure-heading-cell", ix), label.to_owned(), width)
                    .role(Role::ColumnHeader)
            }))
    }
}
impl Render for ObjectInspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = self.columns();
        let identity = format!(
            "{} / {}.{}",
            self.detail_key.database, self.relation.schema, self.relation.name
        );
        let kind = match self.relation.kind.as_str() {
            "table" | "r" => self.tr("테이블", "Table"),
            "view" | "v" => self.tr("뷰", "View"),
            "materialized view" | "m" => self.tr("구체화 뷰", "Materialized view"),
            other => other,
        };
        let mut root = div()
            .id("object-structure-scroll")
            .size_full()
            .min_w_0()
            .overflow_y_scroll()
            .p_4()
            .text_color(rgb(TEXT))
            .text_size(px(12.))
            .child(
                Self::text("structure-target", self.target.clone())
                    .text_color(rgb(MUTED))
                    .text_size(px(10.)),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Self::text("structure-name", self.relation.name.clone())
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(Self::badge(kind.to_owned(), false)),
            )
            .child(
                Self::text("structure-identity", identity)
                    .mt_1()
                    .text_color(rgb(MUTED))
                    .text_size(px(11.)),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_wrap()
                    .child(Self::badge(
                        if self.columns.historical {
                            self.tr(
                                "저장된 메타데이터 · 이전 수집본",
                                "Saved metadata · historical",
                            )
                            .into()
                        } else {
                            self.tr("읽기 전용 스냅샷", "Read-only snapshot").into()
                        },
                        true,
                    ))
                    .child(
                        Self::text(
                            "structure-capture-status",
                            format!(
                                "{} {}s",
                                self.tr("수집 후", "Captured"),
                                self.columns.age().as_secs()
                            ),
                        )
                        .text_size(px(10.))
                        .text_color(rgb(MUTED)),
                    )
                    .when(self.columns.stale(), |el| {
                        el.child(Self::badge(
                            self.tr(
                                "만료됨 · 탐색기에서 새로고침",
                                "Expired · refresh in explorer",
                            )
                            .into(),
                            false,
                        ))
                    })
                    .when(self.columns.truncated, |el| {
                        el.child(Self::badge(
                            self.tr("일부 필드", "Partial fields").into(),
                            false,
                        ))
                    }),
            )
            .child(
                self.section(
                    "structure-fields-heading",
                    self.tr("필드", "Fields"),
                    columns.len(),
                )
                .child(div().flex_1())
                .child(
                    Button::new("copy-structure-fields")
                        .label(self.tr("필드 복사", "Copy fields"))
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
                ),
            );
        let mut grid = div()
            .min_w(px(720.))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded(px(4.))
            .overflow_hidden()
            .child(Self::header(vec![
                (self.tr("이름", "Name"), 150.),
                (self.tr("자료형", "Type"), 170.),
                ("NULL", 70.),
                (self.tr("키", "Key"), 70.),
                (self.tr("기본값 / 속성", "Default / extra"), 260.),
            ]));
        for (ix, column) in columns.iter().enumerate() {
            let key = match (column.primary_key_ordinal.is_some(), column.foreign_key) {
                (true, true) => "PK · FK",
                (true, false) => "PK",
                (false, true) => "FK",
                _ => "—",
            };
            let default = [column.default.clone(), column.extra.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            let label = format!(
                "{} · {} · {} · {key} · {}",
                column.name,
                column.data_type,
                if column.nullable { "NULL" } else { "NOT NULL" },
                default
            );
            grid = grid.child(
                div()
                    .id(("structure-column", ix))
                    .role(Role::Row)
                    .aria_label(label)
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .when(ix % 2 == 1, |el| el.bg(rgb(PANEL)))
                    .hover(|el| el.bg(rgb(0x242c38)))
                    .child(
                        Self::cell(("structure-field-name", ix), column.name.clone(), 150.)
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .child(
                        Self::cell(("structure-field-type", ix), column.data_type.clone(), 170.)
                            .text_color(rgb(MUTED)),
                    )
                    .child(
                        Self::cell(
                            ("structure-field-null", ix),
                            self.tr(
                                if column.nullable { "허용" } else { "불가" },
                                if column.nullable { "Yes" } else { "No" },
                            )
                            .into(),
                            70.,
                        )
                        .text_color(rgb(MUTED)),
                    )
                    .child(
                        Self::cell(("structure-field-key", ix), key.into(), 70.)
                            .text_color(rgb(if key == "—" { MUTED } else { ACCENT })),
                    )
                    .child(
                        Self::cell(
                            ("structure-field-default", ix),
                            if default.is_empty() {
                                "—".into()
                            } else {
                                default
                            },
                            260.,
                        )
                        .font_family("Menlo")
                        .text_size(px(11.))
                        .text_color(rgb(MUTED)),
                    ),
            );
        }
        root = root.child(
            div()
                .id("structure-columns-horizontal")
                .w_full()
                .overflow_x_scroll()
                .child(grid),
        );
        if columns.is_empty() {
            root = root.child(self.empty(
                "structure-no-fields",
                self.tr(
                    "이 스냅샷에 필드가 없습니다.",
                    "No fields in this snapshot.",
                ),
            ));
        }
        root = root.child(
            div()
                .mt_5()
                .pt_3()
                .border_t_1()
                .border_color(rgb(BORDER))
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.tr("객체 상세", "Object details")),
                )
                .when(self.pg, |el| {
                    el.child(
                        Button::new("load-structure-details")
                            .small()
                            .ghost()
                            .label(if self.detail_loading {
                                self.tr("불러오는 중…", "Loading…")
                            } else if self.details.is_some() {
                                self.tr("상세 새로고침", "Refresh details")
                            } else {
                                self.tr("상세 불러오기", "Load details")
                            })
                            .disabled(self.detail_loading)
                            .on_click(cx.listener(|s, _, _, cx| {
                                cx.emit(InspectorEvent::LoadDetails(
                                    s.detail_key.clone(),
                                    s.details.is_some(),
                                ))
                            })),
                    )
                }),
        );
        if let Some(error) = &self.detail_error {
            root = root.child(
                Self::text("structure-detail-error", error.clone())
                    .role(Role::Alert)
                    .mt_2()
                    .p_3()
                    .bg(rgb(BG))
                    .text_color(rgb(0xe6ad80)),
            );
        }
        let Some(snapshot) = &self.details else {
            return root.child(self.empty(
                "structure-detail-status",
                if self.detail_loading {
                    self.tr(
                        "인덱스·제약 조건·뷰 정의를 불러오는 중…",
                        "Loading indexes, constraints and view definition…",
                    )
                } else if self.pg {
                    self.tr(
                        "인덱스·제약 조건·뷰 정의는 필요할 때 불러옵니다.",
                        "Load indexes, constraints and view definition on demand.",
                    )
                } else {
                    self.tr(
                        "이 연결에서는 추가 상세 조회를 지원하지 않습니다.",
                        "Additional object details are unavailable for this connection.",
                    )
                },
            ));
        };
        root = root.child(
            Self::text(
                "structure-detail-status",
                format!(
                    "{} · {}s{}{}",
                    self.tr(
                        "별도 수집된 읽기 전용 상세 스냅샷",
                        "Separate read-only detail snapshot"
                    ),
                    snapshot.age().as_secs(),
                    if snapshot.historical {
                        self.tr(" · 파일 캐시 · 이전 수집본", " · file cache · historical")
                    } else if snapshot.stale() {
                        self.tr(" · 만료됨", " · expired")
                    } else {
                        ""
                    },
                    if snapshot.truncated {
                        self.tr(" · 일부 상세 정보", " · partial details")
                    } else {
                        ""
                    }
                ),
            )
            .mt_2()
            .text_size(px(10.))
            .text_color(rgb(MUTED)),
        );
        if let Data::Details { table, view } = &snapshot.data {
            if let Some(table) = table {
                root = root.child(self.section(
                    "structure-indexes-heading",
                    self.tr("인덱스", "Indexes"),
                    table.indexes.len(),
                ));
                if table.indexes.is_empty() {
                    root = root.child(self.empty(
                        "structure-no-indexes",
                        self.tr(
                            "이 스냅샷에 인덱스가 없습니다.",
                            "No indexes in this snapshot.",
                        ),
                    ));
                }
                for (ix, index) in table.indexes.iter().enumerate() {
                    let keys = index
                        .columns
                        .iter()
                        .chain(index.expressions.iter())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    let mut row = div()
                        .id(("structure-index", ix))
                        .py_3()
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Self::text(("structure-index-name", ix), index.name.clone())
                                        .font_weight(FontWeight::MEDIUM),
                                )
                                .child(Self::badge(index.method.clone(), false))
                                .when(index.unique, |el| {
                                    el.child(Self::badge("UNIQUE".into(), true))
                                })
                                .when(index.constraint_owned, |el| {
                                    el.child(Self::badge(
                                        self.tr("제약 조건", "Constraint").into(),
                                        false,
                                    ))
                                }),
                        )
                        .child(
                            Self::text(("structure-index-keys", ix), keys)
                                .mt_1()
                                .font_family("Menlo")
                                .text_size(px(11.))
                                .text_color(rgb(MUTED)),
                        );
                    if !index.include.is_empty() {
                        row = row.child(
                            Self::text(
                                ("structure-index-include", ix),
                                format!("INCLUDE ({})", index.include.join(", ")),
                            )
                            .mt_1()
                            .text_color(rgb(MUTED)),
                        );
                    }
                    if let Some(predicate) = &index.predicate {
                        row = row.child(
                            Self::text(
                                ("structure-index-predicate", ix),
                                format!("WHERE {predicate}"),
                            )
                            .mt_1()
                            .font_family("Menlo")
                            .text_size(px(11.))
                            .text_color(rgb(MUTED)),
                        );
                    }
                    root = root.child(row);
                }
                let constraint_count =
                    table.foreign_keys.len() + usize::from(table.primary_key.is_some());
                root = root.child(self.section(
                    "structure-constraints-heading",
                    self.tr("키 제약 조건", "Key constraints"),
                    constraint_count,
                ));
                if constraint_count == 0 {
                    root = root.child(self.empty(
                        "structure-no-constraints",
                        self.tr(
                            "이 스냅샷에 기본 키·외래 키가 없습니다.",
                            "No primary or foreign keys in this snapshot.",
                        ),
                    ));
                }
                if let Some(pk) = &table.primary_key {
                    root = root.child(
                        div()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(Self::badge("PK".into(), true))
                                    .child(pk.name.clone().unwrap_or_else(|| {
                                        self.tr("기본 키", "Primary key").into()
                                    })),
                            )
                            .child(
                                Self::text("structure-primary-key-columns", pk.columns.join(", "))
                                    .mt_1()
                                    .font_family("Menlo")
                                    .text_size(px(11.))
                                    .text_color(rgb(MUTED)),
                            ),
                    );
                }
                for (ix, fk) in table.foreign_keys.iter().enumerate() {
                    root = root.child(
                        div()
                            .id(("structure-fk", ix))
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(Self::badge("FK".into(), true))
                                    .child(fk.name.clone().unwrap_or_else(|| {
                                        self.tr("외래 키", "Foreign key").into()
                                    })),
                            )
                            .child(
                                Self::text(("structure-fk-source", ix), fk.columns.join(", "))
                                    .mt_1()
                                    .font_family("Menlo")
                                    .text_size(px(11.)),
                            )
                            .child(
                                Self::text(
                                    ("structure-fk-target", ix),
                                    format!(
                                        "→ {} ({})",
                                        fk.references.sql(),
                                        fk.referenced_columns.join(", ")
                                    ),
                                )
                                .mt_1()
                                .font_family("Menlo")
                                .text_size(px(11.))
                                .text_color(rgb(MUTED)),
                            )
                            .child(
                                Self::text(
                                    ("structure-fk-actions", ix),
                                    format!(
                                        "UPDATE {}  ·  DELETE {}",
                                        fk.on_update.sql(),
                                        fk.on_delete.sql()
                                    ),
                                )
                                .mt_1()
                                .text_size(px(10.))
                                .text_color(rgb(MUTED)),
                            ),
                    );
                }
                if !table.unsupported_features.is_empty() {
                    root = root.child(
                        Self::text(
                            "structure-metadata-limits",
                            format!(
                                "{}: {}",
                                self.tr("추가 메타데이터", "Additional metadata"),
                                table.unsupported_features.join(" · ")
                            ),
                        )
                        .mt_3()
                        .text_color(rgb(MUTED)),
                    );
                }
            }
            if let Some(view) = view {
                root = root
                    .child(self.section(
                        "structure-view-heading",
                        self.tr("뷰 정의", "View definition"),
                        1,
                    ))
                    .child(
                        div()
                            .id("structure-view-scroll")
                            .overflow_x_scroll()
                            .p_3()
                            .bg(rgb(BG))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .rounded(px(4.))
                            .child(
                                Self::text("structure-view-definition", view.definition.clone())
                                    .font_family("Menlo")
                                    .text_size(px(11.))
                                    .whitespace_nowrap(),
                            ),
                    );
            }
        }
        root
    }
}
