//! Native, bounded, read-only database tools. Parent owns GPUI/component initialization.
mod drafts;
mod i18n;
mod model;
mod objects;
mod render;
mod worker;

use dbide_data::{DataDiff, TabularData};
use dbide_i18n::{Locale, Message};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine};
use dbide_schema::{DatabaseIdentity, MigrationPlan, SchemaSnapshot, explain::ExplainDocument};
use gpui::{prelude::*, *};
use gpui_component::input::{InputState, TextareaState};
use model::{OperationState, Tab};
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Mount with `cx.new(|cx| ToolsPanel::new(request, initial_sql, window, cx))`.
/// Request is a fixed credential/profile snapshot; no parent selection is consulted later.
/// Parent opens a new dirty console against this exact captured profile; never auto-run.
#[derive(Clone, Debug)]
pub enum ToolsEvent {
    OpenSqlDraft {
        profile: ConnectionProfile,
        sql: String,
    },
}
impl EventEmitter<ToolsEvent> for ToolsPanel {}

pub struct ToolsPanel {
    locale: Locale,
    request: Option<ConnectionRequest>,
    label: String,
    tab: Tab,
    scope: Entity<InputState>,
    sql: Entity<TextareaState>,
    keys: Entity<InputState>,
    object_schema: Entity<InputState>,
    object_name: Entity<InputState>,
    index_name: Entity<InputState>,
    index_columns: Entity<InputState>,
    view_query: Entity<TextareaState>,
    selected_object: Option<drafts::Selection>,
    unique_index: bool,
    object_draft: Option<drafts::SqlDraft>,
    operation: OperationState,
    cancel: Option<Arc<AtomicBool>>,
    error: Option<i18n::Diagnostic>,
    status: String,
    actual: Option<Arc<SchemaSnapshot>>,
    desired: Option<Arc<SchemaSnapshot>>,
    er: Option<Result<(dbide_schema::er::ErGraph, String), String>>,
    migration: Option<Arc<MigrationPlan>>,
    explain: Option<ExplainDocument>,
    explained_sql: String,
    collapsed: BTreeSet<String>,
    before: Option<Arc<TabularData>>,
    after: Option<Arc<TabularData>>,
    before_label: String,
    after_label: String,
    data_diff: Option<DataDiff>,
}

enum Completed {
    Capture(
        SchemaSnapshot,
        Result<(dbide_schema::er::ErGraph, String), String>,
    ),
    Desired(SchemaSnapshot),
    Compare(MigrationPlan),
    Explain(ExplainDocument),
    Data(bool, TabularData, String),
    DataDiff(DataDiff),
    Draft(drafts::SqlDraft),
    Saved,
    Cancelled,
    ExternalError(String),
}
impl ToolsPanel {
    pub fn new(
        request: Option<ConnectionRequest>,
        initial_sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_locale(request, initial_sql, Locale::default(), window, cx)
    }
    /// Explicit per-panel locale. Does not alter GPUI globals, profiles or the SQL source.
    pub fn new_with_locale(
        request: Option<ConnectionRequest>,
        initial_sql: String,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let label = request
            .as_ref()
            .map(|r| model::target_label_with_locale(&r.profile, locale))
            .unwrap_or_else(|| {
                i18n::text(locale, "No profile captured · offline tools available").into()
            });
        let field = |value: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = InputState::new(window, cx);
                input.set_value(value, window, cx);
                input
            })
        };
        Self {
            locale,
            request,
            label,
            tab: Tab::Schema,
            scope: field("public".into(), window, cx),
            sql: cx.new(|cx| {
                let mut input = TextareaState::new(window, cx);
                input.set_value(initial_sql, window, cx);
                input
            }),
            keys: field(String::new(), window, cx),
            object_schema: field("public".into(), window, cx),
            object_name: field(String::new(), window, cx),
            index_name: field(String::new(), window, cx),
            index_columns: field(String::new(), window, cx),
            view_query: cx.new(|cx| {
                let mut input = TextareaState::new(window, cx);
                input.set_value("SELECT 1", window, cx);
                input
            }),
            selected_object: None,
            unique_index: false,
            object_draft: None,
            operation: OperationState::default(),
            cancel: None,
            error: None,
            status: "Ready · database workflows use dedicated read-only sessions".into(),
            actual: None,
            desired: None,
            er: None,
            migration: None,
            explain: None,
            explained_sql: String::new(),
            collapsed: BTreeSet::new(),
            before: None,
            after: None,
            before_label: "Not loaded".into(),
            after_label: "Not loaded".into(),
            data_diff: None,
        }
    }
    /// Repaint all app-owned messages, including stored status/errors, without retargeting work.
    pub fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        if self.locale == locale {
            return;
        }
        self.locale = locale;
        self.label = self
            .request
            .as_ref()
            .map(|r| model::target_label_with_locale(&r.profile, locale))
            .unwrap_or_else(|| {
                i18n::text(locale, "No profile captured · offline tools available").to_owned()
            });
        cx.notify();
    }
    pub fn locale(&self) -> Locale {
        self.locale
    }
    fn t<'a>(&self, source: &'a str) -> &'a str {
        i18n::text(self.locale, source)
    }
    fn f(&self, message: Message, args: &[(&str, String)]) -> String {
        i18n::format(self.locale, message, args)
    }
    fn flag(&self, value: bool) -> String {
        dbide_i18n::tr(
            self.locale,
            if value {
                Message::ToolsYes
            } else {
                Message::ToolsNo
            },
        )
        .to_owned()
    }
    fn localized_error(&self, error: &i18n::Diagnostic) -> String {
        self.f(
            Message::ToolsToolsOperationFailedError,
            &[("error", error.body(self.locale).to_owned())],
        )
    }
    fn live_enabled(&self) -> bool {
        self.request
            .as_ref()
            .is_some_and(|r| r.profile.engine == DatabaseEngine::PostgreSql)
    }
    fn fail(&mut self, error: impl Into<String>, cx: &mut Context<Self>) {
        self.error = Some(i18n::Diagnostic::Owned(error.into()));
        self.status = "Action needs attention".into();
        cx.notify();
    }
    fn begin(&mut self, label: &str, cx: &mut Context<Self>) -> Option<(u64, Arc<AtomicBool>)> {
        if self.operation.busy() {
            return None;
        }
        let id = self.operation.begin();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.error = None;
        self.status = label.into();
        cx.notify();
        Some((id, cancel))
    }
    fn finish(&mut self, id: u64, result: Result<Completed, String>, cx: &mut Context<Self>) {
        if !self.operation.finish(id) {
            return;
        }
        self.cancel = None;
        self.error = None;
        match result {
            Ok(Completed::Capture(value, er)) => {
                self.er = Some(er);
                self.actual = Some(Arc::new(value));
                self.selected_object = None;
                self.object_draft = None;
                self.migration = None;
                self.status = "Catalog captured · complete within explicit schema scope".into();
            }
            Ok(Completed::Desired(value)) => {
                self.desired = Some(Arc::new(value));
                self.migration = None;
                self.status =
                    "Desired schema loaded · compare direction: desired → captured actual".into();
            }
            Ok(Completed::Compare(value)) => {
                self.migration = Some(Arc::new(value));
                self.status = "Migration preview ready · no SQL executed".into();
            }
            Ok(Completed::Explain(value)) => {
                self.explain = Some(value);
                self.collapsed.clear();
                self.status =
                    "Plan only · ANALYZE never requested · no actual timing metrics".into();
            }
            Ok(Completed::Data(before, value, label)) => {
                if before {
                    self.before = Some(Arc::new(value));
                    self.before_label = label;
                } else {
                    self.after = Some(Arc::new(value));
                    self.after_label = label;
                }
                self.data_diff = None;
                self.status = "Tagged JSON loaded · select explicit unique non-NULL keys".into();
            }
            Ok(Completed::DataDiff(value)) => {
                self.data_diff = Some(value);
                self.status = "Exact typed data comparison ready · before → after".into();
            }
            Ok(Completed::Draft(value)) => {
                self.object_draft = Some(value);
                self.status="SQL draft ready · copy/save or open a new console for explicit review; nothing executed".into();
            }
            Ok(Completed::Saved) => {
                self.status =
                    "Export saved atomically · existing files are never overwritten".into()
            }
            Ok(Completed::Cancelled) => self.status = "File selection cancelled".into(),
            Ok(Completed::ExternalError(error)) => {
                self.error = Some(i18n::Diagnostic::Raw(error));
                self.status = "Operation failed · previous successful inputs retained".into();
            }
            Err(error) => {
                self.error = Some(i18n::Diagnostic::Owned(error));
                self.status = "Operation failed · previous successful inputs retained".into();
            }
        }
        cx.notify();
    }
    fn run<F>(&mut self, id: u64, cancel: Arc<AtomicBool>, future: F, cx: &mut Context<Self>)
    where
        F: std::future::Future<Output = Result<Completed, String>> + Send + 'static,
    {
        let receiver = worker::spawn(cancel, future);
        cx.spawn(async move |view, cx| {
            let result = receiver.await.unwrap_or_else(|_| {
                Err("Read-only worker stopped before returning a result".into())
            });
            let _ = view.update(cx, |s, cx| s.finish(id, result, cx));
        })
        .detach();
    }
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.operation.cancel();
        self.error = None;
        self.status =
            "Cancelled · pending read-only session retiring; stale result discarded".into();
        cx.notify();
    }
    fn capture(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.request.clone().filter(|_| self.live_enabled()) else {
            self.fail("Schema capture currently requires a PostgreSQL profile captured when this panel opens",cx);
            return;
        };
        let scope = match model::scope(&self.scope.read(cx).value()) {
            Ok(v) => v,
            Err(e) => {
                self.fail(e, cx);
                return;
            }
        };
        let Some((id, cancel)) = self.begin("Capturing scoped catalog…", cx) else {
            return;
        };
        self.run(
            id,
            cancel,
            async move {
                let value =
                    match dbide_workflows::capture_schema(&request, &scope, Default::default())
                        .await
                    {
                        Ok(value) => value,
                        Err(error) => return Ok(Completed::ExternalError(error.to_string())),
                    };
                let er = dbide_schema::er::graph(&value, model::er_limits())
                    .and_then(|graph| {
                        dbide_schema::er::export_svg(&value, model::er_limits())
                            .map(|svg| (graph, svg))
                    })
                    .map_err(|e| e.to_string());
                Ok(Completed::Capture(value, er))
            },
            cx,
        );
    }
    fn explain(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.request.clone().filter(|_| self.live_enabled()) else {
            self.fail(
                "EXPLAIN currently requires a PostgreSQL profile captured when this panel opens",
                cx,
            );
            return;
        };
        let sql = self.sql.read(cx).value().to_string();
        let Some((id, cancel)) = self.begin("Planning exactly one statement…", cx) else {
            return;
        };
        self.explain = None;
        self.explained_sql = sql.clone();
        self.run(
            id,
            cancel,
            async move {
                Ok(
                    match dbide_workflows::explain_plan(&request, &sql, false, Default::default())
                        .await
                    {
                        Ok(value) => Completed::Explain(value),
                        Err(error) => Completed::ExternalError(error.to_string()),
                    },
                )
            },
            cx,
        );
    }
    fn compare(&mut self, cx: &mut Context<Self>) {
        let (Some(desired), Some(actual)) = (self.desired.clone(), self.actual.clone()) else {
            self.fail("Load desired schema and capture actual catalog first", cx);
            return;
        };
        let Some((id, cancel)) = self.begin("Comparing desired → actual…", cx) else {
            return;
        };
        self.run(
            id,
            cancel,
            async move {
                dbide_workflows::compare_snapshots(&desired, &actual)
                    .map(Completed::Compare)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
    }
    fn data_compare(&mut self, cx: &mut Context<Self>) {
        let (Some(before), Some(after)) = (self.before.clone(), self.after.clone()) else {
            self.fail("Open both before and after tagged JSON files first", cx);
            return;
        };
        let keys = match model::key_columns(&self.keys.read(cx).value(), &before) {
            Ok(v) => v,
            Err(e) => {
                self.fail(e, cx);
                return;
            }
        };
        let Some((id, cancel)) = self.begin("Comparing typed rows by explicit identity…", cx)
        else {
            return;
        };
        self.data_diff = None;
        self.run(
            id,
            cancel,
            async move {
                dbide_data::diff_by_key(&before, &after, &keys, Default::default())
                    .map(Completed::DataDiff)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
    }
    fn open(&mut self, kind: OpenKind, window: &mut Window, cx: &mut Context<Self>) {
        let default_schema = if matches!(kind, OpenKind::Desired) {
            match model::scope(&self.scope.read(cx).value()) {
                Ok(scope) => scope[0].clone(),
                Err(e) => {
                    self.fail(e, cx);
                    return;
                }
            }
        } else {
            String::from("public")
        };
        let Some((id, cancel)) = self.begin("Choose local input…", cx) else {
            return;
        };
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.t("Open local schema or tagged JSON").into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            let choice = receiver.await;
            let result = match choice {
                Ok(Ok(Some(paths))) => paths
                    .into_iter()
                    .next()
                    .map(Ok)
                    .unwrap_or_else(|| Err("No input file selected".into())),
                Ok(Ok(None)) => {
                    let _ = view.update(cx, |s, cx| s.finish(id, Ok(Completed::Cancelled), cx));
                    return;
                }
                _ => Err("Native file dialog failed".into()),
            };
            let result = match result {
                Ok(path) => worker::spawn(cancel, async move {
                    let label = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    match kind {
                        OpenKind::Desired => {
                            let text = worker::read_text(&path)?;
                            let snapshot = if text.trim_start().starts_with('{') {
                                SchemaSnapshot::from_json(&text)
                            } else {
                                dbide_schema::ddl::import_postgres_ddl(
                                    &text,
                                    DatabaseIdentity {
                                        engine: "postgresql".into(),
                                        identity: format!("offline:{label}"),
                                        name: label.clone(),
                                        server_version: None,
                                    },
                                    &default_schema,
                                    &label,
                                )
                            }
                            .map_err(|e| e.to_string())?;
                            Ok(Completed::Desired(snapshot))
                        }
                        OpenKind::Before | OpenKind::After => {
                            let file = std::fs::File::open(path)
                                .map_err(|_| "Could not open selected file".to_string())?;
                            let data = dbide_data::read_json(file, Default::default())
                                .map_err(|e| e.to_string())?;
                            Ok(Completed::Data(
                                matches!(kind, OpenKind::Before),
                                data,
                                label,
                            ))
                        }
                    }
                })
                .await
                .unwrap_or_else(|_| Err("File worker stopped".into())),
                Err(e) => Err(e),
            };
            let _ = view.update(cx, |s, cx| s.finish(id, result, cx));
        })
        .detach();
    }
    fn save(&mut self, kind: SaveKind, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self.actual.clone();
        let migration = self.migration.clone();
        let draft = self.object_draft.clone();
        if (matches!(kind, SaveKind::Snapshot | SaveKind::Svg) && snapshot.is_none())
            || (matches!(kind, SaveKind::Sql) && migration.is_none())
        {
            self.fail("Create the requested output first", cx);
            return;
        }
        if matches!(kind, SaveKind::Draft) && draft.is_none() {
            self.fail("Generate an object SQL draft first", cx);
            return;
        }
        let Some((id, cancel)) = self.begin("Choose new export destination…", cx) else {
            return;
        };
        let filename = match kind {
            SaveKind::Snapshot => "schema-snapshot.json",
            SaveKind::Sql => "migration-preview.sql",
            SaveKind::Svg => "relationships.svg",
            SaveKind::Draft => "object-draft.sql",
        };
        let receiver = cx.prompt_for_new_path(Path::new("."), Some(filename));
        cx.spawn_in(window, async move |view, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(p))) => p,
                Ok(Ok(None)) => {
                    let _ = view.update(cx, |s, cx| s.finish(id, Ok(Completed::Cancelled), cx));
                    return;
                }
                _ => {
                    let _ = view.update(cx, |s, cx| {
                        s.finish(id, Err("Native save dialog failed".into()), cx)
                    });
                    return;
                }
            };
            let result = worker::spawn(cancel, async move {
                let contents = match kind {
                    SaveKind::Draft => draft.as_ref().expect("checked draft").sql.clone(),
                    SaveKind::Snapshot => snapshot
                        .as_ref()
                        .expect("checked snapshot")
                        .to_json()
                        .map_err(|e| e.to_string())?,
                    SaveKind::Sql => migration.as_ref().expect("checked plan").preview_sql(),
                    SaveKind::Svg => dbide_schema::er::export_svg(
                        snapshot.as_ref().expect("checked snapshot"),
                        model::er_limits(),
                    )
                    .map_err(|e| e.to_string())?,
                };
                worker::write_new_atomic(&path, contents.as_bytes()).map_err(|e| e.to_string())?;
                Ok(Completed::Saved)
            })
            .await
            .unwrap_or_else(|_| Err("Export worker stopped".into()));
            let _ = view.update(cx, |s, cx| s.finish(id, result, cx));
        })
        .detach();
    }
}
impl Drop for ToolsPanel {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Release);
        }
    }
}
#[derive(Clone, Copy)]
enum OpenKind {
    Desired,
    Before,
    After,
}
#[derive(Clone, Copy)]
enum SaveKind {
    Draft,
    Snapshot,
    Sql,
    Svg,
}
