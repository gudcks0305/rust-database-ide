use super::*;

impl Workbench {
    pub(super) fn open_result_find(
        &mut self,
        _: &FindResults,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sql_view_active() || self.logical_mode {
            return;
        }
        let Some(page) = self
            .pages
            .get(self.selected_page)
            .filter(|p| !p.command_only && !p.rows.is_empty())
            .cloned()
        else {
            return;
        };
        self.results_collapsed = false;
        self.dismiss_completion(cx);
        let panel = cx.new(|cx| result_find::ResultFind::new(page, self.locale, window, cx));
        self.result_find_subscription =
            Some(
                cx.subscribe_in(&panel, window, |s, source, event, window, cx| {
                    if s.result_find
                        .as_ref()
                        .is_none_or(|p| p.entity_id() != source.entity_id())
                    {
                        return;
                    }
                    match event {
                        result_find::ResultFindEvent::Close => {
                            s.result_find = None;
                            s.result_find_subscription = None;
                            s.focus_results(&FocusResults, window, cx);
                        }
                        result_find::ResultFindEvent::Selected { page, row, column } => {
                            if !s.sql_view_active() || s.logical_mode {
                                return;
                            }
                            let GridSource::Postgres(current) = &s.grid.read(cx).delegate().source
                            else {
                                return;
                            };
                            if !Rc::ptr_eq(current, page)
                                || *row >= page.rows.len()
                                || *column >= page.columns.len()
                            {
                                return;
                            }
                            s.grid
                                .update(cx, |grid, cx| grid.set_selected_cell(*row, *column, cx));
                        }
                    }
                    cx.notify();
                }),
            );
        self.result_find = Some(panel);
        cx.notify();
    }
}
