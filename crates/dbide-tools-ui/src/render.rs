use super::*;
use dbide_data::RowDifference;
use dbide_schema::explain::PlanNode;
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, Textarea},
};

const PANEL: u32 = 0x1b1e24;
const BORDER: u32 = 0x2b3039;
const TEXT: u32 = 0xdce1e9;
const MUTED: u32 = 0x8b95a5;
pub(crate) fn hint(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .text_color(rgb(MUTED))
        .child(text.into())
}
pub(crate) fn card(title: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(0x20242c))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title.into()))
}
impl ToolsPanel {
    pub(crate) fn button(&self, id: &'static str, label: &'static str) -> Button {
        Button::new(id)
            .label(self.t(label))
            .small()
            .disabled(self.operation.busy())
    }
    fn schema_view(&self, cx: &mut Context<Self>) -> Div {
        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(hint(self.t(
                "Capture a complete PostgreSQL catalog within 1–32 explicitly selected schemas.",
            )))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div().w(px(260.)).child(
                            Input::new(&self.scope)
                                .disabled(self.operation.busy())
                                .aria_label(self.t("Schema scope, comma separated"))
                                .small(),
                        ),
                    )
                    .child(
                        self.button("capture-schema", self.t("Capture schema"))
                            .disabled(self.operation.busy() || !self.live_enabled())
                            .primary()
                            .on_click(cx.listener(|s, _, _, cx| s.capture(cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        self.button("save-snapshot", self.t("Save snapshot JSON…"))
                            .disabled(self.operation.busy() || self.actual.is_none())
                            .on_click(cx.listener(|s, _, w, cx| s.save(SaveKind::Snapshot, w, cx))),
                    )
                    .child(
                        self.button("open-desired", self.t("Open desired JSON / DDL…"))
                            .on_click(cx.listener(|s, _, w, cx| s.open(OpenKind::Desired, w, cx))),
                    ),
            );
        if !self.live_enabled() {
            body = body.child(card(self.t("PostgreSQL-only live catalog")).child(self.t("Schema capture, object browsing/drafts and EXPLAIN require a PostgreSQL profile. This captured engine is not supported by these live workflows; offline desired-schema and data compare remain available.")));
        }
        if let Some(s) = &self.actual {
            let tables = s.tables().count();
            let columns = s.tables().map(|(_, t)| t.columns.len()).sum::<usize>();
            body = body.child(
                card(self.t("Captured actual"))
                    .child(self.f(
                        Message::ToolsSchemasTablesTablesColumnsColumns,
                        &[
                            ("value_a", (s.schemas.len()).to_string()),
                            ("tables", (tables).to_string()),
                            ("columns", (columns).to_string()),
                        ],
                    ))
                    .child(hint(self.f(
                        Message::ToolsScopeComplete,
                        &[
                            ("value_a", (s.scope.join(", ")).to_string()),
                            ("value_b", self.flag(s.complete)),
                        ],
                    )))
                    .child(hint(self.f(
                        Message::ToolsDatabase,
                        &[
                            ("value_a", (s.database.name).to_string()),
                            ("value_b", format!("{:?}", s.provenance)),
                        ],
                    ))),
            );
        }
        if let Some(s) = &self.desired {
            body = body.child(
                card(self.t("Desired schema"))
                    .child(self.f(
                        Message::ToolsTables,
                        &[
                            ("value_a", (s.database.name).to_string()),
                            ("value_b", (s.tables().count()).to_string()),
                        ],
                    ))
                    .child(hint(self.f(
                        Message::ToolsScope,
                        &[
                            ("value_a", (s.scope.join(", ")).to_string()),
                            ("value_b", format!("{:?}", s.provenance)),
                        ],
                    ))),
            );
        }
        body.child(hint(self.t("Catalog and plan requests have 15 s statement limits; parent profile changes do not retarget this panel. Files are processed in background workers. SQLite/MySQL live tools remain unavailable here.")))
    }
    fn migration_view(&self, cx: &mut Context<Self>) -> Div {
        let mut body=div().flex().flex_col().gap_3().child(card(self.t("Desired → captured actual")).child(self.t("Preview operations TO actual to make it match desired.")).child(hint(self.t("Preview only. No database apply action is exposed; exported SQL requires separate review and execution."))))
        .child(div().flex().gap_2().child(self.button("compare-schema",self.t("Compare schemas")).disabled(self.operation.busy()||self.desired.is_none()||self.actual.is_none()).primary().on_click(cx.listener(|s,_,_,cx|s.compare(cx)))).child(self.button("export-migration",self.t("Export SQL preview…")).disabled(self.operation.busy()||self.migration.is_none()).on_click(cx.listener(|s,_,w,cx|s.save(SaveKind::Sql,w,cx)))));
        if let Some(plan) = &self.migration {
            body = body.child(
                card(self.f(
                    Message::ToolsOperationsDestructiveUnsupported,
                    &[
                        ("value_a", (plan.operations.len()).to_string()),
                        ("value_b", self.flag(plan.is_destructive())),
                        ("value_c", self.flag(plan.has_unsupported())),
                    ],
                ))
                .child(hint(plan.direction.clone()))
                .child(hint(self.f(
                    Message::ToolsDesiredActualImplicitCommit,
                    &[
                        ("value_a", (plan.source_database.name).to_string()),
                        ("value_b", (plan.target_database.name).to_string()),
                        ("value_c", self.flag(plan.implicit_commit)),
                    ],
                ))),
            );
            for warning in &plan.warnings {
                body = body.child(hint(model::short(warning, 1000)));
            }
            body = body.child(
                div()
                    .id("migration-operations")
                    .role(Role::Table)
                    .aria_label(self.t("Migration operations, desired to actual"))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(plan.operations.iter().take(300).enumerate().map(|(i, op)| {
                        card(format!("{}. {:?} · {}", i + 1, op.kind, op.object.sql()))
                            .id(("tools-row", i))
                            .role(Role::Row)
                            .child(hint(self.f(
                                Message::ToolsDestructiveDataValidationBlocked,
                                &[
                                    ("value_a", self.flag(op.destructive)),
                                    ("value_b", self.flag(op.requires_data_validation)),
                                    ("value_c", self.flag(op.unsupported)),
                                ],
                            )))
                            .child(model::short(&op.detail, 1200))
                            .child(
                                div()
                                    .font_family("Menlo")
                                    .text_size(px(11.))
                                    .child(model::short(
                                        op.sql
                                            .as_deref()
                                            .unwrap_or("-- BLOCKED: unsupported operation"),
                                        4000,
                                    )),
                            )
                    })),
            );
            if plan.operations.len() > 300 {
                body = body.child(hint(self.t(
                    "Preview capped at 300 operations. Export contains the complete bounded plan.",
                )));
            }
        } else {
            body = body.child(hint(self.t(
                "Capture actual catalog and load desired schema in Schema tab, then compare.",
            )));
        }
        body
    }
    fn relationship_view(&self, cx: &mut Context<Self>) -> Div {
        let mut body = div().flex().flex_col().gap_3().child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    self.button("export-er-svg", self.t("Export ER SVG…"))
                        .disabled(self.operation.busy() || self.actual.is_none())
                        .on_click(cx.listener(|s, _, w, cx| s.save(SaveKind::Svg, w, cx))),
                )
                .child(hint(
                    self.t("80 tables · 24 columns/table · 200 edges maximum"),
                )),
        );
        match &self.er {
            Some(Ok((graph,svg_data)))=>{
                body=body.child(card(self.f(Message::ToolsTablesForeignKeys, &[("value_a", (graph.nodes.len()).to_string()), ("value_b", (graph.edges.len()).to_string())])).child(hint(self.t("Foreign keys from captured catalog. No inferred relationships. SVG preview and accessible table share the same bounded graph."))).child(div().h(px(320.)).w_full().bg(rgb(0xf7f8fa)).child(svg().data(svg_data.as_bytes()).size_full())));
                for warning in &graph.warnings{body=body.child(hint(warning.clone()));}
                body=body.child(div().id("er-relationships").role(Role::Table).aria_label(self.t("Foreign key relationships")).flex().flex_col().gap_2().children(graph.edges.iter().enumerate().map(|(i,e)|{let name=|id:&str|graph.nodes.iter().find(|n|n.id==id).map(|n|n.name.sql()).unwrap_or_else(||self.t("Outside displayed scope").into());card(format!("{}. {} → {}",i+1,name(&e.from),name(&e.to))).id(("tools-row",i)).role(Role::Row).child(hint(format!("{} · {}",e.constraint.as_deref().unwrap_or(self.t("unnamed foreign key")),e.column_pairs.iter().map(|(a,b)|format!("{a} → {b}")).collect::<Vec<_>>().join(", ")))).child(hint(self.f(Message::ToolsTargetInScopeColumnEndpointsVisible, &[("value_a", self.flag(e.target_in_scope)), ("value_b", self.flag(e.column_endpoints_visible))])))})));
                body=body.children(graph.nodes.iter().map(|node|card(node.name.sql()).child(hint(node.columns.iter().map(|c|format!("{}{}: {}{}",if c.primary_key{"PK "}else{""},c.name,c.data_type,if c.nullable{" ?"}else{""})).collect::<Vec<_>>().join("   |   ")))));
            },Some(Err(e))=>body=body.child(hint(self.f(Message::ToolsERGenerationFailedE, &[("e", (e).to_string())]))),None=>body=body.child(hint(self.t("Capture a schema first. Relationships and SVG will be generated from that exact snapshot.")))
        }
        body
    }
    fn explain_view(&self, cx: &mut Context<Self>) -> Div {
        let mut body=div().flex().flex_col().gap_3().child(card(self.t("Plan only · no ANALYZE")).child(self.t("Exactly one parsed SQL statement, planned in a dedicated read-only transaction.")).child(hint(self.t("Estimated costs/rows are planner estimates. Planning may take locks; statement and lock timeouts are enforced."))))
        .child(Textarea::new(&self.sql).aria_label(self.t("Exactly one SQL statement for plan-only EXPLAIN")).h(px(90.)))
        .child(self.button("run-plan",self.t("EXPLAIN plan only")).disabled(self.operation.busy()||!self.live_enabled()).primary().on_click(cx.listener(|s,_,_,cx|s.explain(cx))));
        if !self.live_enabled() {
            body=body.child(card(self.t("EXPLAIN unavailable for captured engine")).child(self.t("PostgreSQL plan-only workflow is disabled. Reopen tools with a PostgreSQL profile to request a live plan.")));
        }
        if let Some(doc) = &self.explain {
            body = body.child(
                card(self.t("SQL captured for this plan"))
                    .child(hint(model::short(&self.explained_sql, 1200))),
            );
            if self.sql.read(cx).value().as_str() != self.explained_sql {
                body = body.child(hint(self.t("SQL input changed; displayed plan belongs to the captured SQL above. Run EXPLAIN again to refresh.")));
            }
            for warning in &doc.warnings {
                body = body.child(hint(warning.clone()));
            }
            body = body.child(hint(
                self.t("Expand or collapse nodes · preview capped at 300 visible nodes."),
            ));
            let mut rows = vec![];
            for (i, stmt) in doc.statements.iter().enumerate() {
                self.plan_rows(&stmt.plan, format!("{i}"), 0, &mut rows, cx);
            }
            body = body.child(
                div()
                    .id("explain-tree")
                    .role(Role::Tree)
                    .aria_label(self.t("Estimated query plan tree"))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(rows),
            );
        }
        body
    }
    fn plan_rows(
        &self,
        node: &PlanNode,
        path: String,
        depth: usize,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        if rows.len() >= 300 {
            return;
        }
        let collapsed = self.collapsed.contains(&path);
        let key = path.clone();
        let row = div()
            .id(SharedString::from(format!("tree-item-{path}")))
            .role(Role::TreeItem)
            .aria_label(self.f(
                Message::ToolsEstimatedCostEstimatedRows,
                &[
                    ("value_a", (node.node_type).to_string()),
                    ("value_b", (number(node.total_cost)).to_string()),
                    ("value_c", (number(node.plan_rows)).to_string()),
                ],
            ))
            .flex()
            .items_center()
            .gap_2()
            .pl(px((depth.min(20) * 16) as f32))
            .py_1()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                Button::new(SharedString::from(format!("plan-{path}")))
                    .label(format!(
                        "{} {}{}",
                        if node.children.is_empty() {
                            "·"
                        } else if collapsed {
                            "▸"
                        } else {
                            "▾"
                        },
                        node.node_type,
                        node.relation_name
                            .as_ref()
                            .map(|n| format!(" · {n}"))
                            .unwrap_or_default()
                    ))
                    .small()
                    .on_click(cx.listener(move |s, _, _, cx| {
                        if !s.collapsed.remove(&key) {
                            s.collapsed.insert(key.clone());
                        }
                        cx.notify();
                    })),
            )
            .child(hint(self.f(
                Message::ToolsCostRowsWidth,
                &[
                    ("value_a", (number(node.startup_cost)).to_string()),
                    ("value_b", (number(node.total_cost)).to_string()),
                    ("value_c", (number(node.plan_rows)).to_string()),
                    ("value_d", (number(node.plan_width)).to_string()),
                ],
            )));
        rows.push(row.into_any_element());
        if !collapsed {
            for (i, child) in node.children.iter().enumerate() {
                self.plan_rows(child, format!("{path}.{i}"), depth + 1, rows, cx);
            }
        }
    }
    fn data_view(&self, cx: &mut Context<Self>) -> Div {
        let mut body=div().flex().flex_col().gap_3().child(hint(self.t("Complete tagged JSON inputs only · exact cell kinds and values · 10,000 rows / 16 MiB per input. Before → after; no positional or guessed identity.")))
        .child(div().flex().gap_2().child(self.button("open-before",self.t("Open before JSON…")).on_click(cx.listener(|s,_,w,cx|s.open(OpenKind::Before,w,cx)))).child(self.button("open-after",self.t("Open after JSON…")).on_click(cx.listener(|s,_,w,cx|s.open(OpenKind::After,w,cx)))))
        .child(card(self.t("Comparison inputs")).child(self.f(Message::ToolsBeforeRows, &[("value_a", if self.before.is_some(){self.before_label.clone()}else{self.t("Not loaded").to_owned()}), ("value_b", (self.before.as_ref().map_or(0,|d|d.rows.len())).to_string())])).child(self.f(Message::ToolsAfterRows, &[("value_a", if self.after.is_some(){self.after_label.clone()}else{self.t("Not loaded").to_owned()}), ("value_b", (self.after.as_ref().map_or(0,|d|d.rows.len())).to_string())])))
        .child(Input::new(&self.keys).aria_label(self.t("Explicit key column names, comma separated")).small())
        .child(self.button("compare-data",self.t("Compare by explicit keys")).disabled(self.operation.busy()||self.before.is_none()||self.after.is_none()).primary().on_click(cx.listener(|s,_,_,cx|s.data_compare(cx))));
        if let Some(before) = &self.before {
            body = body.child(hint(
                self.f(
                    Message::ToolsAvailableColumns,
                    &[(
                        "value_a",
                        (before
                            .columns
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", "))
                        .to_string(),
                    )],
                ),
            ));
        }
        if let Some(diff) = &self.data_diff {
            let counts = diff.differences.iter().fold([0; 3], |mut n, d| {
                n[match d {
                    RowDifference::Insert { .. } => 0,
                    RowDifference::Delete { .. } => 1,
                    RowDifference::Change { .. } => 2,
                }] += 1;
                n
            });
            body = body
                .child(
                    card(self.f(
                        Message::ToolsInsertedDeletedChangedUnchanged,
                        &[
                            ("value_a", (counts[0]).to_string()),
                            ("value_b", (counts[1]).to_string()),
                            ("value_c", (counts[2]).to_string()),
                            ("value_d", (diff.unchanged).to_string()),
                        ],
                    ))
                    .child(hint(self.f(
                        Message::ToolsComparedCompleteInputsRowsKeyColumn,
                        &[
                            ("value_a", (diff.compared_before).to_string()),
                            ("value_b", (diff.compared_after).to_string()),
                            ("value_c", format!("{:?}", diff.key_columns)),
                        ],
                    ))),
                )
                .child(
                    div()
                        .id("data-differences")
                        .role(Role::Table)
                        .aria_label(self.t("Exact typed row differences, before to after"))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(diff.differences.iter().take(100).enumerate().map(|(i, d)| {
                            let (kind, key, before, after) = match d {
                                RowDifference::Insert { key, after } => (
                                    dbide_i18n::tr(self.locale, Message::ToolsDataInsert),
                                    key,
                                    None,
                                    Some(after),
                                ),
                                RowDifference::Delete { key, before } => (
                                    dbide_i18n::tr(self.locale, Message::ToolsDataDelete),
                                    key,
                                    Some(before),
                                    None,
                                ),
                                RowDifference::Change {
                                    key, before, after, ..
                                } => (
                                    dbide_i18n::tr(self.locale, Message::ToolsDataChange),
                                    key,
                                    Some(before),
                                    Some(after),
                                ),
                            };
                            card(
                                self.f(
                                    Message::ToolsKey,
                                    &[
                                        ("value_a", (i + 1).to_string()),
                                        ("value_b", (kind).to_string()),
                                        (
                                            "value_c",
                                            (model::short(
                                                &serde_json::to_string(key).unwrap_or_default(),
                                                400,
                                            ))
                                            .to_string(),
                                        ),
                                    ],
                                ),
                            )
                            .id(("tools-row", i))
                            .role(Role::Row)
                            .when_some(before, |el, b| {
                                el.child(hint(
                                    self.f(
                                        Message::ToolsBefore,
                                        &[(
                                            "value_a",
                                            (model::short(
                                                &serde_json::to_string(b).unwrap_or_default(),
                                                1500,
                                            ))
                                            .to_string(),
                                        )],
                                    ),
                                ))
                            })
                            .when_some(after, |el, a| {
                                el.child(hint(
                                    self.f(
                                        Message::ToolsAfter,
                                        &[(
                                            "value_a",
                                            (model::short(
                                                &serde_json::to_string(a).unwrap_or_default(),
                                                1500,
                                            ))
                                            .to_string(),
                                        )],
                                    ),
                                ))
                            })
                        })),
                );
            if diff.differences.len() > 100 {
                body = body.child(hint(self.t(
                    "Row preview capped at 100 changes; summary counts cover all loaded rows.",
                )));
            }
        }
        body
    }
}
fn number(value: Option<f64>) -> String {
    value
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "—".into())
}
impl Render for ToolsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.tab {
            Tab::Schema => self.schema_view(cx),
            Tab::Objects => self.objects_view(cx),
            Tab::Migration => self.migration_view(cx),
            Tab::Relationships => self.relationship_view(cx),
            Tab::Explain => self.explain_view(cx),
            Tab::Data => self.data_view(cx),
        };
        div()
            .id("database-tools-panel")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .text_color(rgb(TEXT))
            .text_size(px(12.))
            .child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.t("Database tools")),
                    )
                    .child(hint(self.label.clone()))
                    .child(hint(self.t(
                        "Connection fixed at panel open · dedicated read-only sessions",
                    ))),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .children(Tab::ALL.into_iter().enumerate().map(|(i, (tab, label))| {
                        Button::new(("tools-tab", i))
                            .label(self.t(label))
                            .small()
                            .when(self.tab == tab, |b| b.primary())
                            .on_click(cx.listener(move |s, _, _, cx| {
                                s.tab = tab;
                                cx.notify();
                            }))
                    })),
            )
            .child(
                div()
                    .id("tools-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(content),
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .id("tools-error")
                        .role(Role::Alert)
                        .aria_label(self.localized_error(&error))
                        .px_3()
                        .py_2()
                        .bg(rgb(0x302522))
                        .text_color(rgb(0xf0b797))
                        .child(model::short(&self.localized_error(&error), 2000)),
                )
            })
            .child(
                div()
                    .id("tools-status")
                    .role(Role::Status)
                    .aria_label(self.t(&self.status).to_owned())
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(hint(self.t(&self.status).to_owned()))
                    .when(self.operation.busy(), |el| {
                        el.child(
                            Button::new("cancel-tools")
                                .label(self.t("Cancel"))
                                .small()
                                .on_click(cx.listener(|s, _, _, cx| s.cancel(cx))),
                        )
                    }),
            )
    }
}
