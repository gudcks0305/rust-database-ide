use super::*;
impl Workbench {
    pub(super) fn database_explorer(&self, cx: &mut Context<Self>) -> Div {
        let query = self.object_search.read(cx).value().trim().to_lowercase();
        let mut groups =
            std::collections::BTreeMap::<&str, Vec<&dbide_bridge::SchemaObject>>::new();
        for object in &self.schema {
            if dbide_ui_spike::explorer_model::matches(object, &query) {
                groups.entry(&object.schema).or_default().push(object);
            }
        }
        let mut list = div()
            .id("schema-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2();
        if self.schema.is_empty() {
            list = list.child(
                div()
                    .p_2()
                    .text_color(rgb(MUTED))
                    .child(if self.schema_loading {
                        "Loading tables and views…"
                    } else if self.connection == ConnectionState::Connected {
                        "Refresh to load this database."
                    } else {
                        "Connect, then Refresh to browse this database."
                    }),
            );
        } else if groups.is_empty() {
            list = list.child(
                div()
                    .p_2()
                    .text_color(rgb(MUTED))
                    .child("No matching schema, table or column."),
            );
        }
        for (schema, objects) in groups {
            let opened = !query.is_empty() || self.expanded_schemas.contains(schema);
            let key = schema.to_string();
            let mut group = div()
                .id(SharedString::from(format!("schema-group-{schema}")))
                .role(Role::Group)
                .aria_label(format!("Schema {schema}, {} objects", objects.len()))
                .aria_expanded(opened)
                .child(
                    Button::new(SharedString::from(format!("schema-{schema}")))
                        .ghost()
                        .w_full()
                        .justify_start()
                        .label(format!(
                            "{}  {}  ({})",
                            if opened { "▾" } else { "▸" },
                            schema,
                            objects.len()
                        ))
                        .small()
                        .on_click(cx.listener(move |s, _, _, cx| {
                            if !s.expanded_schemas.remove(&key) {
                                s.expanded_schemas.insert(key.clone());
                            }
                            cx.notify();
                        })),
                );
            if opened {
                for object in objects {
                    let key = dbide_schema::QualifiedName::new(&object.schema, &object.name).id();
                    let expanded = self.expanded_objects.contains(&key);
                    let toggle = key.clone();
                    let inspect = object.clone();
                    let data = object.clone();
                    let draft = object.clone();

                    let mut row = div()
                        .id(SharedString::from(format!("object-group-{key}")))
                        .role(Role::Group)
                        .aria_label(format!(
                            "{} {}.{} · {} columns",
                            object.kind,
                            object.schema,
                            object.name,
                            object.columns.len()
                        ))
                        .aria_expanded(expanded)
                        .ml_2()
                        .mt_1()
                        .border_l_1()
                        .border_color(rgb(BORDER))
                        .pl_1()
                        .child(
                            Button::new(SharedString::from(format!("object-{key}")))
                                .ghost()
                                .w_full()
                                .justify_start()
                                .label(format!(
                                    "{} {}  {}",
                                    if expanded { "▾" } else { "▸" },
                                    if dbide_ui_spike::explorer_model::is_table(&object.kind) {
                                        "▦"
                                    } else {
                                        "◫"
                                    },
                                    object.name
                                ))
                                .small()
                                .on_click(cx.listener(move |s, _, _, cx| {
                                    if !s.expanded_objects.remove(&toggle) {
                                        s.expanded_objects.insert(toggle.clone());
                                    }
                                    cx.notify();
                                })),
                        );
                    if expanded {
                        row = row
                            .child(
                                div()
                                    .ml_2()
                                    .flex()
                                    .gap_1()
                                    .child(
                                        Button::new(SharedString::from(format!("structure-{key}")))
                                            .label("Structure")
                                            .small()
                                            .on_click(cx.listener(move |s, _, _, cx| {
                                                let request = s.configured.clone();
                                                s.structure = Some(cx.new(|cx| {
                                                    object_inspector::ObjectInspector::new(
                                                        request,
                                                        inspect.clone(),
                                                        cx,
                                                    )
                                                }));
                                                s.form = None;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!("data-{key}")))
                                            .label("Data")
                                            .small()
                                            .disabled(
                                                self.busy()
                                                    || self.connection
                                                        != ConnectionState::Connected,
                                            )
                                            .on_click(cx.listener(move |s, _, w, cx| {
                                                s.open_object_data(data.clone(), w, cx)
                                            })),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!("sql-{key}")))
                                            .label("SQL")
                                            .small()
                                            .on_click(cx.listener(move |s, _, w, cx| {
                                                let sql =
                                                    dbide_ui_spike::workbench_model::table_query(
                                                        s.engine(),
                                                        &draft.schema,
                                                        &draft.name,
                                                    );
                                                s.new_document(
                                                    sql,
                                                    format!("{}.sql", draft.name),
                                                    false,
                                                    w,
                                                    cx,
                                                );
                                            })),
                                    ),
                            )
                            .children(object.columns.iter().map(|column| {
                                div()
                                    .id(SharedString::from(format!("column-{key}-{}", column.name)))
                                    .role(Role::Label)
                                    .aria_label(format!(
                                        "Column {} · type {} · {}{}{} · default {}",
                                        column.name,
                                        column.data_type,
                                        if column.nullable {
                                            "NULL allowed"
                                        } else {
                                            "NOT NULL"
                                        },
                                        if column.primary_key_ordinal.is_some() {
                                            " · primary key"
                                        } else {
                                            ""
                                        },
                                        if column.foreign_key {
                                            " · foreign key"
                                        } else {
                                            ""
                                        },
                                        column.default.as_deref().unwrap_or("none")
                                    ))
                                    .ml_3()
                                    .py_1()
                                    .text_size(px(10.))
                                    .child(div().text_color(rgb(TEXT)).child(format!(
                                        "{}{}{}",
                                        column.name,
                                        if column.primary_key_ordinal.is_some() {
                                            "  PK"
                                        } else {
                                            ""
                                        },
                                        if column.foreign_key { "  FK" } else { "" }
                                    )))
                                    .child(div().text_color(rgb(MUTED)).child(format!(
                                        "{} · {}",
                                        column.data_type,
                                        if column.nullable { "NULL" } else { "NOT NULL" }
                                    )))
                            }));
                        if object.columns.is_empty() {
                            row = row.child(
                                div()
                                    .id(SharedString::from(format!("no-columns-{key}")))
                                    .role(Role::Label)
                                    .aria_label("No columns returned")
                                    .ml_3()
                                    .text_color(rgb(MUTED))
                                    .child("No columns returned."),
                            );
                        }
                    }
                    group = group.child(row);
                }
            }
            list = list.child(group);
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_color(rgb(MUTED))
                            .text_size(px(10.))
                            .child("DATABASE EXPLORER"),
                    )
                    .child(
                        self.button("refresh-schema", "Refresh")
                            .disabled(
                                self.busy()
                                    || self.schema_loading
                                    || self.connection != ConnectionState::Connected,
                            )
                            .on_click(cx.listener(|s, _, _, cx| s.refresh_schema(cx))),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .pb_2()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child(
                        self.configured
                            .as_ref()
                            .map(|r| dbide_ui_spike::workbench_model::profile_target(&r.profile))
                            .unwrap_or_else(|| "No connection selected".into()),
                    ),
            )
            .child(
                div().px_2().pb_2().child(
                    Input::new(&self.object_search)
                        .small()
                        .aria_label("Search schemas tables and columns"),
                ),
            )
            .child(
                div()
                    .px_3()
                    .pb_2()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "{} loaded objects · {} loaded columns{}",
                        self.schema.len(),
                        self.schema.iter().map(|o| o.columns.len()).sum::<usize>(),
                        if self.schema_truncated {
                            " · metadata limit reached; subset only"
                        } else {
                            ""
                        }
                    )),
            )
            .child(list)
    }
    fn open_object_data(
        &mut self,
        object: dbide_bridge::SchemaObject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() || self.connection != ConnectionState::Connected {
            return;
        }
        if self.engine() == DatabaseEngine::PostgreSql
            && dbide_ui_spike::explorer_model::is_table(&object.kind)
        {
            self.open_table_editor(object.schema, object.name, window, cx);
            return;
        }
        let sql = dbide_ui_spike::workbench_model::table_query(
            self.engine(),
            &object.schema,
            &object.name,
        );
        self.open_data_query(sql, format!("{}-data.sql", object.name), window, cx);
    }
    pub(super) fn open_data_query(
        &mut self,
        sql: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.documents.len() >= 32 {
            self.status =
                "Close a SQL tab before opening table data (maximum 32). No SQL executed.".into();
            cx.notify();
            return;
        }
        self.new_document(sql.clone(), title, false, window, cx);
        self.submit_sql(sql, cx);
    }
}
