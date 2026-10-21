use super::*;
use crate::{
    drafts::{Selection, SqlDraft},
    render::{card, hint},
};
use dbide_schema::QualifiedName;
use gpui_component::{
    Disableable, Sizable,
    button::Button,
    input::{Input, Textarea},
};

#[derive(Clone, Copy)]
enum DraftAction {
    CreateIndex,
    CreateView,
    ReplaceView,
    DropView,
}
impl ToolsPanel {
    fn select_object(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        if self.operation.busy() {
            return;
        }
        let name = selection.name();
        self.object_schema.update(cx, |input, cx| {
            input.set_value(name.schema.clone(), window, cx)
        });
        self.object_name.update(cx, |input, cx| {
            input.set_value(name.name.clone(), window, cx)
        });
        if let Selection::View(name) = &selection
            && let Some(value) = self
                .actual
                .as_ref()
                .and_then(|s| drafts::view(s, name).ok())
        {
            let definition = value.definition.clone();
            self.view_query
                .update(cx, |input, cx| input.set_value(definition, window, cx));
        }
        self.selected_object = Some(selection);
        self.object_draft = None;
        self.error = None;
        cx.notify();
    }
    fn generate_draft(&mut self, action: DraftAction, cx: &mut Context<Self>) {
        let Some(snapshot) = self.actual.clone().filter(|_| self.live_enabled()) else {
            self.fail(
                "Capture a PostgreSQL schema before generating object drafts",
                cx,
            );
            return;
        };
        let selection = self.selected_object.clone();
        let schema = self.object_schema.read(cx).value().to_string();
        let name = self.object_name.read(cx).value().to_string();
        let query = self.view_query.read(cx).value().to_string();
        let index_name = self.index_name.read(cx).value().to_string();
        let columns = self.index_columns.read(cx).value().to_string();
        let unique = self.unique_index;
        let Some((id, cancel)) = self.begin("Validating exactly one object SQL draft…", cx)
        else {
            return;
        };
        self.object_draft = None;
        self.run(
            id,
            cancel,
            async move {
                let result = match action {
                    DraftAction::CreateIndex => match selection {
                        Some(Selection::Table(target)) => {
                            drafts::create_index(&snapshot, &target, &index_name, &columns, unique)
                        }
                        _ => Err("Select a captured table for the index draft".into()),
                    },
                    DraftAction::CreateView => {
                        drafts::create_view(&snapshot, &schema, &name, &query, false)
                    }
                    DraftAction::ReplaceView => {
                        drafts::create_view(&snapshot, &schema, &name, &query, true)
                    }
                    DraftAction::DropView => match selection {
                        Some(Selection::View(target)) => drafts::drop_view(&snapshot, &target),
                        _ => Err("Select a captured ordinary view to draft its removal".into()),
                    },
                };
                result.map(Completed::Draft)
            },
            cx,
        );
    }
    fn drop_index_draft(
        &mut self,
        target: QualifiedName,
        index_name: String,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.actual.clone().filter(|_| self.live_enabled()) else {
            self.fail("Capture a PostgreSQL schema first", cx);
            return;
        };
        let Some((id, cancel)) = self.begin("Validating destructive DROP INDEX draft…", cx)
        else {
            return;
        };
        self.object_draft = None;
        self.run(
            id,
            cancel,
            async move {
                let table = drafts::table(&snapshot, &target)?;
                let index = table
                    .indexes
                    .iter()
                    .find(|i| i.name == index_name)
                    .ok_or_else(|| "Index not present in captured table".to_string())?;
                drafts::drop_index(&target.schema, index).map(Completed::Draft)
            },
            cx,
        );
    }
    fn open_draft_console(&mut self, cx: &mut Context<Self>) {
        if self.operation.busy() {
            return;
        }
        if let (Some(request), Some(draft)) = (&self.request, &self.object_draft) {
            cx.emit(ToolsEvent::OpenSqlDraft {
                profile: request.profile.clone(),
                sql: draft.sql.clone(),
            });
            self.status = "SQL draft sent to a new console for review · not executed".into();
            cx.notify();
        }
    }
    pub(crate) fn objects_view(&self, cx: &mut Context<Self>) -> Div {
        let mut body=div().flex().flex_col().gap_3().child(card("Captured PostgreSQL objects").child("Browse actual table/view metadata, then generate one SQL statement for review.").child(hint("Drafts never execute here. Copy, save or open a new console with the captured profile. Run remains an explicit workbench action.")));
        let Some(snapshot) = &self.actual else {
            return body.child(hint(if self.live_enabled(){"Capture schema in Schema tab to load actual objects."}else{"Live object capture and PostgreSQL object drafts are disabled for this captured engine. Reopen tools with a PostgreSQL profile."}));
        };
        let total_tables = snapshot.tables().count();
        let total_views = snapshot
            .schemas
            .iter()
            .map(|s| s.views.len())
            .sum::<usize>();
        body = body.child(hint(format!(
            "{total_tables} tables · {total_views} views · captured scope {}",
            snapshot.scope.join(", ")
        )));
        let mut objects = div().flex().flex_wrap().gap_1();
        let mut count = 0usize;
        for schema in &snapshot.schemas {
            for table in &schema.tables {
                let target = QualifiedName::new(&schema.name, &table.name);
                let selection = Selection::Table(target.clone());
                let selected = self.selected_object.as_ref() == Some(&selection);
                objects = objects.child(
                    Button::new(("select-table", count))
                        .label(format!(
                            "Table {}{}",
                            target.sql(),
                            if selected { " ●" } else { "" }
                        ))
                        .small()
                        .disabled(self.operation.busy())
                        .on_click(cx.listener(move |s, _, w, cx| {
                            s.select_object(selection.clone(), w, cx)
                        })),
                );
                count += 1;
            }
            for view in &schema.views {
                let target = QualifiedName::new(&schema.name, &view.name);
                let selection = Selection::View(target.clone());
                objects = objects.child(
                    Button::new(("select-view", count))
                        .label(format!(
                            "{} {}",
                            if view.materialized {
                                "Materialized view"
                            } else {
                                "View"
                            },
                            target.sql()
                        ))
                        .small()
                        .disabled(self.operation.busy())
                        .on_click(cx.listener(move |s, _, w, cx| {
                            s.select_object(selection.clone(), w, cx)
                        })),
                );
                count += 1;
            }
            if schema.unsupported_objects.len() > 100 {
                body=body.child(hint(format!("{} unsupported objects in {} · showing first 100 markers; full details in snapshot JSON",schema.unsupported_objects.len(),schema.name)));
            }
            for marker in schema.unsupported_objects.iter().take(100) {
                body = body.child(hint(format!(
                    "Unsupported catalog object in {}: {}",
                    schema.name,
                    model::short(marker, 500)
                )));
            }
        }
        body = body.child(objects);
        if let Some(Selection::Table(target)) = &self.selected_object
            && let Ok(table) = drafts::table(snapshot, target)
        {
            body = body.child(card(target.sql()).child(hint(format!(
                "{} columns · {} foreign keys · {} indexes",
                table.columns.len(),
                table.foreign_keys.len(),
                table.indexes.len()
            ))));
            for feature in &table.unsupported_features {
                body = body.child(hint(format!(
                    "Unsupported table feature: {}",
                    model::short(feature, 800)
                )));
            }
            body = body.child(
                div()
                    .id("object-columns")
                    .role(Role::Table)
                    .aria_label("Captured columns and defaults")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(
                        table
                            .columns
                            .iter()
                            .take(300)
                            .enumerate()
                            .map(|(i, column)| {
                                card(format!("{} · {}", column.name, column.data_type))
                                    .id(("object-column", i))
                                    .role(Role::Row)
                                    .child(hint(format!(
                                        "Nullable: {} · default: {} · generated/identity: {}",
                                        column.nullable,
                                        model::short(
                                            column.default.as_deref().unwrap_or("none"),
                                            1200
                                        ),
                                        model::short(
                                            column.generated.as_deref().unwrap_or("none"),
                                            1200
                                        )
                                    )))
                            }),
                    ),
            );
            if table.columns.len() > 300 {
                body=body.child(hint("Column preview capped at 300. Full bounded capture remains available in snapshot JSON."));
            }
            if let Some(pk) = &table.primary_key {
                body = body.child(
                    card(format!(
                        "Primary key · {}",
                        pk.name.as_deref().unwrap_or("unnamed")
                    ))
                    .child(hint(pk.columns.join(", "))),
                );
            }
            for fk in table.foreign_keys.iter().take(200) {
                body = body.child(
                    card(format!(
                        "Foreign key · {}",
                        fk.name.as_deref().unwrap_or("unnamed")
                    ))
                    .child(hint(format!(
                        "{} → {} ({}) · DELETE {} · UPDATE {} · deferrable {}",
                        fk.columns.join(", "),
                        fk.references.sql(),
                        fk.referenced_columns.join(", "),
                        fk.on_delete.sql(),
                        fk.on_update.sql(),
                        fk.deferrable
                    ))),
                );
            }
            for (i, index) in table.indexes.iter().take(200).enumerate() {
                let name = index.name.clone();
                let target = target.clone();
                let advanced = !index.unsupported_features.is_empty()
                    || !index.expressions.is_empty()
                    || index.predicate.is_some()
                    || !index.include.is_empty()
                    || index.method != "btree";
                let mut item = card(format!(
                    "Index {} · {}{}",
                    index.name,
                    if index.unique { "UNIQUE " } else { "" },
                    index.method
                ))
                .child(hint(format!(
                    "Columns: {} · expressions: {} · INCLUDE: {} · predicate: {}",
                    index.columns.join(", "),
                    model::short(&index.expressions.join(", "), 2000),
                    index.include.join(", "),
                    model::short(index.predicate.as_deref().unwrap_or("none"), 2000)
                )))
                .child(hint(format!(
                    "Constraint owned: {} · unsupported metadata: {}",
                    index.constraint_owned,
                    index.unsupported_features.join(", ")
                )));
                if advanced {
                    item=item.child(hint("Advanced definition shown from captured metadata; expression/predicate/INCLUDE/opclass/collation recreation is unsupported by this basic draft form."));
                }
                item = item.child(
                    Button::new(("draft-drop-index", i))
                        .label("Draft DROP INDEX · destructive")
                        .small()
                        .disabled(self.operation.busy() || index.constraint_owned)
                        .on_click(cx.listener(move |s, _, _, cx| {
                            s.drop_index_draft(target.clone(), name.clone(), cx)
                        })),
                );
                body = body.child(item);
            }
            if table.foreign_keys.len() > 200 || table.indexes.len() > 200 {
                body=body.child(hint("Foreign key/index detail preview capped at 200 each; complete captured metadata remains in snapshot JSON."));
            }
            body = body.child(
                card("Create index draft · ordinary btree only")
                    .child(
                        Input::new(&self.index_name)
                            .aria_label("New index name")
                            .small()
                            .disabled(self.operation.busy()),
                    )
                    .child(
                        Input::new(&self.index_columns)
                            .aria_label("Exact captured column names for index, comma separated")
                            .small()
                            .disabled(self.operation.busy()),
                    )
                    .child(
                        Button::new("toggle-unique-index")
                            .label(if self.unique_index {
                                "Unique: yes"
                            } else {
                                "Unique: no"
                            })
                            .small()
                            .disabled(self.operation.busy())
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.unique_index = !s.unique_index;
                                cx.notify();
                            })),
                    )
                    .child(
                        self.button("draft-create-index", "Generate CREATE INDEX draft")
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.generate_draft(DraftAction::CreateIndex, cx)
                            })),
                    ),
            );
        }
        if let Some(Selection::View(target)) = &self.selected_object
            && let Ok(view) = drafts::view(snapshot, target)
        {
            body=body.child(card(format!("Captured {} {}",if view.materialized{"materialized view"}else{"view"},target.sql())).child(hint(model::short(&view.definition,12_000))).child(hint(if view.materialized{"Materialized view refresh/storage/drop/create is unsupported in this basic view form."}else{"Definition captured from PostgreSQL. View owner, ACL, reloptions/security settings and triggers are outside this basic form."})));
        }
        body=body.child(card("Ordinary view SQL drafts").child(hint("Schema and view name are quoted as identifiers. Definition must parse as one query; review functions and dependencies before running. Existing ordinary views require explicit destructive Replace."))
            .child(Input::new(&self.object_schema).aria_label("View schema in captured scope").small().disabled(self.operation.busy()))
            .child(Input::new(&self.object_name).aria_label("View name").small().disabled(self.operation.busy()))
            .child(Textarea::new(&self.view_query).aria_label("View definition, exactly one query").h(px(110.)).disabled(self.operation.busy()))
            .child(div().flex().flex_wrap().gap_2().child(self.button("draft-create-view","Draft CREATE VIEW").on_click(cx.listener(|s,_,_,cx|s.generate_draft(DraftAction::CreateView,cx)))).child(self.button("draft-replace-view","Draft REPLACE VIEW · destructive").on_click(cx.listener(|s,_,_,cx|s.generate_draft(DraftAction::ReplaceView,cx)))).child(self.button("draft-drop-view","Draft DROP VIEW · destructive").disabled(self.operation.busy()||!matches!(self.selected_object,Some(Selection::View(_)))).on_click(cx.listener(|s,_,_,cx|s.generate_draft(DraftAction::DropView,cx))))));
        if let Some(SqlDraft {
            sql,
            destructive,
            description,
        }) = &self.object_draft
        {
            body=body.child(card(if *destructive{"DESTRUCTIVE SQL draft · review required"}else{"SQL draft · review required"}).child(hint(description.clone())).child(div().font_family("Menlo").text_size(px(11.)).child(model::short(sql,64_000))).child(hint("This draft belongs to the captured target above. It is unchanged by later form edits; regenerate to reflect edits. No SQL has been executed."))
            .child(div().flex().flex_wrap().gap_2().child(self.button("copy-object-draft","Copy SQL draft").on_click(cx.listener(|s,_,_,cx|{if let Some(draft)=&s.object_draft{cx.write_to_clipboard(ClipboardItem::new_string(draft.sql.clone()));s.status="SQL draft copied · nothing executed".into();cx.notify();}}))).child(self.button("save-object-draft","Save SQL draft…").on_click(cx.listener(|s,_,w,cx|s.save(SaveKind::Draft,w,cx)))).child(self.button("open-object-draft","Open draft in new console").on_click(cx.listener(|s,_,_,cx|s.open_draft_console(cx))))));
        }
        body
    }
}
