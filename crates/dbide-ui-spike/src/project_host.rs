use super::*;
use dbide_project::{Manifest, Project, SqlFile};
use gpui_component::{Selectable, WindowExt, notification::Notification};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Locations {
    schema_version: u32,
    current: Option<PathBuf>,
    recent: Vec<PathBuf>,
}
impl Locations {
    fn empty() -> Self {
        Self {
            schema_version: 1,
            ..Default::default()
        }
    }
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1
            || self.recent.len() > 10
            || self.current.iter().chain(&self.recent).any(|p| {
                !p.is_absolute()
                    || p.to_str()
                        .is_none_or(|s| s.len() > 4096 || s.chars().any(char::is_control))
            })
        {
            return Err("Workspace locations are invalid or from a newer version; preserved without overwrite.".into());
        }
        Ok(())
    }
    fn select(&mut self, root: PathBuf) {
        self.current = Some(root.clone());
        self.recent.retain(|p| p != &root);
        self.recent.insert(0, root);
        self.recent.truncate(10);
    }
}
fn read_locations(path: &Path) -> Result<Locations, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = match options.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Locations::empty()),
        Err(e) => return Err(e.to_string()),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Workspace locations must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("Workspace locations exceed64KiB; original preserved".into());
    }
    let locations: Locations = serde_json::from_slice(&bytes)
        .map_err(|_| "Workspace locations malformed; original preserved".to_owned())?;
    locations.validate()?;
    Ok(locations)
}
fn write_locations(path: Option<&Path>, value: &Locations) -> Result<(), String> {
    value.validate()?;
    let Some(path) = path else { return Ok(()) };
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    dbide_ui_spike::write_atomic(path, |out| std::io::Write::write_all(out, &bytes))
        .map_err(|e| e.to_string())
}
#[derive(Default)]
pub(super) struct ProjectHost {
    pub panel: Option<Entity<project_panel::ProjectPanel>>,
    pub root: Option<Project>,
    pub show_files: bool,
    pub loading: bool,
    pub error: Option<String>,
    registry: Option<PathBuf>,
    locations: Locations,
    registry_blocked: bool,
    subscription: Option<Subscription>,
}
impl Workbench {
    pub(super) fn init_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project.registry = self
            .store
            .as_ref()
            .and_then(|s| s.path().parent())
            .map(|p| p.join("project-locations.json"));
        self.project.loading = true;
        let path = self.project.registry.clone();
        let explicit = std::env::var_os("DBIDE_WORKSPACE").map(PathBuf::from);
        cx.spawn_in(window, async move |view, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    path.as_deref()
                        .map(read_locations)
                        .unwrap_or_else(|| Ok(Locations::empty()))
                })
                .await;
            let _ = view.update_in(cx, |s, w, cx| {
                s.project.loading = false;
                match loaded {
                    Ok(locations) => {
                        let current = explicit.or_else(|| locations.current.clone());
                        s.project.locations = locations;
                        if let Some(root) = current {
                            s.activate_project(root, false, w, cx);
                        }
                    }
                    Err(error) => {
                        s.project.registry_blocked = true;
                        s.project.error = Some(error);
                        s.project.locations = Locations::empty();
                        if let Some(root) = explicit {
                            s.activate_project(root, false, w, cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn recent_projects(&self) -> Vec<PathBuf> {
        self.project.locations.recent.clone()
    }
    pub(super) fn open_project_action(
        &mut self,
        _: &OpenProject,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.choose_project(false, w, cx);
    }
    pub(super) fn new_project_action(
        &mut self,
        _: &NewProject,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.choose_project(true, w, cx);
    }
    fn choose_project(&mut self, create: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.loading {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                if create {
                    "Initialize workspace in this folder"
                } else {
                    "Open workspace folder"
                }
                .into(),
            ),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(mut paths))) = receiver.await
                && let Some(root) = paths.pop()
            {
                let _ = view.update_in(cx, |s, w, cx| s.activate_project(root, create, w, cx));
            }
        })
        .detach();
    }
    pub(super) fn activate_project(
        &mut self,
        root: PathBuf,
        create: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.loading {
            return;
        }
        self.project.loading = true;
        self.project.error = None;
        let registry = if self.project.registry_blocked {
            None
        } else {
            self.project.registry.clone()
        };
        let mut locations = self.project.locations.clone();
        cx.spawn_in(window, async move |view, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    let name = root
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Workspace");
                    let project = if create {
                        Project::init(&root, name)
                    } else {
                        Project::open(&root)
                    }
                    .map_err(|e| e.to_string())?;
                    let manifest = project.manifest().map_err(|e| e.to_string())?;
                    locations.select(project.root().to_path_buf());
                    let persistence_error = write_locations(registry.as_deref(), &locations).err();
                    Ok::<_, String>((project, manifest, locations, persistence_error))
                })
                .await;
            let _ = view.update_in(cx, |s, w, cx| {
                s.project.loading = false;
                match loaded {
                    Ok((project, manifest, locations, error)) => {
                        s.project.locations = locations;
                        s.install_project(project, manifest, w, cx);
                        s.project.error = error;
                    }
                    Err(error) => {
                        s.project.error = Some(error.clone());
                        s.project.show_files = true;
                        w.push_notification(Notification::error(error), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn install_project(
        &mut self,
        project: Project,
        manifest: Manifest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.new(|cx| {
            project_panel::ProjectPanel::new(project.clone(), manifest, self.locale, window, cx)
        });
        self.project.subscription =
            Some(cx.subscribe_in(&panel, window, |s, source, event, w, cx| {
                if s.project
                    .panel
                    .as_ref()
                    .is_none_or(|p| p.entity_id() != source.entity_id())
                {
                    return;
                }
                match event {
                    project_panel::ProjectPanelEvent::Open { file, manifest } => {
                        let root = source.read(cx).project().root().to_path_buf();
                        s.open_project_file(root, file.clone(), manifest, w, cx);
                    }
                    project_panel::ProjectPanelEvent::BindCurrent { expected_revision } => {
                        s.bind_project_target(*expected_revision, w, cx)
                    }
                    project_panel::ProjectPanelEvent::ChooseProject => {
                        s.choose_project(false, w, cx)
                    }
                    project_panel::ProjectPanelEvent::CloseProject => s.close_project(w, cx),
                }
            }));
        self.project.root = Some(project);
        self.project.panel = Some(panel);
        self.project.show_files = true;
        self.catalog_collapsed = false;
    }
    fn close_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.loading {
            return;
        }
        self.project.root = None;
        self.project.panel = None;
        self.project.subscription = None;
        self.project.locations.current = None;
        if !self.project.registry_blocked {
            let registry = self.project.registry.clone();
            let locations = self.project.locations.clone();
            self.project.loading = true;
            cx.spawn_in(window, async move |view, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { write_locations(registry.as_deref(), &locations) })
                    .await;
                let _ = view.update(cx, |s, cx| {
                    s.project.loading = false;
                    s.project.error = result.err();
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }
    fn bind_project_target(&mut self, revision: u64, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sql_view_active() {
            window.push_notification(
                Notification::info("Select a SQL console before binding its target."),
                cx,
            );
            return;
        }
        let Some(project) = self.project.root.clone() else {
            return;
        };
        if self
            .console()
            .target()
            .is_some_and(|p| dbide_project::validate_target_profile(p).is_err())
        {
            window.push_notification(Notification::warning(if self.locale==Locale::Ko {"워크스페이스 SQLite 연결은 절대 파일 경로가 필요합니다. 연결 설정을 수정하세요."}else{"Workspace SQLite targets require an absolute file path. Edit the connection profile."}),cx);
            return;
        }
        let Some(target) = self.console().stored_target() else {
            window.push_notification(
                Notification::info(if self.locale == Locale::Ko {
                    "SQL 콘솔에서 연결과 DB를 먼저 선택하세요."
                } else {
                    "Select a connection and database in a SQL console first."
                }),
                cx,
            );
            return;
        };
        let panel = self.project.panel.clone();
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    project
                        .set_default_target(Some(target), revision)
                        .map_err(|e| e.to_string())
                })
                .await;
            let _ = view.update_in(cx, |_, w, cx| match result {
                Ok(_) => {
                    if let Some(panel) = panel {
                        panel.update(cx, |p, cx| p.refresh(cx));
                    }
                }
                Err(error) => w.push_notification(Notification::error(error), cx),
            });
        })
        .detach();
    }
    fn open_project_file(
        &mut self,
        root: PathBuf,
        file: SqlFile,
        manifest: &Manifest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(doc) = self
            .documents
            .iter_mut()
            .find(|d| d.model.path.as_ref() == Some(&file.absolute_path))
        {
            doc.project_root = Some(root);
            let id = doc.model.id;
            self.switch_document(id, cx);
            return;
        }
        if self.documents.len() >= 32 {
            self.show_sql_tab_limit(cx);
            return;
        }
        let title = file
            .absolute_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        self.new_document(file.sql, title, true, window, cx);
        let request = manifest
            .default_target
            .as_ref()
            .and_then(|target| {
                self.workspace
                    .profiles
                    .iter()
                    .find(|p| p.id == target.profile_id)
                    .and_then(|p| target.resolve(p).ok())
            })
            .filter(|profile| dbide_project::validate_target_profile(profile).is_ok())
            .map(|profile| ConnectionRequest {
                profile,
                password: String::new(),
            });
        if manifest.default_target.is_some() && request.is_none() {
            window.push_notification(Notification::warning(if self.locale==Locale::Ko {"워크스페이스의 연결을 확인할 수 없습니다. 이 SQL 탭은 연결되지 않은 상태로 열었습니다."}else{"Workspace target unavailable or changed. Opened an unbound SQL tab."}),cx);
        }
        if let Some(doc) = self
            .documents
            .iter_mut()
            .find(|d| d.model.id == self.active_document)
        {
            doc.model.path = Some(file.absolute_path);
            doc.project_root = Some(root);
            doc.required_profile_id = manifest
                .default_target
                .as_ref()
                .map(|t| t.profile_id.clone());
            doc.session = ConsoleSession::new(doc.model.id);
            if let Some(request) = request {
                let _ = doc.session.bind(doc.session.route(), request);
            }
            let database = doc
                .session
                .target()
                .map(|p| p.database.clone())
                .unwrap_or_default();
            doc.database_draft
                .update(cx, |i, cx| i.set_value(database, window, cx));
            doc.password_draft
                .update(cx, |i, cx| i.set_value(String::new(), window, cx));
        }
        self.status = "Workspace SQL opened; no connection or query started".into();
        cx.notify();
    }
    pub(super) fn project_default_save_directory(&self, cx: &App) -> Option<PathBuf> {
        self.project.panel.as_ref().map(|panel| {
            let p = panel.read(cx);
            let dir = p.current_directory();
            if dir.as_os_str().is_empty() || dir == Path::new(".") {
                p.project().root().join("queries")
            } else {
                p.project().root().join(dir)
            }
        })
    }
    pub(super) fn sidebar_mode_bar(&self, cx: &mut Context<Self>) -> Div {
        div()
            .h(px(35.))
            .flex()
            .gap_1()
            .px_2()
            .items_center()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                Button::new("sidebar-connections")
                    .small()
                    .ghost()
                    .label(if self.locale == Locale::Ko {
                        "연결"
                    } else {
                        "Connections"
                    })
                    .selected(!self.project.show_files)
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.project.show_files = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("sidebar-files")
                    .small()
                    .ghost()
                    .label(if self.locale == Locale::Ko {
                        "파일"
                    } else {
                        "Files"
                    })
                    .selected(self.project.show_files)
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.project.show_files = true;
                        cx.notify();
                    })),
            )
    }
    pub(super) fn project_sidebar(&self, cx: &mut Context<Self>) -> Div {
        let mut pane = div()
            .w(px(self.app_settings.sidebar_width as f32))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(self.sidebar_mode_bar(cx));
        if self.project.loading {
            pane = pane.child(div().p_3().child(if self.locale == Locale::Ko {
                "워크스페이스 불러오는 중…"
            } else {
                "Loading workspace…"
            }));
        }
        if let Some(error) = &self.project.error {
            pane = pane.child(
                div()
                    .p_3()
                    .text_color(rgb(0xe8ad93))
                    .text_size(px(12.))
                    .child(error.clone()),
            );
        }
        if let Some(panel) = &self.project.panel {
            pane = pane.child(div().flex_1().min_h_0().child(panel.clone()));
        } else {
            pane = pane.child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(if self.locale == Locale::Ko {
                        "폴더에 SQL 파일과 기본 연결을 함께 관리합니다."
                    } else {
                        "Keep SQL files and a default connection in a folder."
                    })
                    .child(
                        Button::new("workspace-create")
                            .small()
                            .label(if self.locale == Locale::Ko {
                                "새 워크스페이스…"
                            } else {
                                "New workspace…"
                            })
                            .disabled(self.project.loading)
                            .on_click(cx.listener(|s, _, w, cx| s.choose_project(true, w, cx))),
                    )
                    .child(
                        Button::new("workspace-open")
                            .small()
                            .label(if self.locale == Locale::Ko {
                                "워크스페이스 열기…"
                            } else {
                                "Open workspace…"
                            })
                            .disabled(self.project.loading)
                            .on_click(cx.listener(|s, _, w, cx| s.choose_project(false, w, cx))),
                    ),
            );
            for root in self.project.locations.recent.clone() {
                let label = root
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let id = root.to_string_lossy().into_owned();
                pane = pane.child(
                    Button::new(SharedString::from(format!("recent-{id}")))
                        .small()
                        .ghost()
                        .label(label)
                        .tooltip(id)
                        .disabled(self.project.loading)
                        .on_click(cx.listener(move |s, _, w, cx| {
                            s.activate_project(root.clone(), false, w, cx)
                        })),
                );
            }
        }
        pane
    }
}
