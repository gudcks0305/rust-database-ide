//! Completion transitions and a popup anchored in the editor's current prepaint.
use gpui::{prelude::*, *};
use gpui_component::input::EditorState;

/// SQL commands act only on the visible SQL view. A retained reader/editor
/// belongs to its tab and does not grant permission to dispatch into it.
pub(super) fn sql_view_active(table_visible: bool, tools_visible: bool) -> bool {
    !table_visible && !tools_visible
}

pub(super) fn next_selection(selected: usize, count: usize, down: bool) -> usize {
    if count == 0 {
        return 0;
    }
    let selected = selected.min(count - 1);
    if down {
        (selected + 1) % count
    } else {
        (selected + count - 1) % count
    }
}

pub(super) fn insert(source: &str, item: &dbide_sql::Completion) -> Option<(String, usize)> {
    source.get(item.replace.clone())?;
    let mut sql = source.to_owned();
    sql.replace_range(item.replace.clone(), &item.insert_text);
    Some((sql, item.replace.start + item.insert_text.len()))
}

pub(super) fn snapshot_matches(
    captured: (u64, u64, &str, usize, super::ConsoleRoute, u64),
    current: (u64, u64, &str, usize, super::ConsoleRoute, u64),
) -> bool {
    captured == current
}

/// Defer prepaint until Editor publishes this frame's caret/scroll geometry.
/// Both paint and hitboxes use that anchor; no previous-frame cursor estimate.
pub(super) struct CaretPopup {
    editor: WeakEntity<EditorState>,
    content: AnyElement,
    visible: bool,
}
impl CaretPopup {
    pub(super) fn new(editor: WeakEntity<EditorState>, content: impl IntoElement) -> Self {
        Self {
            editor,
            content: content.into_any_element(),
            visible: false,
        }
    }
}
impl Element for CaretPopup {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (window.request_layout(Style::default(), [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.visible = false;
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        let Some(geometry) = editor.read(cx).prepaint_caret_geometry() else {
            return;
        };
        let viewport = window.viewport_size();
        let size = self.content.layout_as_root(
            Size {
                width: AvailableSpace::Definite(viewport.width),
                height: AvailableSpace::Definite(viewport.height),
            },
            window,
            cx,
        );
        let caret = geometry.cursor_bounds();
        let below = caret.origin.y + geometry.scroll_offset().y + geometry.line_height() + px(4.);
        let above = caret.origin.y + geometry.scroll_offset().y - size.height - px(4.);
        let y = if below + size.height <= viewport.height {
            below
        } else {
            above.max(px(0.))
        };
        let x = (caret.origin.x - px(4.))
            .max(px(0.))
            .min((viewport.width - size.width).max(px(0.)));
        self.content.prepaint_at(point(x, y), window, cx);
        self.visible = true;
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.visible {
            self.content.paint(window, cx);
        }
    }
}
impl IntoElement for CaretPopup {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn sql_command_dispatch_requires_the_visible_sql_view() {
        assert!(sql_view_active(false, false));
        assert!(!sql_view_active(true, false));
        assert!(!sql_view_active(false, true));
        assert!(!sql_view_active(true, true));
    }
    #[::core::prelude::v1::test]
    fn selection_wraps_and_recovers_from_changed_candidate_count() {
        assert_eq!(next_selection(0, 3, true), 1);
        assert_eq!(next_selection(2, 3, true), 0);
        assert_eq!(next_selection(0, 3, false), 2);
        assert_eq!(next_selection(99, 2, true), 0);
        assert_eq!(next_selection(99, 0, false), 0);
    }
    #[::core::prelude::v1::test]
    fn inserts_only_replacement_and_preserves_multibyte_surroundings() {
        let source = "-- 한글\nSEL; SELECT '😊';";
        let start = source.find("SEL;").unwrap();
        let mut item = dbide_sql::Completion {
            label: "SELECT".into(),
            insert_text: "SELECT".into(),
            kind: dbide_sql::CompletionKind::Keyword,
            replace: start..start + 3,
        };
        assert_eq!(
            insert(source, &item),
            Some(("-- 한글\nSELECT; SELECT '😊';".into(), start + 6))
        );
        item.replace = 4..5; // Inside the UTF-8 Korean code point.
        assert!(insert(source, &item).is_none());
    }
    #[::core::prelude::v1::test]
    fn refuses_document_revision_source_cursor_target_or_catalog_changes() {
        let route = super::super::ConsoleRoute {
            console_id: 1,
            generation: 3,
        };
        let captured = (1, 2, "SEL", 3, route, 4);
        assert!(snapshot_matches(captured, captured));
        assert!(!snapshot_matches(captured, (2, 2, "SEL", 3, route, 4)));
        assert!(!snapshot_matches(captured, (1, 3, "SEL", 3, route, 4)));
        assert!(!snapshot_matches(captured, (1, 2, "SE", 3, route, 4)));
        assert!(!snapshot_matches(captured, (1, 2, "SEL", 2, route, 4)));
        assert!(!snapshot_matches(
            captured,
            (
                1,
                2,
                "SEL",
                3,
                super::super::ConsoleRoute {
                    generation: 4,
                    ..route
                },
                4
            )
        ));
        assert!(!snapshot_matches(captured, (1, 2, "SEL", 3, route, 5)));
    }
}
