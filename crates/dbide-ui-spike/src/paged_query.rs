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
        // Closing also suppresses tail-refresh work. Repeatedly cancelling page
        // reads while still scheduling new refreshes could prevent settlement.
        panel.update(cx, |p, cx| p.request_close(cx));
        self.paged_query_visible = true;
        self.recovery.close_after = false;
        self.status = if self.locale == Locale::Ko {
            "닫는 중 · 조회 연결을 정리하고 있습니다."
        } else {
            "Closing · retiring the query connection."
        }
        .into();
        cx.notify();
        true
    }
    pub(super) fn close_when_streams_settle(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.close_after_streams {
            return;
        }
        self.close_after_streams = true;
        cx.spawn_in(window, async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let pending = view
                    .update_in(cx, |s, w, cx| {
                        let csv_busy = s.defer_close_for_stream_export(cx);
                        let page_busy = s.defer_close_for_paged_query(cx);
                        if csv_busy || page_busy {
                            return true;
                        }
                        // A completed query must not lose the close intent merely
                        // because an unrelated local write is still settling.
                        if s.profile_write_busy
                            || !s.profile_writes.is_empty()
                            || s.recovery.loading
                            || s.recovery.preference_busy
                            || s.table_editor
                                .as_ref()
                                .is_some_and(|editor| editor.read(cx).is_busy())
                        {
                            return true;
                        }
                        s.close_after_streams = false;
                        if s.request_window_close(w, cx) {
                            s.finish_window_close(w, cx);
                        }
                        false
                    })
                    .unwrap_or(false);
                if !pending {
                    break;
                }
            }
        })
        .detach();
    }
}
