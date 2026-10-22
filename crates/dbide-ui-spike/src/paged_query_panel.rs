use super::*;
use dbide_export::QueryRequest;
use dbide_result_store::live::{LiveMode, LiveQuery, LiveSnapshot, LiveState, start_live_query};
use dbide_result_store::{PageLimits, StoredPage};
use gpui_component::input::{Textarea, TextareaState};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, TryRecvError, sync_channel},
};

pub(super) enum PagedQueryEvent {
    Close,
}

struct PageOutcome {
    nonce: u64,
    identity: u64,
    index: usize,
    version: u64,
    published_pages: usize,
    page: Result<StoredPage, String>,
}

struct PageJob {
    nonce: u64,
    index: usize,
    cancel: Arc<AtomicBool>,
    terminal: Receiver<PageOutcome>,
}

/// One immutable query capture. Workers own source I/O and bounded page reads.
pub(super) struct PagedQueryPanel {
    request: QueryRequest,
    document_title: String,
    preview: Entity<TextareaState>,
    preview_clipped: bool,
    show_sql: bool,
    show_details: bool,
    locale: Locale,
    started: bool,
    store: Option<Arc<LiveQuery>>,
    snapshot: Option<LiveSnapshot>,
    page_job: Option<PageJob>,
    current_page: Option<usize>,
    wanted_page: Option<usize>,
    page_nonce: u64,
    displayed_version: u64,
    displayed_tail: bool,
    polling: bool,
    cancelling: bool,
    closing: bool,
    error: Option<String>,
    grid: Entity<TableState<PagedGrid>>,
    _grid_subscription: Subscription,
}

fn source_active(state: LiveState) -> bool {
    matches!(
        state,
        LiveState::Starting | LiveState::Streaming | LiveState::AwaitingFetch
    )
}

fn refresh_tail(index: usize, published_pages: usize) -> bool {
    index.saturating_add(1) >= published_pages
}

impl EventEmitter<PagedQueryEvent> for PagedQueryPanel {}

impl PagedQueryPanel {
    pub(super) fn focus_grid(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.grid.focus_handle(cx), cx);
    }

    pub(super) fn new(
        mut request: QueryRequest,
        document_title: String,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        request.options.max_rows = 1_000_000;
        request.options.max_bytes = 64 * 1024 * 1024;
        request.options.max_row_bytes = 512 * 1024;
        request.options.timeout = Duration::from_secs(30);
        let (preview_text, preview_clipped) =
            dbide_ui_spike::value_selection::value_preview(Some(&request.sql));
        let preview = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx);
            input.set_value(preview_text, window, cx);
            input
        });
        let grid = cx.new(|cx| {
            TableState::new(PagedGrid::empty(locale), window, cx)
                .loop_selection(false)
                .col_movable(false)
                .sortable(false)
                .cell_selectable(true)
                .row_header(false)
        });
        let subscription = cx.subscribe(&grid, |_, grid, event, cx| {
            if let TableEvent::SelectCell(row, col)
            | TableEvent::RightClickedCell(row, col)
            | TableEvent::DoubleClickedCell(row, col) = event
            {
                grid.update(cx, |state, cx| {
                    state.delegate_mut().selected = Some((*row, *col));
                    cx.notify();
                });
                cx.notify();
            }
        });
        Self {
            request,
            document_title,
            preview,
            preview_clipped,
            show_sql: true,
            show_details: false,
            locale,
            started: false,
            store: None,
            snapshot: None,
            page_job: None,
            current_page: None,
            wanted_page: None,
            page_nonce: 0,
            displayed_version: 0,
            displayed_tail: false,
            polling: false,
            cancelling: false,
            closing: false,
            error: None,
            grid,
            _grid_subscription: subscription,
        }
    }

    pub(super) fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.locale = locale;
        self.grid
            .update(cx, |grid, _| grid.delegate_mut().locale = locale);
        cx.notify();
    }

    pub(super) fn busy(&self) -> bool {
        self.page_job.is_some()
            || self
                .store
                .as_ref()
                .is_some_and(|store| source_active(store.snapshot().state))
    }

    pub(super) fn request_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            store.cancel();
        }
        if let Some(job) = &self.page_job {
            job.cancel.store(true, Ordering::Release);
        }
        self.wanted_page = None;
        self.cancelling = self.busy();
        self.ensure_polling(cx);
        cx.notify();
    }

    pub(super) fn request_close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.request_cancel(cx);
        if !self.busy() {
            cx.emit(PagedQueryEvent::Close);
        }
    }

    fn tr(&self, ko: &'static str, en: &'static str) -> &'static str {
        if self.locale == Locale::Ko { ko } else { en }
    }

    fn mode(&self) -> LiveMode {
        if self.request.connection.profile.engine == DatabaseEngine::PostgreSql {
            LiveMode::PostgresCursor
        } else {
            LiveMode::Stream
        }
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        if self.started || self.preview_clipped {
            return;
        }
        self.started = true;
        match start_live_query(self.request.clone(), PageLimits::default(), self.mode()) {
            Ok(query) => {
                let store = Arc::new(query);
                self.snapshot = Some(store.snapshot());
                self.store = Some(store);
                self.show_sql = false;
                self.ensure_polling(cx);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
    }

    fn ensure_polling(&mut self, cx: &mut Context<Self>) {
        if self.polling || (self.store.is_none() && self.page_job.is_none()) {
            return;
        }
        self.polling = true;
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
                let active = view
                    .update(cx, |this, cx| {
                        this.poll(cx);
                        let active = this.busy()
                            || this
                                .snapshot
                                .as_ref()
                                .is_some_and(|snapshot| source_active(snapshot.state));
                        if !active {
                            this.polling = false;
                        }
                        active
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        })
        .detach();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        if let Some(store) = &self.store {
            let snapshot = store.snapshot();
            changed = self.snapshot.as_ref().is_none_or(|old| {
                old.version != snapshot.version
                    || old.state != snapshot.state
                    || old.rows != snapshot.rows
                    || old.pages != snapshot.pages
            });
            self.snapshot = Some(snapshot);
        }
        if let Some(job) = &self.page_job {
            let index = job.index;
            let cancelled = job.cancel.load(Ordering::Acquire);
            match job.terminal.try_recv() {
                Ok(outcome) => {
                    let valid = !cancelled
                        && outcome.nonce == self.page_nonce
                        && outcome.nonce == job.nonce
                        && outcome.index == job.index
                        && (self.current_page != Some(index)
                            || outcome.version >= self.displayed_version)
                        && self
                            .store
                            .as_ref()
                            .is_some_and(|store| store.identity() == outcome.identity);
                    self.page_job = None;
                    if valid {
                        match outcome.page {
                            Ok(page) => {
                                let navigation = self.current_page != Some(index);
                                let has_rows = !page.rows.is_empty();
                                let update = self.grid.update(cx, |grid, cx| {
                                    if navigation {
                                        *grid.delegate_mut() = PagedGrid::new(page, self.locale);
                                        grid.clear_selection(cx);
                                        grid.refresh(cx);
                                        if has_rows {
                                            grid.scroll_to_row(0, cx);
                                        }
                                    } else {
                                        // Retain column widths, scroll position and raw-cell selection on append.
                                        grid.delegate_mut().append_page(page)?;
                                    }
                                    cx.notify();
                                    Ok::<(), String>(())
                                });
                                if let Err(error) = update {
                                    self.error = Some(error);
                                }
                                self.current_page = Some(index);
                                self.displayed_version = outcome.version;
                                self.displayed_tail = refresh_tail(index, outcome.published_pages);
                            }
                            Err(error) => self.error = Some(error),
                        }
                    }
                    changed = true;
                }
                Err(TryRecvError::Disconnected) => {
                    self.page_job = None;
                    self.error = Some(
                        self.tr(
                            "페이지 읽기 종료됨. 표시 중인 결과는 보존됩니다.",
                            "Page reader stopped. Displayed results are preserved.",
                        )
                        .into(),
                    );
                    changed = true;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if !self.closing && self.error.is_none() && self.page_job.is_none() {
            let snapshot = self.snapshot.as_ref();
            let pages = snapshot.map_or(0, |snapshot| snapshot.pages);
            let version = snapshot.map_or(0, |snapshot| snapshot.version);
            if self.wanted_page.is_some_and(|index| index < pages) {
                let index = self.wanted_page.take().unwrap();
                self.begin_page(index, cx);
            } else if self.current_page.is_none() && pages > 0 {
                self.begin_page(0, cx);
            } else if self.displayed_tail
                && version > self.displayed_version
                && let Some(index) = self.current_page
            {
                self.begin_page(index, cx);
            }
        }
        if !self.busy() {
            self.cancelling = false;
            self.wanted_page = None;
            if self.closing {
                cx.emit(PagedQueryEvent::Close);
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn begin_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.page_job.is_some() {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        let snapshot = store.snapshot();
        if index >= snapshot.pages {
            return;
        }
        self.page_nonce = self.page_nonce.wrapping_add(1);
        let nonce = self.page_nonce;
        let identity = store.identity();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (sender, terminal) = sync_channel(1);
        match std::thread::Builder::new()
            .name("paged-query-read".into())
            .spawn(move || {
                let read = if worker_cancel.load(Ordering::Acquire) {
                    Err("page read cancelled".into())
                } else {
                    store.read_page_versioned(index)
                };
                let (page, version) = match read {
                    Ok((page, version)) => (Ok(page), version),
                    Err(error) => (Err(error), snapshot.version),
                };
                let _ = sender.send(PageOutcome {
                    nonce,
                    identity,
                    index,
                    version,
                    published_pages: snapshot.pages,
                    page,
                });
            }) {
            Ok(_) => {
                self.page_job = Some(PageJob {
                    nonce,
                    index,
                    cancel,
                    terminal,
                })
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        self.ensure_polling(cx);
        cx.notify();
    }

    fn navigate(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.page_job.is_some() || self.closing {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        let snapshot = store.snapshot();
        if index < snapshot.pages {
            self.wanted_page = None;
            self.begin_page(index, cx);
        } else if index == snapshot.pages
            && snapshot.state == LiveState::AwaitingFetch
            && !self.cancelling
            && self.wanted_page.is_none()
        {
            match store.fetch_next() {
                Ok(()) => {
                    self.wanted_page = Some(index);
                    self.ensure_polling(cx);
                }
                Err(error) => self.error = Some(error),
            }
        }
        cx.notify();
    }

    fn copy_selected(&mut self, cx: &mut Context<Self>) {
        let text = self
            .grid
            .read(cx)
            .delegate()
            .selected_value()
            .map(|value| value.as_deref().unwrap_or("\\N").to_owned());
        if let Some(text) = text
            && text.len() <= 512 * 1024
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn status_text(&self) -> &'static str {
        if self.closing {
            return self.tr("닫는 중…", "Closing…");
        }
        if self.cancelling {
            return self.tr("취소 중…", "Cancelling…");
        }
        if self.error.is_some() {
            return self.tr("부분 결과 · 오류", "Partial · error");
        }
        match self.snapshot.as_ref().map(|snapshot| snapshot.state) {
            None if self.started => self.tr("실행 실패", "Failed to start"),
            None => self.tr("실행 준비", "Ready"),
            Some(LiveState::Starting) => {
                self.tr("연결 중 · 전체 미확정", "Starting · total unknown")
            }
            Some(LiveState::Streaming) => self.tr("수신 중 · 부분 결과", "Receiving · partial"),
            Some(LiveState::AwaitingFetch) => {
                self.tr("서버 커서 대기 · 부분 결과", "Cursor ready · partial")
            }
            Some(LiveState::Complete) => self.tr("완료", "Complete"),
            Some(LiveState::Cancelled) => self.tr("취소됨 · 부분 결과", "Cancelled · partial"),
            Some(LiveState::Failed) => self.tr("오류 · 부분 결과", "Failed · partial"),
        }
    }
}

impl Drop for PagedQueryPanel {
    fn drop(&mut self) {
        if let Some(store) = &self.store {
            store.cancel();
        }
        if let Some(job) = &self.page_job {
            job.cancel.store(true, Ordering::Release);
        }
    }
}

struct PagedGrid {
    page: Option<StoredPage>,
    columns: Vec<Column>,
    selected: Option<(usize, usize)>,
    locale: Locale,
}

impl PagedGrid {
    fn empty(locale: Locale) -> Self {
        Self {
            page: None,
            columns: Vec::new(),
            selected: None,
            locale,
        }
    }
    fn new(page: StoredPage, locale: Locale) -> Self {
        let names = page
            .columns
            .iter()
            .map(|name| dbide_ui_spike::grid_presentation::bounded_label(name, 128))
            .collect();
        Self {
            page: Some(page),
            columns: dbide_ui_spike::result_grid_columns(names),
            selected: None,
            locale,
        }
    }
    fn append_page(&mut self, mut incoming: StoredPage) -> Result<(), String> {
        let current = self.page.as_mut().ok_or("displayed page unavailable")?;
        if incoming.row_start != current.row_start
            || incoming.columns != current.columns
            || incoming.rows.len() < current.rows.len()
            || incoming.rows[..current.rows.len()] != current.rows
        {
            return Err("stored page prefix changed; displayed results preserved".into());
        }
        current
            .rows
            .extend(incoming.rows.drain(current.rows.len()..));
        Ok(())
    }

    fn selected_value(&self) -> Option<&Option<String>> {
        let (row, col) = self.selected?;
        self.page.as_ref()?.rows.get(row)?.get(col)
    }
}

impl TableDelegate for PagedGrid {
    fn context_menu(
        &mut self,
        _: usize,
        menu: gpui_component::menu::PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_component::menu::PopupMenu {
        use gpui_component::menu::PopupMenuItem;
        menu.item(
            PopupMenuItem::new(if self.locale == Locale::Ko {
                "셀 원문 복사 · NULL=\\N"
            } else {
                "Copy raw cell · NULL=\\N"
            })
            .disabled(self.selected.is_none())
            .action(Box::new(CopyCell)),
        )
    }

    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.page.as_ref().map_or(0, |page| page.rows.len())
    }
    fn column(&self, index: usize, _: &App) -> Column {
        self.columns[index].clone()
    }
    fn render_header(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        div()
            .id("paged-query-headers")
            .role(Role::Row)
            .aria_row_index(0)
    }
    fn render_th(
        &mut self,
        col: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .id(("paged-query-header", col))
            .role(Role::ColumnHeader)
            .aria_column_index(col)
            .text_color(rgb(TEXT))
            .font_weight(FontWeight::SEMIBOLD)
            .aria_label(self.columns[col].name.clone())
            .child(self.columns[col].name.clone())
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let count = self.page.as_ref().map_or(0, |page| page.rows.len());
        div()
            .id(("paged-query-row", row))
            .when(row < count, |el| el.role(Role::Row).aria_row_index(row + 1))
    }
    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        use gpui_component::menu::ContextMenuExt;
        let grid = cx.entity().clone();
        let raw = self
            .page
            .as_ref()
            .and_then(|page| page.rows.get(row))
            .and_then(|row| row.get(col))
            .and_then(Option::as_deref);
        let cell = dbide_ui_spike::grid_presentation::present_cell(raw);
        let global_row = self
            .page
            .as_ref()
            .map_or(row as u64 + 1, |page| page.row_start + row as u64 + 1);
        div()
            .id(("paged-query-cell", row * self.columns.len().max(1) + col))
            .role(Role::Cell)
            .aria_row_index(row + 1)
            .aria_column_index(col)
            .aria_selected(self.selected == Some((row, col)))
            .aria_label(format!(
                "{}, row {global_row}: {}",
                self.columns[col].name, cell.accessibility_value
            ))
            .aria_value(cell.accessibility_value)
            .size_full()
            .overflow_hidden()
            .text_ellipsis()
            .text_size(px(12.))
            .text_color(rgb(if raw.is_none() { MUTED } else { TEXT }))
            .child(cell.display)
            .context_menu(move |menu, window, cx| {
                grid.update(cx, |state, cx| {
                    state.set_selected_cell(row, col, cx);
                    state.delegate_mut().selected = Some((row, col));
                    let focus = state.focus_handle(cx);
                    state
                        .delegate_mut()
                        .context_menu(row, menu, window, cx)
                        .action_context(focus)
                })
            })
    }
}

impl Render for PagedQueryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let profile = &self.request.connection.profile;
        let target = profile
            .sqlite_path
            .clone()
            .unwrap_or_else(|| format!("{}:{}/{}", profile.host, profile.port, profile.database));
        let breadcrumb = format!("{}  /  {}", profile.name, self.document_title);
        let pages = self.snapshot.as_ref().map_or(0, |snapshot| snapshot.pages);
        let rows = self.snapshot.as_ref().map_or(0, |snapshot| snapshot.rows);
        let state = self.snapshot.as_ref().map(|snapshot| snapshot.state);
        let active = state.is_some_and(source_active);
        let fetching = self.wanted_page.is_some();
        let reading = self.page_job.is_some();
        let previous = self.current_page.unwrap_or(0).saturating_sub(1);
        let next = self.current_page.map_or(0, |index| index + 1);
        let cursor_next = next >= pages && state == Some(LiveState::AwaitingFetch);
        let can_previous =
            !reading && !self.closing && self.current_page.is_some_and(|index| index > 0);
        let can_next = !reading
            && !self.closing
            && !fetching
            && (next < pages || (cursor_next && !self.cancelling));
        let selected = self.grid.read(cx).delegate().selected_value().is_some();
        let range = self
            .grid
            .read(cx)
            .delegate()
            .page
            .as_ref()
            .map(|page| {
                if page.rows.is_empty() {
                    "0–0".into()
                } else {
                    format!(
                        "{}–{}",
                        page.row_start + 1,
                        page.row_start + page.rows.len() as u64
                    )
                }
            })
            .unwrap_or_else(|| "0–0".into());
        let page_position = if state == Some(LiveState::Complete) {
            format!(
                "{} {}/{}",
                self.tr("페이지", "page"),
                self.current_page.map_or(0, |index| index + 1),
                pages
            )
        } else {
            format!(
                "{} {} · {} {pages}",
                self.tr("페이지", "page"),
                self.current_page.map_or(0, |index| index + 1),
                self.tr("로드한 페이지", "pages loaded")
            )
        };
        let position = if self.started {
            let loaded = if state == Some(LiveState::Complete) {
                format!("{} {rows}", self.tr("전체", "total"))
            } else {
                format!(
                    "{} {rows} · {}",
                    self.tr("받은 행", "loaded"),
                    self.tr("전체 미확정", "total unknown")
                )
            };
            format!(
                "{} {range} · {loaded} · {page_position}",
                self.tr("행", "Rows")
            )
        } else {
            self.tr("캡처된 SQL 확인 후 실행", "Review captured SQL, then run")
                .into()
        };
        let status = self.status_text();
        let problem = self.error.clone().or_else(|| {
            self.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.error.clone())
        });
        let is_partial =
            self.error.is_some() || matches!(state, Some(LiveState::Cancelled | LiveState::Failed));
        let provenance = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.summary.as_ref())
            .map(|summary| {
                format!(
                    "{} {} · SQL SHA-256 {}",
                    self.tr("실행", "Execution"),
                    summary.execution_id,
                    summary.sql_sha256
                )
            });
        let mode = if self.mode() == LiveMode::PostgresCursor {
            self.tr(
                "PostgreSQL 서버 커서 · 다음 요청 시 FETCH",
                "PostgreSQL server cursor · FETCH on demand",
            )
        } else if profile.engine == DatabaseEngine::MySql {
            self.tr(
                "MySQL 스트리밍 · 수신 중 페이지 표시",
                "MySQL streaming · pages appear while receiving",
            )
        } else {
            self.tr(
                "SQLite 스트리밍 · 수신 중 페이지 표시",
                "SQLite streaming · pages appear while receiving",
            )
        };
        div().id("paged-query-panel").flex().flex_col().size_full().min_w_0().min_h_0()
            .bg(rgb(PANEL)).text_color(rgb(TEXT)).text_size(px(12.))
            .on_action(cx.listener(|this, _: &CopyCell, window, cx| {
                if this.grid.read(cx).focus_handle(cx).contains_focused(window, cx) {
                    this.copy_selected(cx);
                    cx.stop_propagation();
                }
            }))
            .child(div().flex().items_center().gap_2().px_3().py_2().border_b_1().border_color(rgb(BORDER))
                .child(div().min_w_0().flex_1().text_ellipsis().font_weight(FontWeight::SEMIBOLD).child(breadcrumb))
                .when(!self.started, |row| row.child(Button::new("paged-query-run").small().label(self.tr("실행", "Run"))
                    .disabled(self.preview_clipped).on_click(cx.listener(|this, _, _, cx| this.run(cx)))))
                .when(active, |row| row.child(Button::new("paged-query-cancel").ghost().small()
                    .label(if self.cancelling { self.tr("취소 중…", "Cancelling…") } else { self.tr("취소", "Cancel") })
                    .disabled(self.cancelling).on_click(cx.listener(|this, _, _, cx| this.request_cancel(cx)))))
                .child(Button::new("paged-query-toggle-sql").ghost().small()
                    .label(if self.show_sql { self.tr("SQL 접기", "Hide SQL") } else { "SQL" })
                    .on_click(cx.listener(|this, _, _, cx| { this.show_sql = !this.show_sql; cx.notify(); })))
                .child(Button::new("paged-query-details").ghost().small()
                    .label(if self.show_details { self.tr("세부 정보 접기", "Hide details") } else { self.tr("세부 정보", "Details") })
                    .on_click(cx.listener(|this, _, _, cx| { this.show_details = !this.show_details; cx.notify(); })))
                .child(Button::new("paged-query-close").ghost().small().label(self.tr("닫기", "Close"))
                    .disabled(self.closing).on_click(cx.listener(|this, _, _, cx| this.request_close(cx)))))
            .when(self.preview_clipped, |panel| panel.child(div().px_3().py_2().text_color(rgb(0xf2bd6b))
                .child(self.tr("SQL이 64 KiB 실행 한도를 초과합니다. 편집기에서 줄인 뒤 다시 여세요.", "SQL exceeds the 64 KiB execution limit. Shorten it in the editor and reopen."))))
            .when(self.show_sql, |panel| panel.child(div().px_3().py_2()
                .child(Textarea::new(&self.preview).readonly(true).h(px(if self.started { 92. } else { 140. }))
                    .aria_label(self.tr("캡처된 조회 SQL · 읽기 전용", "Captured query SQL · read only")))))
            .when(self.show_details, |panel| panel.child(div().flex().flex_col().gap_1().px_3().py_2()
                .text_color(rgb(MUTED)).text_size(px(11.)).border_b_1().border_color(rgb(BORDER))
                .child(target)
                .child(mode)
                .child(self.tr("열 때 고정한 SQL · 독립 READ ONLY 연결 · 임시 테이블과 미커밋 변경 공유 안 됨", "Captured SQL · independent READ ONLY connection · temporary tables and uncommitted changes are not shared"))
                .child(self.tr("최대 1,000,000행 / 64 MiB · 페이지당 200행 / 512 KiB · 실행 30초", "Limits: 1,000,000 rows / 64 MiB · 200 rows / 512 KiB per page · 30 seconds active query time"))
                .when(self.mode() == LiveMode::PostgresCursor, |details| details.child(self.tr("서버 커서 대기 60초 / 연결 수명 300초. 만료 시 받은 행은 보존되며 다시 열어 실행해야 합니다.", "Cursor idle limit 60s / lifetime 300s. Expiry preserves received rows; reopen to run again.")))
                .when_some(provenance, |details, provenance| details.child(provenance))))
            .when_some(problem, |panel, problem| panel.child(div().px_3().py_2().text_ellipsis().text_color(rgb(0xf2bd6b))
                .child(format!("{problem} {}", self.tr("받은 행은 보존됩니다. 다시 실행하려면 패널을 새로 여세요.", "Received rows are preserved. Reopen the panel to run again.")))))
            .child(div().key_context("PagedResultGrid").flex_1().min_h_0().min_w_0()
                .when(self.current_page.is_some(), |panel| panel.child(DataTable::new(&self.grid).small().stripe(false).bordered(false)))
                .when(self.current_page.is_none(), |panel| panel.child(div().p_3().text_color(rgb(MUTED))
                    .child(if self.started && !active { self.tr("받은 결과 없음", "No received results") }
                        else if self.started { self.tr("첫 결과를 기다리는 중…", "Waiting for first results…") }
                        else { self.tr("실행하면 수신된 결과부터 여기에 표시됩니다.", "Run to display results as they arrive.") }))))
            .child(div().flex().items_center().gap_2().px_3().py_2().border_t_1().border_color(rgb(BORDER))
                .child(Button::new("paged-query-previous").ghost().small().label(self.tr("이전", "Previous"))
                    .disabled(!can_previous).on_click(cx.listener(move |this, _, _, cx| this.navigate(previous, cx))))
                .child(Button::new("paged-query-next").ghost().small()
                    .label(if fetching { self.tr("수신 중…", "Fetching…") } else if cursor_next { self.tr("다음 가져오기", "Fetch next") } else { self.tr("다음", "Next") })
                    .disabled(!can_next).on_click(cx.listener(move |this, _, _, cx| this.navigate(next, cx))))
                .child(div().flex_1().min_w_0().text_ellipsis().text_color(rgb(MUTED)).child(position))
                .when(selected, |row| row.child(Button::new("paged-query-copy-cell").ghost().small().label(self.tr("셀 복사", "Copy cell"))
                    .on_click(cx.listener(|this, _, _, cx| this.copy_selected(cx)))))
                .child(div().id("paged-query-status").role(Role::Label).aria_label(status).px_2().py_1()
                    .rounded_md().text_size(px(11.)).text_color(rgb(if is_partial { 0xf2bd6b } else { MUTED })).child(status)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn only_live_source_states_keep_panel_busy() {
        for state in [
            LiveState::Starting,
            LiveState::Streaming,
            LiveState::AwaitingFetch,
        ] {
            assert!(source_active(state));
        }
        for state in [LiveState::Complete, LiveState::Cancelled, LiveState::Failed] {
            assert!(!source_active(state));
        }
    }

    #[::core::prelude::v1::test]
    fn growth_retains_raw_selection_and_columns() {
        let first = StoredPage {
            columns: vec!["value".into()],
            rows: vec![vec![None]],
            row_start: 200,
        };
        let mut grid = PagedGrid::new(first.clone(), Locale::En);
        grid.selected = Some((0, 0));
        let mut grown = first;
        grown.rows.push(vec![Some("".into())]);
        grid.append_page(grown).unwrap();
        assert_eq!(grid.selected, Some((0, 0)));
        assert_eq!(grid.selected_value(), Some(&None));
        assert_eq!(grid.page.as_ref().unwrap().rows.len(), 2);
        assert_eq!(grid.columns.len(), 1);
    }

    #[::core::prelude::v1::test]
    fn changed_prefix_never_overwrites_displayed_raw_cells() {
        let first = StoredPage {
            columns: vec!["value".into()],
            rows: vec![vec![Some("original".into())]],
            row_start: 0,
        };
        let mut grid = PagedGrid::new(first.clone(), Locale::En);
        let mut corrupt = first.clone();
        corrupt.rows[0][0] = Some("changed".into());
        assert!(grid.append_page(corrupt).is_err());
        assert_eq!(grid.page, Some(first));
    }

    #[::core::prelude::v1::test]
    fn tail_refresh_stops_after_page_is_sealed_and_reread() {
        assert!(refresh_tail(0, 1));
        assert!(!refresh_tail(0, 2));
        assert!(refresh_tail(1, 2));
    }
}
