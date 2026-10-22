use super::*;
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
// Reserve space for chrome, wrapped controls and usable results at the minimum window size.
pub(super) fn editor_panel_height(preferred: u16, viewport_height: f32) -> u16 {
    preferred.min((viewport_height - 450.).clamp(120., 720.) as u16)
}

impl Workbench {
    pub(super) fn result_more_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let import_view = cx.weak_entity();
        let json_view = import_view.clone();
        let csv_view = import_view.clone();
        let available = !self.logical_mode
            && self
                .pages
                .get(self.selected_page)
                .is_some_and(|p| !p.command_only);
        let locale = self.locale;
        let selected = self.result_has_selection(cx);
        let rows = self.result_has_rows(cx);
        Button::new("result-more")
            .small()
            .ghost()
            .label("⋯")
            .accessibility_label(if locale == Locale::Ko {
                "결과 작업"
            } else {
                "Result actions"
            })
            .tooltip(if locale == Locale::Ko {
                "값 보기·복사·가져오기·내보내기"
            } else {
                "Inspect, copy, import and export"
            })
            .dropdown_menu(move |menu, _, _| {
                let import_view = import_view.clone();
                let json_view = json_view.clone();
                let csv_view = csv_view.clone();
                menu.item(
                    PopupMenuItem::new(if locale == Locale::Ko {
                        "값 상세 보기"
                    } else {
                        "Inspect value"
                    })
                    .disabled(!selected)
                    .action(Box::new(InspectValue)),
                )
                .item(
                    PopupMenuItem::new(if locale == Locale::Ko {
                        "레코드 보기"
                    } else {
                        "Inspect record"
                    })
                    .disabled(!selected)
                    .action(Box::new(InspectRecord)),
                )
                .item(
                    PopupMenuItem::new(if locale == Locale::Ko {
                        "행 복사 · TSV"
                    } else {
                        "Copy row · TSV"
                    })
                    .disabled(!selected)
                    .action(Box::new(CopyResultRow)),
                )
                .item(
                    PopupMenuItem::new(if locale == Locale::Ko {
                        "열 복사 · TSV"
                    } else {
                        "Copy column · TSV"
                    })
                    .disabled(!selected)
                    .action(Box::new(CopyResultColumn)),
                )
                .item(
                    PopupMenuItem::new(if locale == Locale::Ko {
                        "범위 복사…"
                    } else {
                        "Copy range…"
                    })
                    .disabled(!rows)
                    .action(Box::new(CopyResultRange)),
                )
                .separator()
                .item(
                    PopupMenuItem::new(if locale == Locale::Ko {
                        "가져오기 미리보기…"
                    } else {
                        "Import preview…"
                    })
                    .on_click(move |_, w, cx| {
                        let _ = import_view.update(cx, |s, cx| s.import_preview(w, cx));
                    }),
                )
                .item(
                    PopupMenuItem::new(if locale == Locale::Ko {
                        "JSON 내보내기…"
                    } else {
                        "Export JSON…"
                    })
                    .disabled(!available)
                    .on_click(move |_, w, cx| {
                        let _ = json_view.update(cx, |s, cx| s.export_result(true, w, cx));
                    }),
                )
                .item(
                    PopupMenuItem::new(if locale == Locale::Ko {
                        "CSV 내보내기…"
                    } else {
                        "Export CSV…"
                    })
                    .disabled(!available)
                    .on_click(move |_, w, cx| {
                        let _ = csv_view.update(cx, |s, cx| s.save_file(true, w, cx));
                    }),
                )
            })
            .into_any_element()
    }

    pub(super) fn split_handle(&self, horizontal: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(if horizontal {
                "catalog-divider"
            } else {
                "editor-results-divider"
            })
            .role(Role::GenericContainer)
            .aria_label(if horizontal {
                if self.locale == Locale::Ko {
                    "탐색기 너비 조절"
                } else {
                    "Resize catalog"
                }
            } else if self.locale == Locale::Ko {
                "편집기와 결과 높이 조절"
            } else {
                "Resize editor and results"
            })
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .when(horizontal, |el| {
                el.w(px(5.)).h_full().cursor(CursorStyle::ResizeLeftRight)
            })
            .when(!horizontal, |el| {
                el.h(px(5.)).w_full().cursor(CursorStyle::ResizeUpDown)
            })
            .hover(|el| el.bg(rgb(ACCENT)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |s, event: &MouseDownEvent, window, cx| {
                    let initial = if horizontal {
                        s.app_settings.sidebar_width
                    } else {
                        s.app_settings.editor_height
                    };
                    s.resize_drag = Some((horizontal, event.position, initial as f32));
                    window.prevent_default();
                    cx.stop_propagation();
                }),
            )
    }
    pub(super) fn resize_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((horizontal, origin, initial)) = self.resize_drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.finish_resize(cx);
            return;
        }
        if horizontal {
            self.app_settings.sidebar_width = (initial + f32::from(event.position.x - origin.x))
                .clamp(180., 480.)
                .round() as u16;
        } else {
            self.app_settings.editor_height = (initial + f32::from(event.position.y - origin.y))
                .clamp(120., 720.)
                .round() as u16;
        }
        self.app_settings.editor_height = editor_panel_height(
            self.app_settings.editor_height,
            f32::from(window.viewport_size().height),
        );
        cx.notify();
    }
    pub(super) fn resize_end(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_resize(cx);
    }
    fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if self.resize_drag.take().is_some() {
            self.queue_settings_change(
                dbide_settings::SettingsChange::Layout {
                    sidebar_width: self.app_settings.sidebar_width,
                    editor_height: self.app_settings.editor_height,
                },
                cx,
            );
        }
    }
    fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut tabs: Vec<(bool, u64)> =
            self.documents.iter().map(|d| (false, d.model.id)).collect();
        tabs.extend(self.table_tabs.iter().map(|t| (true, t.id)));
        if tabs.is_empty() {
            return;
        }
        let active = self
            .active_table_tab
            .map(|id| (true, id))
            .unwrap_or((false, self.active_document));
        let current = tabs.iter().position(|v| *v == active).unwrap_or(0);
        let next = if forward {
            (current + 1) % tabs.len()
        } else {
            (current + tabs.len() - 1) % tabs.len()
        };
        let (table, id) = tabs[next];
        if table {
            self.show_table_tab(id, window, cx);
        } else {
            self.active_table_tab = None;
            self.switch_document(id, cx);
            self.editor.update(cx, |e, cx| e.focus(window, cx));
        }
        cx.notify();
    }
    pub(super) fn next_workspace_tab(
        &mut self,
        _: &NextWorkspaceTab,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_tab(true, w, cx);
    }
    pub(super) fn previous_workspace_tab(
        &mut self,
        _: &PreviousWorkspaceTab,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_tab(false, w, cx);
    }
    pub(super) fn focus_catalog(
        &mut self,
        _: &FocusCatalog,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.catalog_collapsed = false;
        self.object_search.update(cx, |s, cx| s.focus(w, cx));
        cx.notify();
    }
    pub(super) fn focus_results(
        &mut self,
        _: &FocusResults,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.active_table_entity() {
            editor.update(cx, |e, cx| e.focus_results(w, cx));
        } else {
            self.results_collapsed = false;
            w.focus(&self.grid.focus_handle(cx), cx);
        }
        cx.notify();
    }
    pub(super) fn toggle_results(
        &mut self,
        _: &ToggleResults,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.results_collapsed = !self.results_collapsed;
        cx.notify();
    }
    pub(super) fn toggle_catalog(
        &mut self,
        _: &ToggleCatalog,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.catalog_collapsed = !self.catalog_collapsed;
        cx.notify();
    }
}
