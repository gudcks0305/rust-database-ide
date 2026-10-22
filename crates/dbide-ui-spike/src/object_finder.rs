//! Saved metadata finder. A single disk worker owns all filesystem reads.
use dbide_catalog_store::{
    SearchLimits, SearchMatch, SearchPage, SearchProfile, SearchQuery, Store,
};
use dbide_i18n::Locale;
use dbide_ui_spike::object_finder_model::FinderModel;
use gpui::{prelude::*, *};
use gpui_component::{
    Disableable, Sizable,
    button::Button,
    input::{Input, InputEvent, InputState},
    tooltip::Tooltip,
};
use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

pub(super) enum FinderEvent {
    Selected(Box<SearchMatch>),
    Close,
}
struct SearchJob {
    generation: u64,
    query: SearchQuery,
}
struct SearchReply {
    generation: u64,
    result: Result<SearchPage, String>,
}
#[derive(Default)]
struct Mailbox {
    pending: Option<SearchJob>,
    reply: Option<SearchReply>,
    shutdown: bool,
}
/// One active read plus one replaceable pending query, independent of typing speed.
struct SearchWorker {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
}
impl SearchWorker {
    fn start(root: PathBuf, profiles: Vec<SearchProfile>) -> Result<Self, String> {
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let background = shared.clone();
        std::thread::Builder::new()
            .name("saved-object-search".into())
            .spawn(move || {
                let store = Store::new(root);
                loop {
                    let (lock, ready) = &*background;
                    let job = {
                        let Ok(mut queue) = lock.lock() else {
                            return;
                        };
                        while queue.pending.is_none() && !queue.shutdown {
                            queue = match ready.wait(queue) {
                                Ok(queue) => queue,
                                Err(_) => return,
                            };
                        }
                        if queue.shutdown {
                            return;
                        }
                        queue.pending.take().expect("pending job")
                    };
                    let result =
                        store.search_objects(&profiles, &job.query, SearchLimits::default());
                    let Ok(mut queue) = lock.lock() else {
                        return;
                    };
                    if queue.shutdown {
                        return;
                    }
                    queue.reply = Some(SearchReply {
                        generation: job.generation,
                        result,
                    });
                }
            })
            .map_err(|error| format!("Object search worker unavailable: {error}"))?;
        Ok(Self { shared })
    }
    fn submit(&self, generation: u64, query: SearchQuery) {
        if let Ok(mut queue) = self.shared.0.lock() {
            queue.pending = Some(SearchJob { generation, query });
            self.shared.1.notify_one();
        }
    }
    fn poll(&self) -> Option<SearchReply> {
        self.shared.0.lock().ok()?.reply.take()
    }
}
impl Drop for SearchWorker {
    fn drop(&mut self) {
        if let Ok(mut queue) = self.shared.0.lock() {
            queue.shutdown = true;
            queue.pending = None;
            self.shared.1.notify_one();
        }
    }
}
pub(super) struct ObjectFinder {
    search: Entity<InputState>,
    kind: Option<String>,
    model: FinderModel,
    worker: Option<SearchWorker>,
    locale: Locale,
    placeholder_locale: Locale,
    scroll: ScrollHandle,
    profile_count: usize,
    debounce: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<FinderEvent> for ObjectFinder {}
impl ObjectFinder {
    pub(super) fn new(
        root: PathBuf,
        profiles: Vec<SearchProfile>,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = if locale == Locale::Ko {
            "객체 이름 또는 schema.name"
        } else {
            "Object name or schema.name"
        };
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        search.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe(&search, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.schedule(cx);
            }
        });
        let profile_count = profiles.len();
        let worker = SearchWorker::start(root, profiles);
        let mut model = FinderModel::default();
        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(error) => {
                model.error = Some(error);
                None
            }
        };
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if view
                    .update(cx, |this, cx| {
                        if let Some(reply) = this.worker.as_ref().and_then(SearchWorker::poll)
                            && this.model.complete(reply.generation, reply.result)
                        {
                            this.reveal_selection();
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            search,
            kind: None,
            model,
            worker,
            locale,
            placeholder_locale: locale,
            scroll: ScrollHandle::new(),
            profile_count,
            debounce: None,
            _subscriptions: vec![subscription],
        };
        this.schedule(cx);
        this
    }
    pub(super) fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.locale = locale;
        cx.notify();
    }
    fn tr(&self, ko: &'static str, en: &'static str) -> &'static str {
        if self.locale == Locale::Ko { ko } else { en }
    }
    fn schedule(&mut self, cx: &mut Context<Self>) {
        if self.worker.is_none() {
            return;
        }
        let generation = self.model.begin();
        self.scroll.scroll_to_item(0);
        let query = SearchQuery {
            text: self.search.read(cx).value().to_string(),
            kind: self.kind.clone(),
        };
        self.debounce = Some(cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(180))
                .await;
            let _ = view.update(cx, |this, _| {
                if this.model.generation == generation
                    && let Some(worker) = &this.worker
                {
                    worker.submit(generation, query);
                }
            });
        }));
        cx.notify();
    }
    fn choose(&mut self, cx: &mut Context<Self>) {
        if let Some(found) = self.model.selection().cloned() {
            cx.emit(FinderEvent::Selected(Box::new(found)));
        }
    }
    fn reveal_selection(&self) {
        if self.model.selection().is_some() {
            self.scroll.scroll_to_item(self.model.selected);
        }
    }
    fn capture(&mut self, action: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        match action {
            "up" | "down" => {
                self.model.move_selection(action == "down");
                self.reveal_selection();
            }
            "enter" => self.choose(cx),
            "escape" => cx.emit(FinderEvent::Close),
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }
    fn status(&self) -> String {
        if self.model.searching {
            return self
                .tr("저장된 메타데이터 검색 중…", "Searching saved metadata…")
                .into();
        }
        if let Some(error) = &self.model.error {
            return format!(
                "{}: {error}",
                self.tr("캐시 검색 불가", "Cache search unavailable")
            );
        }
        let Some(page) = &self.model.page else {
            return String::new();
        };
        if !page.cache_present {
            return self.tr("현재 프로필에 저장된 메타데이터 없음. 탐색기에서 분기를 펼쳐 메타데이터를 저장하세요.", "No saved metadata for current profiles. Expand Explorer branches to cache metadata.").into();
        }
        if page.partial {
            return format!(
                "{} · {} {} · {} {}",
                self.tr(
                    "제한된 부분집합 · 전체 검색 아님 (최대 100개 / 64 shards / 4 MiB)",
                    "Bounded subset · search incomplete (up to 100 results / 64 shards / 4 MiB)"
                ),
                page.matches.len(),
                self.tr("결과", "results"),
                page.skipped_shards,
                self.tr("파일 건너뜀", "files skipped")
            );
        }
        format!(
            "{} {} · {} {}",
            page.matches.len(),
            self.tr("저장된 객체", "saved objects"),
            page.scanned_shards,
            self.tr("객체 파일 검색", "object shards searched")
        )
    }
}
impl Render for ObjectFinder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.placeholder_locale != self.locale {
            let placeholder = self.tr("객체 이름 또는 schema.name", "Object name or schema.name");
            self.search.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx);
            });
            self.placeholder_locale = self.locale;
        }
        let mut filters = div().flex().gap_2().flex_wrap();
        for (index, (kind, ko, en)) in [
            (None, "전체 종류", "All kinds"),
            (Some("table"), "테이블", "Tables"),
            (Some("view"), "뷰", "Views"),
            (Some("materialized view"), "구체화 뷰", "Materialized views"),
        ]
        .into_iter()
        .enumerate()
        {
            let selected = self.kind.as_deref() == kind;
            filters = filters.child(
                Button::new(("finder-kind", index))
                    .small()
                    .label(format!(
                        "{}{}",
                        if selected { "✓ " } else { "" },
                        self.tr(ko, en)
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.kind = kind.map(String::from);
                        this.schedule(cx);
                    })),
            );
        }
        let matches = self
            .model
            .page
            .as_ref()
            .map(|page| page.matches.clone())
            .unwrap_or_default();
        let rendered_generation = self.model.generation;
        div()
            .id("saved-object-finder")
            .flex()
            .flex_col()
            .gap_3()
            .size_full()
            .min_w_0()
            .min_h_0()
            .p_4()
            .bg(rgb(0x18212b))
            .text_color(rgb(0xdce1e9))
            .text_size(px(12.))
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::MoveUp, window, cx| {
                    this.capture("up", window, cx)
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::MoveDown, window, cx| {
                    this.capture("down", window, cx)
                }),
            )
            .capture_action(cx.listener(
                |this, action: &gpui_component::input::Enter, window, cx| {
                    if !action.secondary && !action.shift {
                        this.capture("enter", window, cx);
                    }
                },
            ))
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::Escape, window, cx| {
                    this.capture("escape", window, cx)
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.tr("객체 찾기", "Find object")),
                    )
                    .child(
                        Button::new("close-object-finder")
                            .small()
                            .label(self.tr("닫기", "Close"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FinderEvent::Close))),
                    ),
            )
            .child(Input::new(&self.search).aria_label(self.tr(
                "저장된 데이터베이스 객체 검색",
                "Search saved database objects",
            )))
            .child(filters)
            .child(div().text_color(rgb(0x8b95a5)).child(format!(
                "{} {} · {}",
                self.profile_count,
                self.tr("현재 프로필", "current profiles"),
                self.tr(
                    "과거 메타데이터만 검색 · 연결과 SQL 실행 없음",
                    "Historical metadata only · no connection or SQL execution"
                )
            )))
            .child(
                div()
                    .id("object-finder-status")
                    .role(Role::Label)
                    .text_size(px(11.))
                    .text_color(rgb(0x8b95a5))
                    .aria_label(self.status())
                    .child(self.status()),
            )
            .child(
                div()
                    .id("object-finder-results")
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(matches.into_iter().enumerate().map(|(index, found)| {
                        let label = format!(
                            "{}.{} · {} · {} / {} · {} · {}",
                            found.relation.schema,
                            found.relation.name,
                            found.relation.kind,
                            found.profile.profile_name,
                            found.database,
                            found.profile.engine,
                            dbide_ui_spike::history_model::timestamp(found.captured_unix_ms)
                        );
                        let tooltip = label.clone();
                        div()
                            .id(("object-finder-result", index))
                            .role(Role::Button)
                            .aria_label(label.clone())
                            .aria_selected(self.model.selected == index)
                            .p_2()
                            .rounded_md()
                            .bg(rgb(if self.model.selected == index {
                                0x25334b
                            } else {
                                0x18212b
                            }))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x25334b)))
                            .tooltip(move |_, cx| cx.new(|_| Tooltip::new(tooltip.clone())).into())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_ellipsis()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(format!(
                                                "{}.{}",
                                                found.relation.schema, found.relation.name
                                            )),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_size(px(11.))
                                            .text_color(rgb(0x8b95a5))
                                            .child(found.relation.kind.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_ellipsis()
                                    .text_size(px(11.))
                                    .text_color(rgb(0x8b95a5))
                                    .child(format!(
                                        "{} / {} · {}",
                                        found.profile.profile_name,
                                        found.database,
                                        found.profile.engine
                                    )),
                            )
                            .child(
                                div()
                                    .text_ellipsis()
                                    .text_size(px(11.))
                                    .text_color(rgb(0x8b95a5))
                                    .child(format!(
                                        "{} · {}",
                                        self.tr("저장", "Saved"),
                                        dbide_ui_spike::history_model::timestamp(
                                            found.captured_unix_ms
                                        )
                                    )),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.model.generation == rendered_generation
                                    && !this.model.searching
                                {
                                    cx.emit(FinderEvent::Selected(Box::new(found.clone())));
                                }
                            }))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0x8b95a5))
                            .child(self.tr(
                                "↑↓ 선택 · Enter 구조 보기 · Esc 닫기",
                                "↑↓ select · Enter view structure · Esc close",
                            )),
                    )
                    .child(
                        div().flex().justify_end().child(
                            Button::new("open-found-object")
                                .small()
                                .label(self.tr("구조 보기", "View structure"))
                                .disabled(self.model.selection().is_none())
                                .on_click(cx.listener(|this, _, _, cx| this.choose(cx))),
                        ),
                    ),
            )
    }
}
