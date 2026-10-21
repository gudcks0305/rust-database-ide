use super::*;

/// Read-only metadata captured for one object and one profile; selection cannot retarget it.
pub(super) struct ObjectInspector {
    pub object: dbide_bridge::SchemaObject,
    target: String,
    table: Option<dbide_schema::Table>,
    definition: Option<String>,
    status: String,
}
impl ObjectInspector {
    pub fn new(
        request: Option<ConnectionRequest>,
        object: dbide_bridge::SchemaObject,
        cx: &mut Context<Self>,
    ) -> Self {
        let target = request
            .as_ref()
            .map(|r| {
                format!(
                    "{} · {}",
                    r.profile.name,
                    dbide_ui_spike::workbench_model::profile_target(&r.profile)
                )
            })
            .unwrap_or_else(|| "Cached metadata".into());
        let pg = request
            .as_ref()
            .is_some_and(|r| r.profile.engine == DatabaseEngine::PostgreSql);
        if let Some(request) = request.filter(|_| pg) {
            let schema = object.schema.clone();
            let name = object.name.clone();
            cx.spawn(async move |view,cx| {
                let result=cx.background_executor().spawn(async move {
                    let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e|e.to_string())?;
                    runtime.block_on(dbide_workflows::capture_schema(&request,std::slice::from_ref(&schema),dbide_workflows::CaptureLimits::default())).map_err(|e|e.to_string()).map(|snapshot|(snapshot,schema,name))
                }).await;
                let _=view.update(cx,|s,cx|{
                    match result {
                        Ok((snapshot,schema,name))=>{
                            if let Some(schema)=snapshot.schemas.into_iter().find(|v|v.name==schema) {
                                s.table=schema.tables.into_iter().find(|v|v.name==name);
                                s.definition=schema.views.into_iter().find(|v|v.name==name).map(|v|v.definition);
                            }
                            s.status=if s.table.is_some()||s.definition.is_some(){"Live PostgreSQL structure · separate read-only metadata session".into()}else{"Object no longer present in the captured catalog. Refresh the explorer.".into()};
                        },
                        Err(error)=>s.status=format!("Detailed metadata unavailable: {error}. Basic cached columns remain visible."),
                    }
                    cx.notify();
                });
            }).detach();
        }
        Self {
            object,
            target,
            table: None,
            definition: None,
            status: if pg {
                "Loading defaults, keys and indexes…".into()
            } else {
                "Live columns, nullability, primary/foreign key markers and defaults. Index and FK target details were not loaded by this adapter."
                    .into()
            },
        }
    }
}
impl Render for ObjectInspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .id("object-structure-scroll")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .text_color(rgb(TEXT))
            .text_size(px(11.))
            .child(
                div()
                    .id("structure-target")
                    .role(Role::Label)
                    .aria_label(self.target.clone())
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child(self.target.clone()),
            )
            .child(
                div()
                    .id("structure-name")
                    .role(Role::Label)
                    .aria_label(format!("{}.{}", self.object.schema, self.object.name))
                    .mt_2()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{}.{}", self.object.schema, self.object.name)),
            )
            .child(
                div()
                    .id("structure-capture-status")
                    .role(Role::Label)
                    .aria_label(self.status.clone())
                    .mt_2()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child(self.status.clone()),
            )
            .child(
                Button::new("copy-structure-fields")
                    .label("Copy fields")
                    .small()
                    .ghost()
                    .on_click(cx.listener(|s, _, _, cx| {
                        let mut text = "Name\tType\tNULL\tKey\tDefault\tExtra\n".to_string();
                        for c in &s.object.columns {
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
        let cell =
            |value: String, width: f32| div().w(px(width)).flex_shrink_0().px_2().child(value);
        let mut columns = div().min_w(px(650.)).child(
            div()
                .h(px(26.))
                .flex()
                .items_center()
                .bg(rgb(BG))
                .text_color(rgb(MUTED))
                .child(cell("Name".into(), 130.))
                .child(cell("Type".into(), 150.))
                .child(cell("NULL".into(), 55.))
                .child(cell("Key".into(), 55.))
                .child(cell("Default / extra".into(), 260.)),
        );
        for c in &self.object.columns {
            let detail = self
                .table
                .as_ref()
                .and_then(|t| t.columns.iter().find(|v| v.name == c.name));
            let pk = c.primary_key_ordinal.is_some()
                || self
                    .table
                    .as_ref()
                    .and_then(|t| t.primary_key.as_ref())
                    .is_some_and(|k| k.columns.contains(&c.name));
            let dtype = detail
                .map(|c| c.data_type.clone())
                .unwrap_or_else(|| c.data_type.clone());
            let default = detail
                .and_then(|d| d.default.as_deref())
                .or(c.default.as_deref())
                .unwrap_or("—");
            let extra = c.extra.as_deref().unwrap_or("");
            let key = format!(
                "{}{}",
                if pk { "PK " } else { "" },
                if c.foreign_key { "FK" } else { "" }
            );
            let label = format!(
                "Column {} · {} · {} · {} · default {} · {}",
                c.name,
                dtype,
                if c.nullable {
                    "NULL allowed"
                } else {
                    "NOT NULL"
                },
                key,
                default,
                extra
            );
            columns = columns.child(
                div()
                    .id(SharedString::from(format!("structure-column-{}", c.name)))
                    .role(Role::Label)
                    .aria_label(label)
                    .min_h(px(30.))
                    .py_1()
                    .flex()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(cell(c.name.clone(), 130.))
                    .child(cell(dtype, 150.))
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
        if let Some(table) = &self.table {
            root = root.child(
                div()
                    .id("structure-indexes-heading")
                    .role(Role::Label)
                    .aria_label("Indexes")
                    .mt_4()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Indexes"),
            );
            if table.indexes.is_empty() {
                root = root.child(
                    div()
                        .id("structure-no-indexes")
                        .role(Role::Label)
                        .aria_label("No indexes in this catalog capture")
                        .mt_2()
                        .text_color(rgb(MUTED))
                        .child("No indexes in this catalog capture."),
                );
            }
            root = root.children(table.indexes.iter().map(|index| {
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
                div()
                    .id(SharedString::from(format!(
                        "structure-index-{}",
                        index.name
                    )))
                    .role(Role::Label)
                    .aria_label(text.clone())
                    .mt_2()
                    .child(text)
            }));
            root = root.children(table.foreign_keys.iter().enumerate().map(|(ix, fk)| {
                let text = format!(
                    "FK {} ({}) → {} ({}) · update {} · delete {}",
                    fk.name.as_deref().unwrap_or(""),
                    fk.columns.join(", "),
                    fk.references.sql(),
                    fk.referenced_columns.join(", "),
                    fk.on_update.sql(),
                    fk.on_delete.sql()
                );
                div()
                    .id(("structure-fk", ix))
                    .role(Role::Label)
                    .aria_label(text.clone())
                    .mt_2()
                    .child(text)
            }));
        }
        if let Some(sql) = &self.definition {
            root = root.child(
                div()
                    .id("structure-view-definition")
                    .role(Role::Label)
                    .aria_label(format!("View definition: {sql}"))
                    .mt_4()
                    .font_family("Menlo")
                    .child(sql.clone()),
            );
        }
        root
    }
}
