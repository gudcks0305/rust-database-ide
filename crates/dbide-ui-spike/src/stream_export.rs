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
        self.status = if self.locale == Locale::Ko {
            "닫는 중 · CSV 작업을 정리하고 있습니다."
        } else {
            "Closing · finishing CSV cleanup."
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
        if self.close_after_streams || self.recovery.close_after || self.close_prompt_open {
            return;
        }
        if let Err(error) = self.prepare_console_password(cx) {
            self.status = error;
            cx.notify();
            return;
        }
        let Some(connection) = self.console().request() else {
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
                connection,
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
