use dbide_i18n::Locale;
use dbide_profiles::HistoryEntry;
use dbide_ui_spike::history_model::{HistoryModel, captured_database, sql_excerpt, timestamp};
use gpui::{prelude::*, *};
use gpui_component::{
    Disableable, Sizable,
    button::Button,
    input::{self, Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    tooltip::Tooltip,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub enum HistoryPanelEvent {
    Open(HistoryEntry),
    Close,
}
impl EventEmitter<HistoryPanelEvent> for HistoryPanel {}

pub struct HistoryPanel {
    locale: Locale,
    model: HistoryModel,
    search: Entity<InputState>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    _subscription: Subscription,
}
fn text(locale: Locale, ko: &'static str, en: &'static str) -> &'static str {
    if locale == Locale::Ko { ko } else { en }
}

impl HistoryPanel {
    pub fn new(
        locale: Locale,
        entries: Vec<HistoryEntry>,
        profile_names: BTreeMap<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx));
        search.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe(&search, |s, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                s.model.search = input.read(cx).value().to_string();
                s.model.refresh();
                s.reveal_selection();
                cx.notify();
            }
        });
        let mut model = HistoryModel::default();
        model.update(entries, profile_names);
        Self {
            locale,
            model,
            search,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            _subscription: subscription,
        }
    }
    pub fn update(
        &mut self,
        entries: Vec<HistoryEntry>,
        profile_names: BTreeMap<String, String>,
        locale: Locale,
        cx: &mut Context<Self>,
    ) {
        self.model.update(entries, profile_names);
        self.reveal_selection();
        self.locale = locale;
        cx.notify();
    }
    fn open(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.model.selected().cloned() {
            cx.emit(HistoryPanelEvent::Open(entry));
        }
    }
    fn reveal_selection(&self) {
        if let Some(index) = self
            .model
            .visible
            .iter()
            .position(|index| self.model.selected() == Some(&self.model.entries[*index]))
        {
            self.scroll.scroll_to_item(index);
        }
    }
    fn capture(&mut self, action: &str, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            "up" | "down" => {
                self.model.move_selection(action == "down");
                self.reveal_selection();
            }
            "enter" => self.open(cx),
            "escape" => cx.emit(HistoryPanelEvent::Close),
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }
}
impl Render for HistoryPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.locale;
        let names = self.model.profiles();
        let weak = cx.weak_entity();
        let profile_label = self
            .model
            .profile
            .as_ref()
            .and_then(|id| names.get(id))
            .cloned()
            .or_else(|| self.model.profile.clone())
            .unwrap_or_else(|| text(locale, "모든 프로필", "All profiles").into());
        let profile_filter = Button::new("history-profile")
            .small()
            .label(format!("{profile_label} ▾"))
            .dropdown_menu(move |mut menu, _, _| {
                let reset = weak.clone();
                menu = menu.item(
                    PopupMenuItem::new(text(locale, "모든 프로필", "All profiles")).on_click(
                        move |_, _, cx| {
                            let _ = reset.update(cx, |s, cx| {
                                s.model.profile = None;
                                s.model.refresh();
                                s.reveal_selection();
                                cx.notify();
                            });
                        },
                    ),
                );
                for (id, name) in &names {
                    let weak = weak.clone();
                    let id = id.clone();
                    menu = menu.item(PopupMenuItem::new(format!("{name} · {id}")).on_click(
                        move |_, _, cx| {
                            let _ = weak.update(cx, |s, cx| {
                                s.model.profile = Some(id.clone());
                                s.model.refresh();
                                s.reveal_selection();
                                cx.notify();
                            });
                        },
                    ));
                }
                menu
            });
        let weak = cx.weak_entity();
        let outcomes: BTreeSet<String> = [
            "Completed",
            "Failed",
            "Cancelled",
            "OutcomeUnknown",
            "Rejected",
        ]
        .into_iter()
        .map(str::to_string)
        .chain(self.model.entries.iter().map(|entry| entry.outcome.clone()))
        .collect();
        let outcome_label = self
            .model
            .outcome
            .clone()
            .unwrap_or_else(|| text(locale, "모든 결과", "All outcomes").into());
        let outcome_filter = Button::new("history-outcome")
            .small()
            .label(format!("{outcome_label} ▾"))
            .dropdown_menu(move |mut menu, _, _| {
                let reset = weak.clone();
                menu = menu.item(
                    PopupMenuItem::new(text(locale, "모든 결과", "All outcomes")).on_click(
                        move |_, _, cx| {
                            let _ = reset.update(cx, |s, cx| {
                                s.model.outcome = None;
                                s.model.refresh();
                                s.reveal_selection();
                                cx.notify();
                            });
                        },
                    ),
                );
                for outcome in &outcomes {
                    let weak = weak.clone();
                    let outcome = outcome.clone();
                    menu = menu.item(PopupMenuItem::new(outcome.clone()).on_click(
                        move |_, _, cx| {
                            let _ = weak.update(cx, |s, cx| {
                                s.model.outcome = Some(outcome.clone());
                                s.model.refresh();
                                s.reveal_selection();
                                cx.notify();
                            });
                        },
                    ));
                }
                menu
            });
        let rows = self
            .model
            .visible
            .iter()
            .map(|index| {
                let index = *index;
                let entry = &self.model.entries[index];
                let profile = self
                    .model
                    .profile_names
                    .get(&entry.profile_id)
                    .cloned()
                    .unwrap_or_else(|| entry.profile_id.clone());
                let database = captured_database(entry).unwrap_or(text(
                    locale,
                    "대상 미기록",
                    "Target not captured",
                ));
                let count = entry
                    .row_count
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".into());
                let target = format!("{profile} / {database}");
                let target_tooltip = target.clone();
                div()
                    .id(("history-entry", index))
                    .role(Role::Button)
                    .aria_selected(self.model.selected() == Some(entry))
                    .aria_label(format!(
                        "{profile} / {database} · {} · {}",
                        entry.outcome,
                        sql_excerpt(&entry.sql, 160)
                    ))
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0x2a3644))
                    .cursor_pointer()
                    .when(self.model.selected() == Some(entry), |row| {
                        row.bg(rgb(0x24364b))
                    })
                    .on_click(cx.listener(move |s, _, window, cx| {
                        s.model.select(index);
                        s.focus.focus(window, cx);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(11.))
                            .text_color(rgb(0x9aa9bd))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_ellipsis()
                                    .child(timestamp(entry.executed_at_unix_ms)),
                            )
                            .child(div().flex_shrink_0().child(entry.outcome.clone())),
                    )
                    .child(
                        div()
                            .id(("history-target", index))
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(11.))
                            .text_color(rgb(0x9aa9bd))
                            .tooltip(move |_, cx| {
                                cx.new(|_| Tooltip::new(target_tooltip.clone())).into()
                            })
                            .child(div().min_w_0().flex_1().text_ellipsis().child(target))
                            .child(div().flex_shrink_0().child(format!(
                                "{count} {}",
                                text(locale, "보관 행", "retained rows")
                            ))),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_ellipsis()
                            .child(sql_excerpt(&entry.sql, 160).replace(['\n', '\r'], " ")),
                    )
            })
            .collect::<Vec<_>>();
        div()
            .id("query-history-panel")
            .track_focus(&self.focus)
            .key_context("Input")
            .capture_action(cx.listener(|s, _: &input::MoveUp, w, cx| s.capture("up", w, cx)))
            .capture_action(cx.listener(|s, _: &input::MoveDown, w, cx| s.capture("down", w, cx)))
            .capture_action(cx.listener(|s, a: &input::Enter, w, cx| {
                if !a.secondary && !a.shift {
                    s.capture("enter", w, cx);
                }
            }))
            .capture_action(cx.listener(|s, _: &input::Escape, w, cx| s.capture("escape", w, cx)))
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .pt_2()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(text(
                        locale,
                        "쿼리 기록",
                        "Query history",
                    )))
                    .child(div().flex_1())
                    .child(
                        Button::new("history-close")
                            .label(text(locale, "닫기", "Close"))
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(HistoryPanelEvent::Close))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0x9aa9bd))
                            .child(text(locale, "검색", "Search")),
                    )
                    .child(div().flex_1().min_w_0().child(
                        Input::new(&self.search).small().aria_label(text(
                            locale,
                            "SQL · 프로필 · 데이터베이스 검색",
                            "Search SQL, profile or database",
                        )),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .px_3()
                    .child(profile_filter)
                    .child(outcome_filter),
            )
            .child(
                div()
                    .px_3()
                    .text_size(px(11.))
                    .text_color(rgb(0x9aa9bd))
                    .child(format!(
                        "{} / {} · {}",
                        self.model.visible.len(),
                        self.model.entries.len(),
                        text(
                            locale,
                            "최신순 · ↑↓ 선택 · Enter 초안 열기",
                            "Newest first · ↑↓ select · Enter opens draft"
                        )
                    )),
            )
            .child(
                div()
                    .id("history-results")
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(self.model.visible.is_empty(), |list| {
                        list.child(div().p_4().text_color(rgb(0x9aa9bd)).child(
                            if self.model.entries.is_empty() {
                                text(
                                    locale,
                                    "아직 실행 기록이 없습니다.",
                                    "No query history yet.",
                                )
                            } else {
                                text(
                                    locale,
                                    "검색 조건과 일치하는 기록이 없습니다.",
                                    "No history matches these filters.",
                                )
                            },
                        ))
                    }),
            )
            .when_some(self.model.selected(), |panel, entry| {
                panel.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(rgb(0x2a3644))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(0x9aa9bd))
                                .child(text(
                                    locale,
                                    "선택한 SQL 미리보기 · 읽기 전용",
                                    "Selected SQL preview · read only",
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .max_h(px(100.))
                                .overflow_hidden()
                                .child(sql_excerpt(&entry.sql, 2000)),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .px_3()
                    .pb_2()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0x9aa9bd))
                            .child(text(
                                locale,
                                "SQL과 기록된 대상을 초안으로 복원 · 자동 실행 안 함",
                                "Restore SQL and captured target as draft · no automatic execution",
                            )),
                    )
                    .child(
                        div().flex().justify_end().child(
                            Button::new("history-open")
                                .small()
                                .label(text(locale, "SQL 초안 열기", "Open SQL draft"))
                                .disabled(self.model.selected().is_none())
                                .on_click(cx.listener(|s, _, _, cx| s.open(cx))),
                        ),
                    ),
            )
    }
}
