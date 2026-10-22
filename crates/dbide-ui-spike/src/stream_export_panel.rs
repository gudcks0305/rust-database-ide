use super::*;
use dbide_export::{
    ExportOptions, ExportProgress, ExportRequest, ExportSummary, export_csv_with_password,
};
use gpui_component::{
    input::{Textarea, TextareaState},
    tooltip::Tooltip,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, TryRecvError, sync_channel},
    },
};

pub(super) enum StreamExportEvent {
    Close,
}

enum ExportStatus {
    Ready,
    Running,
    Cancelling,
    Complete { rows: u64, bytes: u64 },
    Failed(String),
    StartFailed(String),
    Unknown,
}

struct ExportJob {
    cancel: Arc<AtomicBool>,
    progress: Arc<ExportProgress>,
    terminal: Receiver<Result<ExportSummary, dbide_export::ExportError>>,
}

/// The target and SQL belong to this panel, independent of the active editor/session.
pub(super) struct StreamExportPanel {
    profile: ConnectionProfile,
    password: String,
    sql: String,
    document_title: String,
    preview: Entity<TextareaState>,
    preview_clipped: bool,
    locale: Locale,
    output: Option<PathBuf>,
    picker_busy: bool,
    job: Option<ExportJob>,
    status: ExportStatus,
}

impl EventEmitter<StreamExportEvent> for StreamExportPanel {}

impl StreamExportPanel {
    pub(super) fn new(
        connection: ConnectionRequest,
        sql: String,
        document_title: String,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (preview_text, preview_clipped) =
            dbide_ui_spike::value_selection::value_preview(Some(&sql));
        let preview = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx);
            input.set_value(preview_text, window, cx);
            input
        });
        Self {
            profile: connection.profile,
            password: connection.password,
            sql,
            document_title,
            preview,
            preview_clipped,
            locale,
            output: None,
            picker_busy: false,
            job: None,
            status: ExportStatus::Ready,
        }
    }

    pub(super) fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.locale = locale;
        cx.notify();
    }

    pub(super) fn busy(&self) -> bool {
        self.picker_busy || self.job.is_some()
    }

    pub(super) fn request_cancel(&mut self, cx: &mut Context<Self>) {
        self.cancel(cx);
    }

    fn tr(&self, ko: &'static str, en: &'static str) -> &'static str {
        if self.locale == Locale::Ko { ko } else { en }
    }

    fn can_run(&self) -> bool {
        self.job.is_none()
            && !self.preview_clipped
            && !self.picker_busy
            && self.output.is_some()
            && !matches!(
                self.status,
                ExportStatus::Unknown | ExportStatus::Complete { .. }
            )
    }

    fn choose_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.job.is_some() || self.picker_busy {
            return;
        }
        self.picker_busy = true;
        let receiver = cx.prompt_for_new_path(std::path::Path::new("."), Some("query-export.csv"));
        cx.spawn_in(window, async move |view, cx| {
            let picked = receiver.await;
            let _ = view.update(cx, |this, cx| {
                this.picker_busy = false;
                match picked {
                    Ok(Ok(Some(path))) if this.job.is_none() => {
                        this.output = Some(path);
                        this.status = ExportStatus::Ready;
                    }
                    Ok(Err(error)) => this.status = ExportStatus::StartFailed(error.to_string()),
                    Err(error) => this.status = ExportStatus::StartFailed(error.to_string()),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        if !self.can_run() {
            return;
        }
        let Some(output) = self.output.clone() else {
            return;
        };
        let request = ExportRequest {
            profile: self.profile.clone(),
            sql: self.sql.clone(),
            output,
            options: ExportOptions::default(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(ExportProgress::default());
        let worker_cancel = cancel.clone();
        let worker_progress = progress.clone();
        let password = self.password.clone();
        // Only a terminal result crosses the channel; progress remains constant-size atomics.
        let (sender, terminal) = sync_channel(1);
        match std::thread::Builder::new()
            .name("csv-stream-export".into())
            .spawn(move || {
                let _ = sender.send(export_csv_with_password(
                    request,
                    password,
                    worker_cancel,
                    worker_progress,
                ));
            }) {
            Ok(_) => {
                self.job = Some(ExportJob {
                    cancel,
                    progress,
                    terminal,
                });
                self.status = ExportStatus::Running;
                cx.spawn(async move |view, cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(100))
                            .await;
                        let active = view
                            .update(cx, |this, cx| {
                                this.poll(cx);
                                this.job.is_some()
                            })
                            .unwrap_or(false);
                        if !active {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(error) => self.status = ExportStatus::StartFailed(error.to_string()),
        }
        cx.notify();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let Some(job) = &self.job else {
            return;
        };
        match job.terminal.try_recv() {
            Ok(result) => {
                self.job = None;
                self.status = match result {
                    Ok(summary) => ExportStatus::Complete {
                        rows: summary.rows,
                        bytes: summary.encoded_bytes,
                    },
                    Err(error) => ExportStatus::Failed(error.to_string()),
                };
            }
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                self.status = ExportStatus::Unknown;
            }
            Err(TryRecvError::Empty) => {}
        }
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Release);
            self.status = ExportStatus::Cancelling;
            cx.notify();
        }
    }

    fn status_text(&self) -> String {
        match &self.status {
            ExportStatus::Ready if self.output.is_some() => self.tr("준비됨 · 캡처된 SQL을 새로 실행합니다.", "Ready · runs the captured SQL as a new execution.").into(),
            ExportStatus::Ready => self.tr("출력 파일 선택 후 내보내기를 실행하세요.", "Choose an output file, then run export.").into(),
            ExportStatus::Running => self.tr("독립 읽기 전용 내보내기 실행 중…", "Independent read-only export running…").into(),
            ExportStatus::Cancelling => self.tr("취소 요청됨 · 작업 종료 확인 중…", "Cancellation requested · waiting for terminal outcome…").into(),
            ExportStatus::Complete { rows, bytes } => format!("{} · {rows} {} · {bytes} bytes", self.tr("완료", "Complete"), self.tr("행", "rows")),
            ExportStatus::Failed(error) => format!("{}: {error}", self.tr("내보내기 중단", "Export stopped")),
            ExportStatus::StartFailed(error) => format!("{}: {error}", self.tr("시작 불가", "Could not start")),
            ExportStatus::Unknown => self.tr("작업 결과 확인 불가. 출력 경로를 확인하세요. 재실행하려면 새 파일을 선택하세요.", "Worker outcome unknown. Check the destination. Choose a new file before another run.").into(),
        }
    }

    fn detail(id: &'static str, text: String) -> Stateful<Div> {
        let tooltip = text.clone();
        div()
            .id(id)
            .role(Role::Label)
            .min_w_0()
            .text_ellipsis()
            .aria_label(text.clone())
            .tooltip(move |_, cx| cx.new(|_| Tooltip::new(tooltip.clone())).into())
            .child(text)
    }
}

impl Drop for StreamExportPanel {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Release);
        }
    }
}

impl Render for StreamExportPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.job.is_some();
        let picker_disabled = active || self.picker_busy;
        let source = self.document_title.clone();
        let target = self.profile.sqlite_path.clone().unwrap_or_else(|| {
            format!(
                "{}:{}/{}",
                self.profile.host, self.profile.port, self.profile.database
            )
        });
        let output = self
            .output
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| {
                self.tr("출력 파일 미선택", "No output file selected")
                    .into()
            });
        let status = self.status_text();
        let close_hint = self.tr(
            "실행 중에는 먼저 취소 후 종료를 기다리세요.",
            "Cancel an active export and wait for it to finish before closing.",
        );
        let explanation = self.tr(
            "열 때 고정한 편집기 SQL · 별도 읽기 전용 연결에서 새로 실행. 현재 결과·임시 테이블·미커밋 변경과 다를 수 있음",
            "Editor SQL captured when opened · runs on a separate read-only connection. Results may differ from the current result, temporary tables or uncommitted changes.",
        );
        let limits = self.tr(
            "최대 1,000,000행 · 64 MiB · 행당 1 MiB · 30초",
            "Limits: 1,000,000 rows · 64 MiB · 1 MiB/row · 30 seconds",
        );
        let sql_label = self.tr(
            "열 때 고정한 편집기 전체 SQL · 읽기 전용",
            "Full editor SQL captured when opened · read only",
        );
        let clipped_hint = self.tr(
                        "SQL이 실행 한도 64 KiB를 초과합니다. 편집기에서 줄인 뒤 패널을 다시 여세요.",
                        "SQL exceeds the 64 KiB execution limit. Shorten it in the editor and reopen this panel.",
        );
        let picker_label = if self.picker_busy {
            self.tr("파일 선택 중…", "Choosing file…")
        } else {
            self.tr("새 CSV 파일 선택…", "Choose new CSV file…")
        };
        div()
            .id("stream-export-panel")
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .p_3()
            .gap_2()
            .bg(rgb(PANEL))
            .text_color(rgb(TEXT))
            .text_size(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("SQL → CSV"),
                    )
                    .child(
                        Button::new("stream-export-close")
                            .ghost()
                            .small()
                            .label(self.tr("닫기", "Close"))
                            .disabled(picker_disabled)
                            .tooltip(close_hint)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.job.is_none() && !this.picker_busy {
                                    cx.emit(StreamExportEvent::Close);
                                }
                            })),
                    ),
            )
            .child(Self::detail(
                "stream-export-profile",
                self.profile.name.clone(),
            ))
            .child(
                Self::detail("stream-export-target", target)
                    .text_color(rgb(MUTED))
                    .text_size(px(11.)),
            )
            .child(
                Self::detail("stream-export-source", source)
                    .text_color(rgb(MUTED))
                    .text_size(px(11.)),
            )
            .child(div().text_color(rgb(MUTED)).child(explanation))
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .text_size(px(11.))
                    .child(limits),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(sql_label),
            )
            .when(self.preview_clipped, |panel| {
                panel.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(0xf2bd6b))
                        .child(clipped_hint),
                )
            })
            .child(
                div().flex_1().min_h_0().child(
                    Textarea::new(&self.preview)
                        .readonly(true)
                        .h_full()
                        .aria_label(self.tr("캡처된 내보내기 SQL", "Captured export SQL")),
                ),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(rgb(0x2a3644))
                    .pt_2()
                    .child(Self::detail("stream-export-output", output)),
            )
            .child(
                div().flex().child(
                    Button::new("stream-export-choose-output")
                        .small()
                        .label(picker_label)
                        .disabled(picker_disabled)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.choose_output(window, cx)),
                        ),
                ),
            )
            .when_some(self.job.as_ref(), |panel, job| {
                let progress = job.progress.snapshot();
                panel.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(format!(
                            "{} {} · {} bytes {}",
                            progress.rows,
                            self.tr("행 처리", "rows processed"),
                            progress.payload_bytes,
                            self.tr("페이로드", "payload")
                        )),
                )
            })
            .child(
                div()
                    .id("stream-export-status")
                    .role(Role::Label)
                    .aria_label(status.clone())
                    .child(status),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("stream-export-run")
                            .small()
                            .label(self.tr("내보내기 실행", "Run export"))
                            .disabled(!self.can_run())
                            .on_click(cx.listener(|this, _, _, cx| this.run(cx))),
                    )
                    .child(
                        Button::new("stream-export-cancel")
                            .ghost()
                            .small()
                            .label(self.tr("취소", "Cancel"))
                            .disabled(!active || matches!(self.status, ExportStatus::Cancelling))
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                    ),
            )
    }
}
