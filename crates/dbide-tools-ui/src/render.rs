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
            .label(label)
            .small()
            .disabled(self.operation.busy())
    }
    fn schema_view(&self, cx: &mut Context<Self>) -> Div {
        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(hint(
                "Capture a complete PostgreSQL catalog within 1–32 explicitly selected schemas.",
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div().w(px(260.)).child(
                            Input::new(&self.scope)
                                .disabled(self.operation.busy())
                                .aria_label("Schema scope, comma separated")
                                .small(),
                        ),
                    )
                    .child(
                        self.button("capture-schema", "Capture schema")
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
                        self.button("save-snapshot", "Save snapshot JSON…")
                            .disabled(self.operation.busy() || self.actual.is_none())
                            .on_click(cx.listener(|s, _, w, cx| s.save(SaveKind::Snapshot, w, cx))),
                    )
                    .child(
                        self.button("open-desired", "Open desired JSON / DDL…")
                            .on_click(cx.listener(|s, _, w, cx| s.open(OpenKind::Desired, w, cx))),
                    ),
            );
        if !self.live_enabled() {
            body = body.child(card("PostgreSQL-only live catalog").child("Schema capture, object browsing/drafts and EXPLAIN require a PostgreSQL profile. This captured engine is not supported by these live workflows; offline desired-schema and data compare remain available."));
        }
        if let Some(s) = &self.actual {
            let tables = s.tables().count();
            let columns = s.tables().map(|(_, t)| t.columns.len()).sum::<usize>();
            body = body.child(
                card("Captured actual")
                    .child(format!(
                        "{} schemas · {tables} tables · {columns} columns",
                        s.schemas.len()
                    ))
                    .child(hint(format!(
                        "Scope: {} · complete: {}",
                        s.scope.join(", "),
                        s.complete
                    )))
                    .child(hint(format!(
                        "Database: {} · {:?}",
                        s.database.name, s.provenance
                    ))),
            );
        }
        if let Some(s) = &self.desired {
            body = body.child(
                card("Desired schema")
                    .child(format!(
                        "{} · {} tables",
                        s.database.name,
                        s.tables().count()
                    ))
                    .child(hint(format!(
                        "Scope: {} · {:?}",
                        s.scope.join(", "),
                        s.provenance
                    ))),
            );
        }
        body.child(hint("Catalog and plan requests have 15 s statement limits; parent profile changes do not retarget this panel. Files are processed in background workers. SQLite/MySQL live tools remain unavailable here."))
    }
    fn migration_view(&self, cx: &mut Context<Self>) -> Div {
        let mut body=div().flex().flex_col().gap_3().child(card("Desired → captured actual").child("Preview operations TO actual to make it match desired.").child(hint("Preview only. No database apply action is exposed; exported SQL requires separate review and execution.")))
        .child(div().flex().gap_2().child(self.button("compare-schema","Compare schemas").disabled(self.operation.busy()||self.desired.is_none()||self.actual.is_none()).primary().on_click(cx.listener(|s,_,_,cx|s.compare(cx)))).child(self.button("export-migration","Export SQL preview…").disabled(self.operation.busy()||self.migration.is_none()).on_click(cx.listener(|s,_,w,cx|s.save(SaveKind::Sql,w,cx)))));
        if let Some(plan) = &self.migration {
            body = body.child(
                card(format!(
                    "{} operations · destructive: {} · unsupported: {}",
                    plan.operations.len(),
                    plan.is_destructive(),
                    plan.has_unsupported()
                ))
                .child(hint(plan.direction.clone()))
                .child(hint(format!(
                    "Desired: {} → Actual: {} · implicit commit: {}",
                    plan.source_database.name, plan.target_database.name, plan.implicit_commit
                ))),
            );
            for warning in &plan.warnings {
                body = body.child(hint(model::short(warning, 1000)));
            }
            body = body.child(
                div()
                    .id("migration-operations")
                    .role(Role::Table)
                    .aria_label("Migration operations, desired to actual")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(plan.operations.iter().take(300).enumerate().map(|(i, op)| {
                        card(format!("{}. {:?} · {}", i + 1, op.kind, op.object.sql()))
                            .id(("tools-row", i))
                            .role(Role::Row)
                            .child(hint(format!(
                                "Destructive: {} · data validation: {} · blocked: {}",
                                op.destructive, op.requires_data_validation, op.unsupported
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
                body = body.child(hint(
                    "Preview capped at 300 operations. Export contains the complete bounded plan.",
                ));
            }
        } else {
            body = body.child(hint(
                "Capture actual catalog and load desired schema in Schema tab, then compare.",
            ));
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
                    self.button("export-er-svg", "Export ER SVG…")
                        .disabled(self.operation.busy() || self.actual.is_none())
                        .on_click(cx.listener(|s, _, w, cx| s.save(SaveKind::Svg, w, cx))),
                )
                .child(hint("80 tables · 24 columns/table · 200 edges maximum")),
        );
        match &self.er {
            Some(Ok((graph,svg_data)))=>{
                body=body.child(card(format!("{} tables · {} foreign keys",graph.nodes.len(),graph.edges.len())).child(hint("Foreign keys from captured catalog. No inferred relationships. SVG preview and accessible table share the same bounded graph.")).child(div().h(px(320.)).w_full().bg(rgb(0xf7f8fa)).child(svg().data(svg_data.as_bytes()).size_full())));
                for warning in &graph.warnings{body=body.child(hint(warning.clone()));}
                body=body.child(div().id("er-relationships").role(Role::Table).aria_label("Foreign key relationships").flex().flex_col().gap_2().children(graph.edges.iter().enumerate().map(|(i,e)|{let name=|id:&str|graph.nodes.iter().find(|n|n.id==id).map(|n|n.name.sql()).unwrap_or_else(||"Outside displayed scope".into());card(format!("{}. {} → {}",i+1,name(&e.from),name(&e.to))).id(("tools-row",i)).role(Role::Row).child(hint(format!("{} · {}",e.constraint.as_deref().unwrap_or("unnamed foreign key"),e.column_pairs.iter().map(|(a,b)|format!("{a} → {b}")).collect::<Vec<_>>().join(", ")))).child(hint(format!("Target in scope: {} · column endpoints visible: {}",e.target_in_scope,e.column_endpoints_visible)))})));
                body=body.children(graph.nodes.iter().map(|node|card(node.name.sql()).child(hint(node.columns.iter().map(|c|format!("{}{}: {}{}",if c.primary_key{"PK "}else{""},c.name,c.data_type,if c.nullable{" ?"}else{""})).collect::<Vec<_>>().join("   |   ")))));
            },Some(Err(e))=>body=body.child(hint(format!("ER generation failed: {e}"))),None=>body=body.child(hint("Capture a schema first. Relationships and SVG will be generated from that exact snapshot."))
        }
        body
    }
    fn explain_view(&self, cx: &mut Context<Self>) -> Div {
        let mut body=div().flex().flex_col().gap_3().child(card("Plan only · no ANALYZE").child("Exactly one parsed SQL statement, planned in a dedicated read-only transaction.").child(hint("Estimated costs/rows are planner estimates. Planning may take locks; statement and lock timeouts are enforced.")))
        .child(Textarea::new(&self.sql).aria_label("Exactly one SQL statement for plan-only EXPLAIN").h(px(90.)))
        .child(self.button("run-plan","EXPLAIN plan only").disabled(self.operation.busy()||!self.live_enabled()).primary().on_click(cx.listener(|s,_,_,cx|s.explain(cx))));
        if !self.live_enabled() {
            body=body.child(card("EXPLAIN unavailable for captured engine").child("PostgreSQL plan-only workflow is disabled. Reopen tools with a PostgreSQL profile to request a live plan."));
        }
        if let Some(doc) = &self.explain {
            body = body.child(
                card("SQL captured for this plan")
                    .child(hint(model::short(&self.explained_sql, 1200))),
            );
            if self.sql.read(cx).value().as_str() != self.explained_sql {
                body = body.child(hint("SQL input changed; displayed plan belongs to the captured SQL above. Run EXPLAIN again to refresh."));
            }
            for warning in &doc.warnings {
                body = body.child(hint(warning.clone()));
            }
            body = body.child(hint(
                "Expand or collapse nodes · preview capped at 300 visible nodes.",
            ));
            let mut rows = vec![];
            for (i, stmt) in doc.statements.iter().enumerate() {
                self.plan_rows(&stmt.plan, format!("{i}"), 0, &mut rows, cx);
            }
            body = body.child(
                div()
                    .id("explain-tree")
                    .role(Role::Tree)
                    .aria_label("Estimated query plan tree")
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
            .aria_label(format!(
                "{} · estimated cost {} · estimated rows {}",
                node.node_type,
                number(node.total_cost),
                number(node.plan_rows)
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
            .child(hint(format!(
                "cost {}…{} · rows {} · width {}",
                number(node.startup_cost),
                number(node.total_cost),
                number(node.plan_rows),
                number(node.plan_width)
            )));
        rows.push(row.into_any_element());
        if !collapsed {
            for (i, child) in node.children.iter().enumerate() {
                self.plan_rows(child, format!("{path}.{i}"), depth + 1, rows, cx);
            }
        }
    }
    fn data_view(&self, cx: &mut Context<Self>) -> Div {
        let mut body=div().flex().flex_col().gap_3().child(hint("Complete tagged JSON inputs only · exact cell kinds and values · 10,000 rows / 16 MiB per input. Before → after; no positional or guessed identity."))
        .child(div().flex().gap_2().child(self.button("open-before","Open before JSON…").on_click(cx.listener(|s,_,w,cx|s.open(OpenKind::Before,w,cx)))).child(self.button("open-after","Open after JSON…").on_click(cx.listener(|s,_,w,cx|s.open(OpenKind::After,w,cx)))))
        .child(card("Comparison inputs").child(format!("Before: {} · {} rows",self.before_label,self.before.as_ref().map_or(0,|d|d.rows.len()))).child(format!("After: {} · {} rows",self.after_label,self.after.as_ref().map_or(0,|d|d.rows.len()))))
        .child(Input::new(&self.keys).aria_label("Explicit key column names, comma separated").small())
        .child(self.button("compare-data","Compare by explicit keys").disabled(self.operation.busy()||self.before.is_none()||self.after.is_none()).primary().on_click(cx.listener(|s,_,_,cx|s.data_compare(cx))));
        if let Some(before) = &self.before {
            body = body.child(hint(format!(
                "Available columns: {}",
                before
                    .columns
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
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
                    card(format!(
                        "{} inserted · {} deleted · {} changed · {} unchanged",
                        counts[0], counts[1], counts[2], diff.unchanged
                    ))
                    .child(hint(format!(
                        "Compared complete inputs: {} → {} rows · key column indices: {:?}",
                        diff.compared_before, diff.compared_after, diff.key_columns
                    ))),
                )
                .child(
                    div()
                        .id("data-differences")
                        .role(Role::Table)
                        .aria_label("Exact typed row differences, before to after")
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(diff.differences.iter().take(100).enumerate().map(|(i, d)| {
                            let (kind, key, before, after) = match d {
                                RowDifference::Insert { key, after } => {
                                    ("INSERT", key, None, Some(after))
                                }
                                RowDifference::Delete { key, before } => {
                                    ("DELETE", key, Some(before), None)
                                }
                                RowDifference::Change {
                                    key, before, after, ..
                                } => ("CHANGE", key, Some(before), Some(after)),
                            };
                            card(format!(
                                "{}. {} · key {}",
                                i + 1,
                                kind,
                                model::short(&serde_json::to_string(key).unwrap_or_default(), 400)
                            ))
                            .id(("tools-row", i))
                            .role(Role::Row)
                            .when_some(before, |el, b| {
                                el.child(hint(format!(
                                    "Before: {}",
                                    model::short(
                                        &serde_json::to_string(b).unwrap_or_default(),
                                        1500
                                    )
                                )))
                            })
                            .when_some(after, |el, a| {
                                el.child(hint(format!(
                                    "After: {}",
                                    model::short(
                                        &serde_json::to_string(a).unwrap_or_default(),
                                        1500
                                    )
                                )))
                            })
                        })),
                );
            if diff.differences.len() > 100 {
                body = body.child(hint(
                    "Row preview capped at 100 changes; summary counts cover all loaded rows.",
                ));
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
                            .child("Database tools"),
                    )
                    .child(hint(self.label.clone()))
                    .child(hint(
                        "Connection fixed at panel open · dedicated read-only sessions",
                    )),
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
                            .label(label)
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
                        .aria_label(error.clone())
                        .px_3()
                        .py_2()
                        .bg(rgb(0x302522))
                        .text_color(rgb(0xf0b797))
                        .child(model::short(&error, 2000)),
                )
            })
            .child(
                div()
                    .id("tools-status")
                    .role(Role::Status)
                    .aria_label(self.status.clone())
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(hint(self.status.clone()))
                    .when(self.operation.busy(), |el| {
                        el.child(
                            Button::new("cancel-tools")
                                .label("Cancel")
                                .small()
                                .on_click(cx.listener(|s, _, _, cx| s.cancel(cx))),
                        )
                    }),
            )
    }
}
