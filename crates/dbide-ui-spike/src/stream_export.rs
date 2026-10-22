use super::*;

impl Workbench {
    /// Keep a graceful close from killing a file writer before its terminal
    /// outcome and temporary-file cleanup have been observed.
    pub(super) fn defer_close_for_stream_export(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(panel) = self
            .stream_export
            .as_ref()
            .filter(|panel| panel.read(cx).busy())
            .cloned()
        else {
            return false;
        };
        panel.update(cx, |panel, cx| panel.request_cancel(cx));
        self.stream_export_visible = true;
        self.recovery.close_after = false;
        self.quit_after_close = false;
        self.status = if self.locale == Locale::Ko {
            "CSV 작업 종료를 기다린 뒤 창을 다시 닫으세요. 파일 선택 중이면 선택창을 먼저 닫으세요."
        } else {
            "Wait for CSV export to stop, then close again. Dismiss any open file picker first."
        }
        .into();
        cx.notify();
        true
    }
    /// Export is an explicitly captured editor operation, not continuation of a
    /// displayed result or an action on whichever tab happens to be active later.
    pub(super) fn open_stream_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Keep the existing captured source and job. Closing its panel is the
        // explicit boundary before capturing another editor/target.
        if self.stream_export.is_some() {
            self.stream_export_visible = true;
            cx.notify();
            return;
        }
        if !self.sql_view_active() {
            return;
        }
        let Some(profile) = self
            .console()
            .target()
            .filter(|profile| {
                profile.engine == DatabaseEngine::Sqlite
                    && profile
                        .sqlite_path
                        .as_deref()
                        .is_some_and(|path| path != ":memory:")
            })
            .cloned()
        else {
            return;
        };
        let sql = self.editor.read(cx).value().to_string();
        let title = self
            .documents
            .iter()
            .find(|document| document.model.id == self.active_document)
            .map(|document| document.model.title.clone())
            .unwrap_or_default();
        self.dismiss_completion(cx);
        let panel = cx.new(|cx| {
            stream_export_panel::StreamExportPanel::new(
                profile,
                sql,
                title,
                self.locale,
                window,
                cx,
            )
        });
        self.stream_export_subscription =
            Some(cx.subscribe_in(&panel, window, |s, source, event, _, cx| {
                if s.stream_export
                    .as_ref()
                    .is_none_or(|current| current.entity_id() != source.entity_id())
                {
                    return;
                }
                match event {
                    stream_export_panel::StreamExportEvent::Close => {
                        s.stream_export = None;
                        s.stream_export_visible = false;
                        s.stream_export_subscription = None;
                    }
                }
                cx.notify();
            }));
        self.stream_export = Some(panel);
        self.stream_export_visible = true;
        cx.notify();
    }
}
