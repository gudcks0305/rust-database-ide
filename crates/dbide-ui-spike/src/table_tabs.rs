//! Retained table pages. Only `table_editor` owns the shared legacy reader.
use super::*;
use dbide_ui_spike::navigator::{Key, Relation};
use table_editor::TableEditor;

#[path = "table_tabs_model.rs"]
pub(super) mod model;

pub(super) struct TableTab {
    pub(super) id: u64,
    pub(super) key: Key,
    pub(super) request: ConnectionRequest,
    pub(super) relation: Relation,
    pub(super) editor: Entity<TableEditor>,
}
impl Workbench {
    pub(super) fn active_table_entity(&self) -> Option<Entity<TableEditor>> {
        self.table_tabs
            .iter()
            .find(|tab| Some(tab.id) == self.active_table_tab)
            .map(|tab| tab.editor.clone())
    }

    pub(super) fn table_reader_busy(&self, cx: &App) -> bool {
        self.busy()
            || self.active_operation.is_some()
            || self.pending_sql.is_some()
            || self.pending_table_open.is_some()
            || self
                .table_editor
                .as_ref()
                .is_some_and(|editor| editor.read(cx).is_busy())
    }

    pub(super) fn disable_table_read_controls(&mut self, cx: &mut Context<Self>) {
        for tab in &self.table_tabs {
            tab.editor
                .update(cx, |editor, cx| editor.set_read_controls_enabled(false, cx));
        }
    }

    pub(super) fn show_table_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.table_tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let (key, request, relation) = (tab.key.clone(), tab.request.clone(), tab.relation.clone());
        if !self.navigator.authorized(&key)
            || self
                .table_editor
                .as_ref()
                .is_none_or(|owner| owner.entity_id() != tab.editor.entity_id())
        {
            tab.editor
                .update(cx, |editor, cx| editor.set_read_controls_enabled(false, cx));
        }
        self.dismiss_completion(cx);
        self.active_table_tab = Some(id);
        self.tools = None;
        self.begin_table_open(key, request, relation, window, cx);
        cx.notify();
    }

    pub(super) fn close_table_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.table_tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let editor = tab.editor.clone();
        let owns_reader = self
            .table_editor
            .as_ref()
            .is_some_and(|owner| owner.entity_id() == editor.entity_id());
        if window.has_active_prompt()
            || editor.read(cx).is_busy()
            || (owns_reader && (self.table_reader_busy(cx) || self.pending_table_tab == Some(id)))
        {
            return;
        }
        if editor.read(cx).has_unsaved(cx) {
            let expected = editor.read(cx).edit_revision();
            let receiver = window.prompt(PromptLevel::Warning, "Discard local table edits?", Some("Closing discards local table buffers. It does not resolve a server transaction."), &["Keep editing", "Discard and close"], cx);
            cx.spawn_in(window, async move |view, cx| {
                if receiver.await == Ok(1) {
                    let _ = view.update_in(cx, |s, w, cx| {
                        if editor.read(cx).edit_revision() == expected && !editor.read(cx).is_busy()
                        {
                            s.remove_table_tab(id, w, cx);
                        }
                    });
                }
            })
            .detach();
        } else {
            self.remove_table_tab(id, window, cx);
        }
    }

    fn remove_table_tab(&mut self, id: u64, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.table_tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let editor = &self.table_tabs[index].editor;
        let owns_reader = self
            .table_editor
            .as_ref()
            .is_some_and(|owner| owner.entity_id() == editor.entity_id());
        if editor.read(cx).is_busy()
            || (owns_reader && self.table_reader_busy(cx))
            || self.pending_table_tab == Some(id)
        {
            return;
        }
        self.table_tabs.remove(index);
        if owns_reader {
            self.table_editor = None;
        }
        if self.active_table_tab == Some(id) {
            self.active_table_tab = None;
        }
        cx.notify();
    }

    pub(super) fn table_tab_entries(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut strip = vec![];
        for tab in &self.table_tabs {
            let id = tab.id;
            let active = self.active_table_tab == Some(id);
            let label = format!("{} · {}", tab.relation.name, tab.key.database);
            let target = format!(
                "{}.{} · {} · {}",
                tab.relation.schema,
                tab.relation.name,
                tab.request.profile.name,
                dbide_ui_spike::workbench_model::profile_target(&tab.request.profile)
            );
            strip.push(
                div()
                    .h(px(36.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .border_b_2()
                    .border_color(if active { rgb(ACCENT) } else { rgb(PANEL) })
                    .when(active, |el| el.bg(rgb(BG)))
                    .child(
                        Button::new(SharedString::from(format!("table-tab-{id}")))
                            .small()
                            .ghost()
                            .label(label)
                            .tooltip(target)
                            .on_click(cx.listener(move |s, _, w, cx| s.show_table_tab(id, w, cx))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("close-table-tab-{id}")))
                            .accessibility_label(format!(
                                "{} {} · {}",
                                tr(self.locale, Message::CloseTable),
                                tab.relation.name,
                                tab.key.database
                            ))
                            .small()
                            .ghost()
                            .label("×")
                            .disabled(
                                tab.editor.read(cx).is_busy() || self.pending_table_tab == Some(id),
                            )
                            .on_click(cx.listener(move |s, _, w, cx| s.close_table_tab(id, w, cx))),
                    )
                    .into_any_element(),
            );
        }
        strip
    }
}
