use super::*;

impl Workbench {
    pub(super) fn open_paged_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.paged_query.is_some() {
            self.paged_query_visible = true;
            self.stream_export_visible = false;
            cx.notify();
            return;
        }
        if !self.sql_view_active() {
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
            .find(|d| d.model.id == self.active_document)
            .map(|d| d.model.title.clone())
            .unwrap_or_default();
        self.dismiss_completion(cx);
        let request = dbide_export::QueryRequest {
            connection,
            sql,
            options: Default::default(),
        };
        let panel = cx.new(|cx| {
            paged_query_panel::PagedQueryPanel::new(request, title, self.locale, window, cx)
        });
        self.paged_query_subscription =
            Some(cx.subscribe_in(&panel, window, |s, source, event, _, cx| {
                if s.paged_query
                    .as_ref()
                    .is_none_or(|p| p.entity_id() != source.entity_id())
                {
                    return;
                }
                match event {
                    paged_query_panel::PagedQueryEvent::Close => {
                        s.paged_query = None;
                        s.paged_query_visible = false;
                        s.paged_query_subscription = None;
                    }
                }
                cx.notify();
            }));
        self.paged_query = Some(panel);
        self.paged_query_visible = true;
        self.stream_export_visible = false;
        cx.notify();
    }
    pub(super) fn defer_close_for_paged_query(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(panel) = self
            .paged_query
            .as_ref()
            .filter(|p| p.read(cx).busy())
            .cloned()
        else {
            return false;
        };
        panel.update(cx, |p, cx| p.request_cancel(cx));
        self.paged_query_visible = true;
        self.recovery.close_after = false;
        self.quit_after_close = false;
        self.status = if self.locale == Locale::Ko {
            "페이지 조회 종료를 기다린 뒤 창을 다시 닫으세요."
        } else {
            "Wait for the paged query to stop, then close again."
        }
        .into();
        cx.notify();
        true
    }
}
