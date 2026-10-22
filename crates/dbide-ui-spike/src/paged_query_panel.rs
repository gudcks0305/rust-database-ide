use super::*;
use dbide_export::{ExportProgress, QueryRequest};
use dbide_result_store::{PageLimits, PagedResult, StoredPage, query_pages};
use gpui_component::input::{Textarea, TextareaState};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, TryRecvError, sync_channel},
};

pub(super) enum PagedQueryEvent {
    Close,
}

struct QueryJob {
    cancel: Arc<AtomicBool>,
    progress: Arc<ExportProgress>,
    terminal: Receiver<Result<PagedResult, String>>,
}

struct PageOutcome {
    nonce: u64,
    store: Arc<PagedResult>,
    page: Result<StoredPage, String>,
}

struct PageJob {
    nonce: u64,
    index: usize,
    store: Arc<PagedResult>,
    cancel: Arc<AtomicBool>,
    terminal: Receiver<PageOutcome>,
}

/// Owns one captured query and its private result file, independent of editor tabs.
pub(super) struct PagedQueryPanel {
    request: QueryRequest,
    document_title: String,
    preview: Entity<TextareaState>,
    preview_clipped: bool,
    show_sql: bool,
    locale: Locale,
    started: bool,
    query_job: Option<QueryJob>,
    page_job: Option<PageJob>,
    store: Option<Arc<PagedResult>>,
    current_page: Option<usize>,
    page_nonce: u64,
    grid: Entity<TableState<PagedGrid>>,
    status: String,
    status_key: StatusKey,
    _grid_subscription: Subscription,
}

#[derive(Clone, Copy)]
enum StatusKey {
    Ready,
    Running,
    Cancelling,
    Reading,
    Complete,
    Cancelled,
    PageCancelled,
    Error,
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
        // These are product limits, not editable export preferences.
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
            TableState::new(PagedGrid::empty(), window, cx)
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
            locale,
            started: false,
            query_job: None,
            page_job: None,
            store: None,
            current_page: None,
            page_nonce: 0,
            grid,
            status: String::new(),
            status_key: StatusKey::Ready,
            _grid_subscription: subscription,
        }
    }

    pub(super) fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.locale = locale;
        cx.notify();
    }

    pub(super) fn busy(&self) -> bool {
        self.query_job.is_some() || self.page_job.is_some()
    }

    pub(super) fn request_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = &self.query_job {
            job.cancel.store(true, Ordering::Release);
            self.status_key = StatusKey::Cancelling;
        }
        if let Some(job) = &self.page_job {
            job.cancel.store(true, Ordering::Release);
            self.status_key = StatusKey::Cancelling;
        }
        cx.notify();
    }

    fn tr(&self, ko: &'static str, en: &'static str) -> &'static str {
        if self.locale == Locale::Ko { ko } else { en }
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        if self.started || self.busy() || self.preview_clipped {
            return;
        }
        self.started = true;
        let request = self.request.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(ExportProgress::default());
        let worker_cancel = cancel.clone();
        let worker_progress = progress.clone();
        // Rows stay in the private spool. The channel only carries one terminal result.
        let (sender, terminal) = sync_channel(1);
        match std::thread::Builder::new()
            .name("paged-query-spool".into())
            .spawn(move || {
                let result = query_pages(
                    request,
                    PageLimits::default(),
                    worker_cancel,
                    worker_progress,
                )
                .map_err(|error| error.to_string());
                let _ = sender.send(result);
            }) {
            Ok(_) => {
                self.query_job = Some(QueryJob {
                    cancel,
                    progress,
                    terminal,
                });
                self.status_key = StatusKey::Running;
                self.poll_until_idle(cx);
            }
            Err(error) => self.fail(error.to_string()),
        }
        cx.notify();
    }

    fn poll_until_idle(&self, cx: &mut Context<Self>) {
        // Context::spawn supplies a weak entity, so an idle/dropped panel is not retained.
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let active = view
                    .update(cx, |this, cx| {
                        this.poll(cx);
                        this.busy()
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        })
        .detach();
    }

    fn fail(&mut self, error: String) {
        self.status = error;
        self.status_key = StatusKey::Error;
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = &self.query_job {
            let cancelled = job.cancel.load(Ordering::Acquire);
            match job.terminal.try_recv() {
                Ok(result) => {
                    self.query_job = None;
                    match result {
                        Ok(_) if cancelled => self.status_key = StatusKey::Cancelled,
                        Ok(result) => {
                            let store = Arc::new(result);
                            let empty = store.page_count() == 0;
                            self.store = Some(store);
                            self.show_sql = false;
                            if empty {
                                self.status_key = StatusKey::Complete;
                            } else {
                                self.begin_page(0, cx);
                            }
                        }
                        Err(_) if cancelled => self.status_key = StatusKey::Cancelled,
                        Err(error) => self.fail(error),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.query_job = None;
                    self.fail(self.tr("조회 작업 결과 확인 불가. 새 패널에서 명시적으로 실행하세요.", "Query worker outcome unknown. Open a new panel to explicitly run again.").into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(job) = &self.page_job {
            let index = job.index;
            let cancelled = job.cancel.load(Ordering::Acquire);
            match job.terminal.try_recv() {
                Ok(outcome) => {
                    let valid = outcome.nonce == self.page_nonce
                        && outcome.nonce == job.nonce
                        && Arc::ptr_eq(&outcome.store, &job.store)
                        && self
                            .store
                            .as_ref()
                            .is_some_and(|store| Arc::ptr_eq(store, &outcome.store));
                    self.page_job = None;
                    if cancelled {
                        self.status_key = StatusKey::PageCancelled;
                    } else if valid {
                        match outcome.page {
                            Ok(page) => {
                                let has_rows = !page.rows.is_empty();
                                self.grid.update(cx, |grid, cx| {
                                    *grid.delegate_mut() = PagedGrid::new(page);
                                    grid.clear_selection(cx);
                                    grid.refresh(cx);
                                    if has_rows {
                                        grid.scroll_to_row(0, cx);
                                    }
                                });
                                self.current_page = Some(index);
                                self.status_key = StatusKey::Complete;
                            }
                            Err(error) => self.fail(error),
                        }
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.page_job = None;
                    self.fail(
                        self.tr(
                            "페이지 읽기 작업 종료됨. 표시 중인 페이지는 보존됩니다.",
                            "Page reader stopped. The displayed page is preserved.",
                        )
                        .into(),
                    );
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        cx.notify();
    }

    fn begin_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        if index >= store.page_count() {
            return;
        }
        self.page_nonce = self.page_nonce.wrapping_add(1);
        let nonce = self.page_nonce;
        let worker_store = store.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, terminal) = sync_channel(1);
        match std::thread::Builder::new()
            .name("paged-query-read".into())
            .spawn(move || {
                let page = worker_store
                    .read_page(index)
                    .map_err(|error| error.to_string());
                let _ = sender.send(PageOutcome {
                    nonce,
                    store: worker_store,
                    page,
                });
            }) {
            Ok(_) => {
                self.page_job = Some(PageJob {
                    nonce,
                    index,
                    store,
                    cancel,
                    terminal,
                });
                self.status_key = StatusKey::Reading;
            }
            Err(error) => self.fail(error.to_string()),
        }
        cx.notify();
    }

    fn navigate(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.begin_page(index, cx);
        if self.page_job.is_some() {
            self.poll_until_idle(cx);
        }
    }

    fn copy_selected(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let text = self
            .grid
            .read(cx)
            .delegate()
            .selected_value()
            .map(|value| value.as_deref().unwrap_or("\\N").to_owned());
        if let Some(text) = text {
            // Each raw cell is bounded by the 512 KiB page budget.
            if text.len() <= 512 * 1024 {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    fn status_text(&self) -> String {
        match self.status_key {
            StatusKey::Ready => self.tr("캡처된 SQL 확인 후 조회 실행을 누르세요. 새 독립 실행입니다.", "Review captured SQL, then press Run query. This is a new independent execution.").into(),
            StatusKey::Running => self.tr("독립 읽기 전용 조회 · 결과 파일 저장 중…", "Independent read-only query · storing result file…").into(),
            StatusKey::Cancelling => self.tr("취소 요청됨 · 작업 종료 확인 중…", "Cancellation requested · waiting for the worker to finish…").into(),
            StatusKey::Reading => self.tr("저장된 페이지 읽는 중…", "Reading stored page…").into(),
            StatusKey::Complete => self.tr("조회 완료 · 페이지 이동은 저장된 파일만 읽습니다.", "Query complete · page navigation reads only the stored file.").into(),
            StatusKey::Cancelled => self.tr("조회 취소됨. 다시 실행하려면 패널을 새로 여세요.", "Query cancelled. Open a new panel to run again.").into(),
            StatusKey::PageCancelled => self.tr("페이지 읽기 취소됨 · 표시 중인 페이지 보존.", "Page read cancelled · displayed page preserved.").into(),
            StatusKey::Error => format!("{}: {}", self.tr("작업 중단", "Operation stopped"), self.status),
        }
    }
}

impl Drop for PagedQueryPanel {
    fn drop(&mut self) {
        if let Some(job) = &self.query_job {
            job.cancel.store(true, Ordering::Release);
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
}

impl PagedGrid {
    fn empty() -> Self {
        Self {
            page: None,
            columns: Vec::new(),
            selected: None,
        }
    }
    fn new(page: StoredPage) -> Self {
        let names = page
            .columns
            .iter()
            .map(|name| dbide_ui_spike::grid_presentation::bounded_label(name, 128))
            .collect();
        Self {
            page: Some(page),
            columns: dbide_ui_spike::result_grid_columns(names),
            selected: None,
        }
    }
    fn selected_value(&self) -> Option<&Option<String>> {
        let (row, col) = self.selected?;
        self.page.as_ref()?.rows.get(row)?.get(col)
    }
}

impl TableDelegate for PagedGrid {
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
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
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
    }
}

impl Render for PagedQueryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.busy();
        let profile = &self.request.connection.profile;
        let target = profile
            .sqlite_path
            .clone()
            .unwrap_or_else(|| format!("{}:{}/{}", profile.host, profile.port, profile.database));
        let page_count = self.store.as_ref().map_or(0, |store| store.page_count());
        let has_page = self.current_page.is_some();
        let previous = self.current_page.unwrap_or(0).saturating_sub(1);
        let next = self.current_page.map_or(0, |index| index + 1);
        let can_previous = !busy && self.current_page.is_some_and(|index| index > 0);
        let can_next = !busy && next < page_count;
        let selected = self.grid.read(cx).delegate().selected_value().is_some();
        let page_description = if let Some(store) = &self.store {
            let grid = self.grid.read(cx);
            let range = grid
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
            format!(
                "{} {}/{} · {} {range} / {} · {} bytes",
                self.tr("페이지", "Page"),
                self.current_page.map_or(0, |index| index + 1),
                page_count,
                self.tr("행", "Rows"),
                store.summary.rows,
                store.summary.payload_bytes
            )
        } else {
            self.tr("결과 없음 · 실행 전", "No result · not executed")
                .into()
        };
        let status = self.status_text();
        let identity = self.store.as_ref().map(|store| {
            format!(
                "{} {} · SQL SHA-256 {} · {} {}",
                self.tr("실행", "Execution"),
                store.summary.execution_id,
                store.summary.sql_sha256,
                self.tr("DB", "DB"),
                store.summary.execution_target.database
            )
        });
        div().id("paged-query-panel").flex().flex_col().size_full().min_w_0().min_h_0()
            .p_3().gap_2().bg(rgb(PANEL)).text_color(rgb(TEXT)).text_size(px(12.))
            .on_action(cx.listener(|this, _: &CopyCell, window, cx| {
                if this.grid.read(cx).focus_handle(cx).contains_focused(window, cx) {
                    this.copy_selected(cx);
                    cx.stop_propagation();
                }
            }))
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(self.tr("저장 결과 페이지 조회", "Stored result pages")))
                .child(Button::new("paged-query-close").ghost().small().label(self.tr("닫기", "Close"))
                    .disabled(busy).on_click(cx.listener(|this, _, _, cx| { if !this.busy() { cx.emit(PagedQueryEvent::Close); } }))))
            .child(div().text_ellipsis().child(format!("{} · {} · {}", profile.name, target, self.document_title)))
            .child(div().text_color(rgb(MUTED)).child(self.tr("열 때 고정한 SQL · 별도 읽기 전용 연결의 새 조회. 임시 테이블·미커밋 변경은 공유하지 않습니다.", "SQL captured when opened · a new query on a separate read-only connection. Temporary tables and uncommitted changes are not shared.")))
            .child(div().text_color(rgb(MUTED)).child(self.tr("조회 완료 후 저장된 결과를 페이지별로 읽습니다. 페이지 이동은 SQL 재실행하지 않음", "After the query completes, stored results are read page by page. Page navigation does not rerun SQL.")))
            .child(div().text_color(rgb(MUTED)).text_size(px(11.)).child(self.tr("최대 1,000,000행 · 64 MiB · 페이지당 200행 / 512 KiB · 30초", "Limits: 1,000,000 rows · 64 MiB · 200 rows / 512 KiB per page · 30 seconds")))
            .child(div().flex().gap_2()
                .child(Button::new("paged-query-run").small().label(self.tr("조회 실행", "Run query"))
                    .disabled(self.started || busy || self.preview_clipped).on_click(cx.listener(|this, _, _, cx| this.run(cx))))
                .child(Button::new("paged-query-cancel").ghost().small().label(self.tr("취소", "Cancel"))
                    .disabled(!busy || matches!(self.status_key, StatusKey::Cancelling)).on_click(cx.listener(|this, _, _, cx| this.request_cancel(cx))))
                .child(Button::new("paged-query-toggle-sql").ghost().small().label(if self.show_sql { self.tr("SQL 접기", "Hide SQL") } else { self.tr("캡처된 SQL 보기", "Show captured SQL") })
                    .on_click(cx.listener(|this, _, _, cx| { this.show_sql = !this.show_sql; cx.notify(); }))))
            .when(self.preview_clipped, |panel| panel.child(div().text_color(rgb(0xf2bd6b)).child(self.tr("SQL이 64 KiB 실행 한도를 초과합니다. 편집기에서 줄인 뒤 다시 여세요.", "SQL exceeds the 64 KiB execution limit. Shorten it in the editor and reopen."))))
            .when(self.show_sql, |panel| panel.child(Textarea::new(&self.preview).readonly(true).h(px(110.)).aria_label(self.tr("캡처된 조회 SQL · 읽기 전용", "Captured query SQL · read only"))))
            .when_some(identity, |panel, identity| panel.child(div().text_color(rgb(MUTED)).text_size(px(11.)).text_ellipsis().child(identity)))
            .when_some(self.query_job.as_ref(), |panel, job| {
                let progress = job.progress.snapshot();
                panel.child(div().text_color(rgb(MUTED)).child(format!("{} {} · {} bytes", progress.rows, self.tr("행 처리", "rows processed"), progress.payload_bytes)))
            })
            .child(div().id("paged-query-status").role(Role::Label).aria_label(status.clone()).child(status))
            .child(div().flex().items_center().gap_2()
                .child(Button::new("paged-query-previous").small().label(self.tr("이전 페이지", "Previous page")).disabled(!can_previous)
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(previous, cx))))
                .child(Button::new("paged-query-next").small().label(self.tr("다음 페이지", "Next page")).disabled(!can_next)
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(next, cx))))
                .child(div().flex_1().child(page_description))
                .child(Button::new("paged-query-copy-cell").ghost().small().label(self.tr("선택 셀 원문 복사 · NULL=\\N", "Copy raw cell · NULL=\\N")).disabled(busy || !selected)
                    .on_click(cx.listener(|this, _, _, cx| this.copy_selected(cx)))))
            .child(div().key_context("PagedResultGrid").flex_1().min_h_0().min_w_0().when(has_page, |panel| panel.child(DataTable::new(&self.grid).small().stripe(false).bordered(false))))
    }
}
