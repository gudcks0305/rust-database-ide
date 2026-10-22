//! Inline find for the immutable retained page. Explicit searches run off the UI.
use dbide_i18n::Locale;
use dbide_ui_spike::{
    ResultPage,
    result_find_model::{FindError, FindLimits, FindReport, search_page},
};
use gpui::{prelude::*, *};
use gpui_component::{
    Disableable, Sizable,
    button::Button,
    input::{self, Input, InputEvent, InputState},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) enum ResultFindEvent {
    /// Host must compare `Arc::ptr_eq` with its current grid page before selection.
    Selected {
        page: Arc<ResultPage>,
        row: usize,
        column: usize,
    },
    Close,
}

pub(super) struct ResultFind {
    page: Arc<ResultPage>,
    search: Entity<InputState>,
    case_sensitive: bool,
    report: Option<FindReport>,
    error: Option<FindError>,
    searching: bool,
    generation: u64,
    cancellation: Option<Arc<AtomicBool>>,
    task: Option<Task<()>>,
    locale: Locale,
    placeholder_locale: Locale,
    _subscription: Subscription,
}
impl EventEmitter<ResultFindEvent> for ResultFind {}
impl Drop for ResultFind {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancellation {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}
impl ResultFind {
    pub(super) fn new(
        page: Arc<ResultPage>,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(if locale == Locale::Ko {
                "현재 결과에서 텍스트 찾기"
            } else {
                "Find text in current result"
            })
        });
        search.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe(&search, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.invalidate();
                cx.notify();
            }
        });
        Self {
            page,
            search,
            case_sensitive: false,
            report: None,
            error: None,
            searching: false,
            generation: 0,
            cancellation: None,
            task: None,
            locale,
            placeholder_locale: locale,
            _subscription: subscription,
        }
    }
    pub(super) fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.locale = locale;
        cx.notify();
    }
    fn tr(&self, ko: &'static str, en: &'static str) -> &'static str {
        if self.locale == Locale::Ko { ko } else { en }
    }
    fn invalidate(&mut self) {
        if let Some(cancel) = self.cancellation.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.task = None;
        self.generation = self.generation.wrapping_add(1);
        self.report = None;
        self.error = None;
        self.searching = false;
    }
    fn find(&mut self, previous: bool, cx: &mut Context<Self>) {
        if self.searching {
            return;
        }
        if self.report.is_some() {
            self.navigate(!previous, cx);
            return;
        }
        let query = self.search.read(cx).value().to_string();
        if query.is_empty() {
            return;
        }
        self.invalidate();
        let generation = self.generation;
        let page = self.page.clone();
        let case_sensitive = self.case_sensitive;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.cancellation = Some(cancelled.clone());
        self.searching = true;
        self.task = Some(cx.spawn(async move |view, cx| {
            let report = cx
                .background_executor()
                .spawn(async move {
                    search_page(&page, &query, case_sensitive, FindLimits::default(), || {
                        cancelled.load(Ordering::Relaxed)
                    })
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.searching = false;
                this.cancellation = None;
                match report {
                    Ok(mut report) => {
                        if previous && !report.matches.is_empty() {
                            report.selected = report.matches.len() - 1;
                        }
                        this.report = Some(report);
                        this.choose(cx);
                    }
                    Err(FindError::Cancelled) => {}
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn choose(&self, cx: &mut Context<Self>) {
        if let Some(selected) = self.report.as_ref().and_then(FindReport::selection) {
            cx.emit(ResultFindEvent::Selected {
                page: self.page.clone(),
                row: selected.row,
                column: selected.column,
            });
        }
    }
    fn navigate(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(report) = self.report.as_mut() {
            report.move_selection(forward);
            self.choose(cx);
            cx.notify();
        }
    }
    fn close(&mut self, cx: &mut Context<Self>) {
        self.invalidate();
        cx.emit(ResultFindEvent::Close);
    }
    fn status(&self) -> String {
        if self.searching {
            return self
                .tr("현재 결과 검색 중…", "Searching current result…")
                .into();
        }
        if self.error == Some(FindError::QueryTooLong) {
            return self
                .tr("검색어는 최대 4 KiB", "Search text limit: 4 KiB")
                .into();
        }
        let Some(report) = &self.report else {
            return self
                .tr(
                    "Enter 검색 · Shift+Enter 이전 · SQL NULL 제외",
                    "Enter find · Shift+Enter previous · SQL NULL excluded",
                )
                .into();
        };
        let count = report.matches.len();
        let current = if count == 0 { 0 } else { report.selected + 1 };
        let mut status = format!(
            "{current}/{count} {} · {} {}",
            self.tr("일치 셀", "matching cells"),
            report.scanned_cells,
            self.tr("셀 검색", "cells searched")
        );
        if report.partial {
            status.push_str(self.tr(
                " · 부분 검색 (최대 10만 셀 / 8 MiB / 셀당 64 KiB / 1,000개)",
                " · Partial search (100k cells / 8 MiB / 64 KiB per cell / 1,000 matches max)",
            ));
        }
        status
    }
}
impl Render for ResultFind {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.placeholder_locale != self.locale {
            let placeholder = self.tr("현재 결과에서 텍스트 찾기", "Find text in current result");
            self.search.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx);
            });
            self.placeholder_locale = self.locale;
        }
        let has_matches = self.report.as_ref().is_some_and(|r| !r.matches.is_empty());
        let status = self.status();
        div()
            .id("result-find")
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(0x18212b))
            .text_color(rgb(0xdce1e9))
            .text_size(px(12.))
            .capture_action(cx.listener(|this, action: &input::Enter, window, cx| {
                if !action.secondary && this.search.read(cx).focus_handle(cx).is_focused(window) {
                    this.find(action.shift, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input::Escape, window, cx| {
                this.close(cx);
                window.prevent_default();
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.search).aria_label(
                        self.tr("현재 결과에서 텍스트 찾기", "Find text in current result"),
                    )))
                    .child(
                        Button::new("result-find-case")
                            .small()
                            .label(if self.case_sensitive { "✓ Aa" } else { "Aa" })
                            .tooltip(self.tr("대소문자 구분", "Match case"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.case_sensitive = !this.case_sensitive;
                                this.invalidate();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("result-find-run")
                            .small()
                            .label(self.tr("찾기", "Find"))
                            .disabled(self.searching)
                            .on_click(cx.listener(|this, _, _, cx| this.find(false, cx))),
                    )
                    .child(
                        Button::new("result-find-previous")
                            .small()
                            .label(self.tr("이전", "Previous"))
                            .disabled(!has_matches)
                            .on_click(cx.listener(|this, _, _, cx| this.navigate(false, cx))),
                    )
                    .child(
                        Button::new("result-find-next")
                            .small()
                            .label(self.tr("다음", "Next"))
                            .disabled(!has_matches)
                            .on_click(cx.listener(|this, _, _, cx| this.navigate(true, cx))),
                    )
                    .child(
                        Button::new("result-find-close")
                            .small()
                            .label(self.tr("닫기", "Close"))
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .child(
                div()
                    .id("result-find-status")
                    .role(Role::Label)
                    .aria_label(status.clone())
                    .text_color(rgb(0x8b95a5))
                    .child(status),
            )
    }
}
