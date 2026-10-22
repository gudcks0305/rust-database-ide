use super::*;
use std::path::PathBuf;

pub(super) struct LoadedConfiguration {
    pub store: ProfileStore,
    pub workspace: WorkspaceState,
}

fn load_configuration(path: PathBuf) -> Result<LoadedConfiguration, String> {
    let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let store = ProfileStore::open(path).map_err(|e| e.to_string())?;
    let workspace = store.load().map_err(|e| e.to_string())?;
    Ok(LoadedConfiguration { store, workspace })
}

#[derive(Default)]
struct ConfigurationWindows(Vec<(AnyWindowHandle, WeakEntity<Workbench>)>);
impl Global for ConfigurationWindows {}

pub(super) fn register(window: AnyWindowHandle, view: WeakEntity<Workbench>, cx: &mut App) {
    if !cx.has_global::<ConfigurationWindows>() {
        cx.set_global(ConfigurationWindows::default());
    }
    cx.update_global::<ConfigurationWindows, _>(|windows, _| {
        windows.0.retain(|(_, view)| view.upgrade().is_some());
        windows.0.push((window, view));
    });
}

/// Every window follows its own recovery, transaction and writer-close checks.
/// A configuration window may never quit the process underneath another window.
pub(super) fn request_quit(cx: &mut App) {
    let windows = cx.global::<ConfigurationWindows>().0.clone();
    cx.defer(move |cx| {
        for (handle, view) in windows {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = view.update(cx, |view, cx| {
                    view.quit_after_close = true;
                    if view.request_window_close(window, cx) {
                        view.finish_window_close(window, cx);
                    } else if !view.close_prompt_open
                        && !view.recovery.close_after
                        && !view.close_after_streams
                    {
                        view.quit_after_close = false;
                    }
                });
            });
        }
    });
}

fn open_configuration(configuration: LoadedConfiguration, cx: &mut App) {
    let windows = cx.global::<ConfigurationWindows>().0.clone();
    for (window, view) in windows {
        if let Some(view) = view.upgrade()
            && view
                .read(cx)
                .store
                .as_ref()
                .is_some_and(|s| s.path().parent() == configuration.store.path().parent())
        {
            let _ = window.update(cx, |_, w, _| w.activate_window());
            return;
        }
    }
    open_workbench_with_configuration(cx, Instant::now(), false, None, Some(configuration));
}

impl Workbench {
    pub(super) fn open_settings_transfer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_transfer.is_some() {
            self.stream_export_visible = false;
            cx.notify();
            return;
        }
        if self.settings_busy
            || !self.settings_pending.is_empty()
            || self.profile_write_busy
            || !self.profile_writes.is_empty()
            || self.recovery.loading
            || self.recovery.preference_busy
        {
            self.settings_error = Some(
                if self.locale == Locale::Ko {
                    "설정 저장 중입니다. 완료 후 설정 이동을 다시 여세요."
                } else {
                    "Settings are being saved. Open transfer again after saving completes."
                }
                .into(),
            );
            cx.notify();
            return;
        }
        let Some(store) = &self.store else {
            return;
        };
        let profile_path = store.path().to_path_buf();
        let panel = cx.new(|cx| {
            portable_settings::SettingsTransferPanel::new(
                self.workspace.clone(),
                self.app_settings,
                self.recovery.enabled,
                profile_path,
                self.locale,
                window,
                cx,
            )
        });
        self.settings_transfer_subscription =
            Some(cx.subscribe_in(&panel, window, |s, source, event, w, cx| {
                if s.settings_transfer
                    .as_ref()
                    .is_none_or(|p| p.entity_id() != source.entity_id())
                {
                    return;
                }
                match event {
                    portable_settings::SettingsTransferEvent::Close => {
                        s.settings_transfer = None;
                        s.settings_transfer_subscription = None;
                        s.editor.update(cx, |editor, cx| editor.focus(w, cx));
                    }
                    portable_settings::SettingsTransferEvent::OpenConfiguration(path) => {
                        if !s.close_after_streams && !s.quit_after_close {
                            let path = path.clone();
                            cx.spawn(async move |view, cx| {
                                let result = cx
                                    .background_executor()
                                    .spawn(async move { load_configuration(path) })
                                    .await;
                                let _ = view.update(cx, |s, cx| {
                                    match result {
                                        Ok(configuration)
                                            if !s.close_after_streams && !s.quit_after_close =>
                                        {
                                            cx.defer(move |cx| {
                                                open_configuration(configuration, cx)
                                            })
                                        }
                                        Ok(_) => {}
                                        Err(error) => {
                                            s.diagnostic = Some(Diagnostic::new((
                                                "Could not open configuration".into(),
                                                error,
                                                "The current workspace was not changed.".into(),
                                            )))
                                        }
                                    }
                                    cx.notify();
                                });
                            })
                            .detach();
                        }
                    }
                }
                cx.notify();
            }));
        self.preferences = None;
        self.stream_export_visible = false;
        self.settings_transfer = Some(panel);
        cx.notify();
    }

    pub(super) fn settings_transfer_busy(&self, cx: &App) -> bool {
        self.settings_transfer
            .as_ref()
            .is_some_and(|p| p.read(cx).busy())
    }
}
