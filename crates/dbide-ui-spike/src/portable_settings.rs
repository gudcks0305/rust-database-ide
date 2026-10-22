//! Settings transfer uses captured metadata only; every filesystem operation is
//! isolated from GPUI and every opened window receives an explicit profile path.
use super::*;
use dbide_portable::{Bundle, Inspection, RestoreReport};
use dbide_settings::AppSettings;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) enum SettingsTransferEvent {
    Close,
    OpenConfiguration(PathBuf),
}

#[derive(Clone, Copy)]
enum FolderChoice {
    ExportBase,
    ImportRoot,
    OpenConfiguration,
}

pub(super) struct SettingsTransferPanel {
    profile_path: PathBuf,
    locale: Locale,
    export_base: PathBuf,
    bundle_path: Option<PathBuf>,
    bundle: Option<Arc<Bundle>>,
    inspection: Option<Inspection>,
    remap_root: Option<PathBuf>,
    destination: Option<PathBuf>,
    restored: Option<RestoreReport>,
    picker_busy: bool,
    working: bool,
    closing: bool,
    feedback: Option<String>,
    failed: bool,
}

impl EventEmitter<SettingsTransferEvent> for SettingsTransferPanel {}

fn bounded_feedback(message: impl AsRef<str>) -> String {
    let message = message.as_ref();
    if message.len() <= 1024 {
        return message.to_owned();
    }
    let mut end = 1021;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &message[..end])
}

fn saved_configuration(path: &Path) -> Result<(WorkspaceState, AppSettings, bool), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| "Configuration profiles file is unavailable".to_owned())?;
    if !metadata.is_file() {
        return Err("Configuration profiles must be a regular file".into());
    }
    let mut workspace = ProfileStore::open(path)
        .and_then(|store| store.load())
        .map_err(|error| bounded_feedback(error.to_string()))?;
    // History is excluded and never retained by the panel or passed to capture.
    workspace.history.clear();
    workspace.persist_history = false;
    let settings = dbide_settings::SettingsStore::for_profile_path(path)
        .load()
        .map_err(|error| bounded_feedback(error.to_string()))?;
    let recovery = dbide_portable::read_recovery_enabled(path)
        .map_err(|error| bounded_feedback(error.to_string()))?;
    Ok((workspace, settings, recovery))
}

fn existing_configuration(folder: &Path) -> Result<PathBuf, String> {
    let path = folder.join("profiles.json");
    saved_configuration(&path)?;
    Ok(path)
}

impl SettingsTransferPanel {
    pub(super) fn new(
        _workspace: WorkspaceState,
        _settings: AppSettings,
        _recovery_enabled: bool,
        profile_path: PathBuf,
        locale: Locale,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Self {
        Self {
            locale,
            export_base: profile_path
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf(),
            profile_path,
            bundle_path: None,
            bundle: None,
            inspection: None,
            remap_root: None,
            destination: None,
            restored: None,
            picker_busy: false,
            working: false,
            closing: false,
            feedback: None,
            failed: false,
        }
    }

    pub(super) fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.locale = locale;
        cx.notify();
    }

    pub(super) fn busy(&self) -> bool {
        self.picker_busy || self.working
    }

    pub(super) fn request_close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        if !self.busy() {
            cx.emit(SettingsTransferEvent::Close);
        }
        cx.notify();
    }

    fn tr(&self, ko: &'static str, en: &'static str) -> &'static str {
        if self.locale == Locale::Ko { ko } else { en }
    }

    fn feedback(&mut self, message: impl AsRef<str>, failed: bool) {
        self.feedback = Some(bounded_feedback(message));
        self.failed = failed;
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        self.working = false;
        if self.closing {
            cx.emit(SettingsTransferEvent::Close);
        }
        cx.notify();
    }

    fn choose_folder(&mut self, choice: FolderChoice, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() || self.closing {
            return;
        }
        self.picker_busy = true;
        let prompt = match choice {
            FolderChoice::ExportBase => self.tr(
                "상대 SQLite / CA 경로의 기준 폴더",
                "Base folder for relative SQLite / CA paths",
            ),
            FolderChoice::ImportRoot => self.tr(
                "가져올 상대 경로의 새 기준 폴더",
                "New root for imported relative paths",
            ),
            FolderChoice::OpenConfiguration => self.tr(
                "profiles.json이 있는 설정 폴더 열기",
                "Open a configuration folder containing profiles.json",
            ),
        };
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(prompt.into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            let picked = receiver.await;
            let _ = view.update(cx, |this, cx| {
                this.picker_busy = false;
                if this.closing {
                    cx.emit(SettingsTransferEvent::Close);
                    return;
                }
                match picked {
                    Ok(Ok(Some(mut paths))) => {
                        if let Some(folder) = paths.pop() {
                            match choice {
                                FolderChoice::ExportBase => this.export_base = folder,
                                FolderChoice::ImportRoot => {
                                    this.remap_root = Some(folder);
                                    this.destination = None;
                                    this.restored = None;
                                }
                                FolderChoice::OpenConfiguration => this.open_existing(folder, cx),
                            }
                        }
                    }
                    Ok(Err(error)) => this.feedback(error.to_string(), true),
                    Err(error) => this.feedback(error.to_string(), true),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() || self.closing {
            return;
        }
        self.picker_busy = true;
        let receiver = cx.prompt_for_new_path(&self.export_base, Some("dbide-settings.json"));
        cx.spawn_in(window, async move |view, cx| {
            let picked = receiver.await;
            let _ = view.update(cx, |this, cx| {
                this.picker_busy = false;
                if this.closing {
                    cx.emit(SettingsTransferEvent::Close);
                    return;
                }
                match picked {
                    Ok(Ok(Some(path))) => this.write_export(path, cx),
                    Ok(Err(error)) => this.feedback(error.to_string(), true),
                    Err(error) => this.feedback(error.to_string(), true),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn write_export(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.working = true;
        self.failed = false;
        self.feedback = None;
        let profile_path = self.profile_path.clone();
        let base = self.export_base.clone();
        let displayed = path.clone();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let (workspace, settings, recovery) = saved_configuration(&profile_path)?;
                    Bundle::capture(&workspace, settings, recovery, &base)
                        .and_then(|bundle| bundle.write_new(&path))
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.feedback(
                        format!(
                            "{} {}",
                            this.tr("JSON 저장 완료:", "JSON saved:"),
                            displayed.display()
                        ),
                        false,
                    ),
                    Err(error) => this.feedback(error, true),
                }
                this.finish(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn choose_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() || self.closing {
            return;
        }
        self.picker_busy = true;
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                self.tr(
                    "설정 JSON 선택 · 먼저 미리보기",
                    "Choose settings JSON · preview first",
                )
                .into(),
            ),
        });
        cx.spawn_in(window, async move |view, cx| {
            let picked = receiver.await;
            let _ = view.update(cx, |this, cx| {
                this.picker_busy = false;
                if this.closing {
                    cx.emit(SettingsTransferEvent::Close);
                    return;
                }
                match picked {
                    Ok(Ok(Some(mut paths))) => {
                        if let Some(path) = paths.pop() {
                            this.read_import(path, cx);
                        }
                    }
                    Ok(Err(error)) => this.feedback(error.to_string(), true),
                    Err(error) => this.feedback(error.to_string(), true),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn read_import(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.working = true;
        self.bundle = None;
        self.inspection = None;
        self.bundle_path = None;
        self.remap_root = None;
        self.destination = None;
        self.restored = None;
        self.feedback = None;
        let worker_path = path.clone();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Bundle::read(&worker_path)
                        .map(|bundle| {
                            let inspection = bundle.inspect();
                            (Arc::new(bundle), inspection)
                        })
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                match result {
                    Ok((bundle, inspection)) => {
                        this.bundle = Some(bundle);
                        this.inspection = Some(inspection);
                        this.remap_root =
                            Some(path.parent().unwrap_or(Path::new(".")).to_path_buf());
                        this.bundle_path = Some(path);
                        this.failed = false;
                    }
                    Err(error) => {
                        this.bundle_path = None;
                        this.remap_root = None;
                        this.feedback(error, true);
                    }
                }
                this.finish(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn choose_destination(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() || self.closing || self.bundle.is_none() {
            return;
        }
        self.picker_busy = true;
        let parent = self.export_base.clone();
        let receiver = cx.prompt_for_new_path(&parent, Some("dbide-restored"));
        cx.spawn_in(window, async move |view, cx| {
            let picked = receiver.await;
            let _ = view.update(cx, |this, cx| {
                this.picker_busy = false;
                if this.closing {
                    cx.emit(SettingsTransferEvent::Close);
                    return;
                }
                match picked {
                    Ok(Ok(Some(path))) => {
                        this.destination = Some(path);
                        this.restored = None;
                    }
                    Ok(Err(error)) => this.feedback(error.to_string(), true),
                    Err(error) => this.feedback(error.to_string(), true),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn restore(&mut self, cx: &mut Context<Self>) {
        if self.busy() || self.closing || self.restored.is_some() {
            return;
        }
        let (Some(bundle), Some(destination)) = (self.bundle.clone(), self.destination.clone())
        else {
            return;
        };
        let remap = self.remap_root.clone();
        self.working = true;
        self.feedback = None;
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    bundle
                        .restore_new_directory(&destination, remap.as_deref())
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                match result {
                    Ok(report) => {
                        let profile_path = report.profile_path.clone();
                        this.feedback(
                            format!(
                                "{} {}",
                                this.tr("새 설정 폴더 생성 완료:", "New configuration created:"),
                                report.directory.display()
                            ),
                            false,
                        );
                        this.restored = Some(report);
                        if !this.closing {
                            cx.emit(SettingsTransferEvent::OpenConfiguration(profile_path));
                        }
                    }
                    Err(error) => this.feedback(error, true),
                }
                this.finish(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn open_existing(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        self.working = true;
        self.feedback = None;
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { existing_configuration(&folder) })
                .await;
            let _ = view.update(cx, |this, cx| {
                match result {
                    Ok(path) if !this.closing => {
                        cx.emit(SettingsTransferEvent::OpenConfiguration(path))
                    }
                    Ok(_) => {}
                    Err(error) => this.feedback(error, true),
                }
                this.finish(cx);
            });
        })
        .detach();
        cx.notify();
    }
}

fn transfer_note(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Stateful<Div> {
    let value = value.into();
    div()
        .id(id)
        .role(Role::Label)
        .aria_label(value.clone())
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(value)
}

fn transfer_path(id: &'static str, label: &'static str, path: String) -> Stateful<Div> {
    use gpui_component::tooltip::Tooltip;
    let tooltip = path.clone();
    div()
        .id(id)
        .role(Role::Label)
        .aria_label(format!("{label}: {path}"))
        .flex_1()
        .min_w_0()
        .text_ellipsis()
        .child(path)
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
}

impl Render for SettingsTransferPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.busy();
        let can_restore =
            !busy && self.bundle.is_some() && self.destination.is_some() && self.restored.is_none();
        let export_base = self.export_base.display().to_string();
        let bundle_path = self
            .bundle_path
            .as_ref()
            .map(|path| path.display().to_string());
        let root = self
            .remap_root
            .as_ref()
            .map(|path| path.display().to_string());
        let destination = self
            .destination
            .as_ref()
            .map(|path| path.display().to_string());
        let preview = self.inspection.as_ref().map(|inspection| {
            let language = match inspection.app_settings.locale {
                Locale::Ko => "한국어",
                Locale::En => "English",
            };
            let selected = inspection.selected_profile_id.as_deref().unwrap_or("—");
            let recovery = if inspection.recovery_enabled {
                self.tr("켜짐", "on")
            } else {
                self.tr("꺼짐", "off")
            };
            [
                format!(
                    "{} {} · {} {}",
                    self.tr("연결 프로필", "Profiles"),
                    inspection.profile_count,
                    self.tr("선택 ID", "Selected ID"),
                    bounded_feedback(selected)
                ),
                format!(
                    "{language} · {} {} px · {} {} · {} {}",
                    self.tr("글꼴", "font"),
                    inspection.app_settings.editor_font_size,
                    self.tr("탭", "tab"),
                    inspection.app_settings.editor_tab_width,
                    self.tr("행/페이지", "rows/page"),
                    inspection.app_settings.table_page_size
                ),
                format!(
                    "{} {recovery} · {} {} · {} {}",
                    self.tr("SQL 복구", "SQL recovery"),
                    self.tr("상대 경로", "Relative paths"),
                    inspection.relative_path_count,
                    self.tr("외부 경로", "External paths"),
                    inspection.external_path_count
                ),
            ]
        });
        let warnings = self
            .restored
            .as_ref()
            .map(|report| &report.warnings)
            .or_else(|| {
                self.inspection
                    .as_ref()
                    .map(|inspection| &inspection.warnings)
            });
        let warnings: Vec<String> = warnings
            .map(|warnings| warnings.iter().take(6).map(bounded_feedback).collect())
            .unwrap_or_default();
        let operation_status = if self.picker_busy {
            self.tr("경로 선택 중…", "Choosing a path…")
        } else {
            self.tr("설정 파일 처리 중…", "Processing settings files…")
        };
        let restored_path = self
            .restored
            .as_ref()
            .map(|report| report.profile_path.clone());
        div().id("settings-transfer-panel").flex().flex_col().size_full().min_w_0().min_h_0()
            .bg(rgb(PANEL)).text_color(rgb(TEXT)).text_size(px(12.))
            .child(div().flex().items_center().gap_2().px_4().py_3().border_b_1().border_color(rgb(BORDER))
                .child(div().id("transfer-title").flex_1().font_weight(FontWeight::SEMIBOLD).role(Role::Label)
                    .aria_label(self.tr("설정 이동", "Settings transfer")).child(self.tr("설정 이동", "Settings transfer")))
                .child(Button::new("transfer-open-configuration").ghost().small().disabled(busy)
                    .label(self.tr("기존 설정 폴더 열기…", "Open config folder…"))
                    .on_click(cx.listener(|this, _, window, cx| this.choose_folder(FolderChoice::OpenConfiguration, window, cx))))
                .child(Button::new("transfer-close").ghost().small().disabled(self.closing)
                    .label(if self.closing { self.tr("닫는 중…", "Closing…") } else { self.tr("닫기", "Close") })
                    .on_click(cx.listener(|this, _, _, cx| this.request_close(cx)))))
            .child(div().id("settings-transfer-content").flex_1().min_h_0().overflow_y_scroll().p_4()
                .child(div().flex().flex_col().gap_4()
                    .child(transfer_note("transfer-included", self.tr("포함: 연결 프로필·선택한 연결·화면/편집기 설정·SQL 복구 사용 여부", "Included: connection profiles, selected connection, appearance/editor settings, and SQL recovery preference")))
                    .child(transfer_note("transfer-excluded", self.tr("제외: 비밀번호·Keychain·쿼리 기록·SQL 초안·실제 DB/CA 파일", "Excluded: passwords, Keychain entries, query history, SQL drafts, and actual database/CA files")))
                    .child(div().flex().flex_col().gap_2().pb_4().border_b_1().border_color(rgb(BORDER))
                        .child(div().id("transfer-export-heading").font_weight(FontWeight::SEMIBOLD).role(Role::Label).aria_label(self.tr("내보내기", "Export")).child(self.tr("내보내기", "Export")))
                        .child(transfer_note("transfer-export-description", self.tr("저장된 최신 설정을 읽어 새 JSON 파일로 저장합니다.", "Read the latest saved settings into a new JSON file.")))
                        .child(div().flex().items_center().gap_2()
                            .child(transfer_path("transfer-export-base-path", self.tr("내보내기 기준 폴더", "Export base"), export_base))
                            .child(Button::new("transfer-export-base").ghost().small().disabled(busy)
                                .label(self.tr("기준 폴더 선택…", "Choose base…"))
                                .on_click(cx.listener(|this, _, window, cx| this.choose_folder(FolderChoice::ExportBase, window, cx)))))
                        .child(transfer_note("transfer-export-paths-description", self.tr("기준 폴더 안의 SQLite·CA 경로는 상대 경로로, 외부 경로는 절대 경로로 기록합니다.", "SQLite/CA paths inside this base become relative; external paths remain absolute.")))
                        .child(Button::new("transfer-export-json").small().disabled(busy)
                            .label(self.tr("JSON 내보내기…", "Export JSON…"))
                            .on_click(cx.listener(|this, _, window, cx| this.export(window, cx)))))
                    .child(div().flex().flex_col().gap_2()
                        .child(div().id("transfer-import-heading").font_weight(FontWeight::SEMIBOLD).role(Role::Label).aria_label(self.tr("가져오기", "Import")).child(self.tr("가져오기", "Import")))
                        .child(transfer_note("transfer-import-description", self.tr("미리보기 후 새 설정 폴더를 만듭니다. 기존 폴더는 덮어쓰지 않습니다.", "Preview, then create a new configuration folder. Existing folders are never overwritten.")))
                        .child(Button::new("transfer-import-json").small().disabled(busy)
                            .label(self.tr("JSON 선택·미리보기…", "Choose JSON / preview…"))
                            .on_click(cx.listener(|this, _, window, cx| this.choose_import(window, cx))))
                        .when_some(bundle_path, |section, path| section.child(transfer_note("transfer-bundle-path", path)))
                        .when_some(preview, |section, lines| section.child(div().flex().flex_col().gap_1().py_2().children(lines.into_iter().enumerate().map(|(index, line)| transfer_note(("transfer-preview", index), line)))))
                        .when(self.inspection.is_some(), |section| section
                            .child(div().flex().items_center().gap_2()
                                .child(transfer_path("transfer-import-root-path", self.tr("가져오기 경로 기준 폴더", "Import path root"), root.unwrap_or_default()))
                                .child(Button::new("transfer-import-root").ghost().small().disabled(busy)
                                    .label(self.tr("경로 기준 폴더…", "Path root…"))
                                    .on_click(cx.listener(|this, _, window, cx| this.choose_folder(FolderChoice::ImportRoot, window, cx)))))
                            .child(transfer_note("transfer-import-paths-description", self.tr("위 폴더에서 상대 SQLite·CA 경로를 찾습니다. 기본값은 선택한 JSON 파일의 폴더입니다.", "Relative SQLite/CA paths resolve under the folder above; default is the selected JSON file's folder.")))
                            .when(self.inspection.as_ref().is_some_and(|inspection| inspection.external_path_count > 0), |section| section.child(transfer_note("transfer-import-external-description", self.tr("외부 절대 경로는 현재 OS 형식만 유지합니다. 다른 OS의 절대 경로는 CLI config import --map ORIGINAL=DESTINATION으로 매핑하세요.", "External absolute paths keep the current OS format. Map another OS's absolute paths with CLI config import --map ORIGINAL=DESTINATION."))))
                            .child(Button::new("transfer-destination").ghost().small().disabled(busy)
                                .label(self.tr("새 설정 폴더 이름 선택…", "Choose new config folder name…"))
                                .on_click(cx.listener(|this, _, window, cx| this.choose_destination(window, cx))))
                            .when_some(destination, |section, path| section.child(transfer_note("transfer-destination-path", format!("{} {path}", self.tr("새 폴더:", "New folder:")))))
                            .child(Button::new("transfer-restore").small().disabled(!can_restore)
                                .label(self.tr("복원 후 새 창 열기", "Restore and open new window"))
                                .on_click(cx.listener(|this, _, _, cx| this.restore(cx))))
                            .child(transfer_note("transfer-new-window-description", self.tr("새 창은 복원한 설정을 사용하며 DB에 자동 연결하지 않습니다.", "The new window uses restored settings and does not connect to a database automatically."))))
                        .children(warnings.into_iter().enumerate().map(|(index, warning)| transfer_note(("transfer-warning", index), warning).text_color(rgb(0xf2bd6b))))
                        .when_some(restored_path, |section, path| section.child(Button::new("transfer-reopen-restored").ghost().small().disabled(busy)
                            .label(self.tr("복원한 설정으로 새 창 열기", "Open restored configuration"))
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(SettingsTransferEvent::OpenConfiguration(path.clone())))))))))
            .when_some(self.feedback.clone(), |panel, feedback| panel.child(div().id("settings-transfer-feedback").role(Role::Status).aria_label(feedback.clone()).px_4().py_2().text_size(px(12.))
                .text_color(rgb(if self.failed { 0xf2bd6b } else { MUTED })).child(feedback)))
            .when(busy, |panel| panel.child(div().id("settings-transfer-status").role(Role::Status).aria_label(operation_status).px_4().py_2().text_color(rgb(MUTED))
                .child(operation_status)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn existing_configuration_rejects_missing_or_corrupt_files_before_opening() {
        let root = std::env::temp_dir().join(format!(
            "dbide-transfer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let profile = root.join("profiles.json");
        assert!(existing_configuration(&root).is_err());
        assert!(!profile.exists());
        std::fs::write(&profile, b"not JSON").unwrap();
        assert!(existing_configuration(&root).is_err());
        std::fs::write(
            &profile,
            serde_json::to_vec(&WorkspaceState::default()).unwrap(),
        )
        .unwrap();
        assert_eq!(existing_configuration(&root).unwrap(), profile);
        let settings = root.join("app-settings.json");
        std::fs::write(&settings, b"not JSON").unwrap();
        assert!(existing_configuration(&root).is_err());
        std::fs::remove_file(settings).unwrap();
        let recovery = root.join("editor-preferences.json");
        std::fs::write(&recovery, b"not JSON").unwrap();
        assert!(existing_configuration(&root).is_err());
        std::fs::write(&recovery, br#"{"enabled":true}"#).unwrap();
        assert_eq!(existing_configuration(&root).unwrap(), profile);
        let settings_store = dbide_settings::SettingsStore::for_profile_path(&profile);
        let changed_settings = AppSettings {
            editor_font_size: 20,
            ..AppSettings::default()
        };
        settings_store.save(&changed_settings).unwrap();
        let mut source = WorkspaceState {
            persist_history: true,
            ..WorkspaceState::default()
        };
        source.history.push(HistoryEntry {
            profile_id: "fixture".into(),
            sql: "SELECT secret_literal".into(),
            executed_at_unix_ms: 1,
            outcome: "success".into(),
            row_count: Some(1),
            execution_target: None,
        });
        std::fs::write(&profile, serde_json::to_vec(&source).unwrap()).unwrap();
        let (captured, saved_settings, enabled) = saved_configuration(&profile).unwrap();
        assert_eq!(saved_settings, changed_settings);
        assert!(enabled);
        assert!(captured.history.is_empty());
        assert!(!captured.persist_history);
        settings_store.save(&AppSettings::default()).unwrap();
        assert_eq!(
            saved_configuration(&profile).unwrap().1,
            AppSettings::default()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[::core::prelude::v1::test]
    fn error_feedback_respects_utf8_and_bound() {
        let feedback = bounded_feedback("한글".repeat(1000));
        assert!(feedback.len() <= 1024);
        assert!(feedback.ends_with('…'));
        assert_eq!(bounded_feedback("short error"), "short error");
    }
}
