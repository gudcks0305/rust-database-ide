use super::*;
use dbide_ui_spike::value_selection::{
    InspectionSnapshot, SelectionLimits, ValueSnapshot, pretty_json, value_preview,
};
use gpui_component::input::{Textarea, TextareaState};
use gpui_component::{Selectable, tooltip::Tooltip};

pub(super) enum ValueInspectorEvent {
    Close,
}

/// Captured result values are independent of result refresh, paging and DB sessions.
pub(super) struct ValueInspector {
    snapshot: InspectionSnapshot,
    selected: usize,
    input: Entity<TextareaState>,
    locale: Locale,
    formatted: bool,
    clipped: bool,
    error: Option<String>,
}
impl EventEmitter<ValueInspectorEvent> for ValueInspector {}
fn caption(raw: &str) -> String {
    let mut text: String = raw.chars().take(120).collect();
    if text.len() < raw.len() {
        text.push('…');
    }
    text
}
impl ValueInspector {
    pub(super) fn new(
        snapshot: InspectionSnapshot,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (preview, clipped) =
            value_preview(snapshot.values().first().and_then(|v| v.raw.as_deref()));
        let input = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx);
            input.set_value(preview, window, cx);
            input
        });
        Self {
            snapshot,
            selected: 0,
            input,
            locale,
            formatted: false,
            clipped,
            error: None,
        }
    }
    pub(super) fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.locale = locale;
        cx.notify();
    }
    fn label(&self, ko: &'static str, en: &'static str) -> &'static str {
        if self.locale == Locale::Ko { ko } else { en }
    }
    fn value(&self) -> Option<&ValueSnapshot> {
        self.snapshot.values().get(self.selected)
    }
    fn show_value(&mut self, formatted: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(value) = self.value() else {
            return;
        };
        let result = if formatted {
            value
                .raw
                .as_deref()
                .ok_or_else(|| {
                    self.label("SQL NULL은 JSON이 아님", "SQL NULL is not JSON")
                        .to_owned()
                })
                .and_then(|raw| {
                    pretty_json(raw, SelectionLimits::default()).map_err(|e| e.to_string())
                })
        } else {
            Ok(value.raw.as_deref().unwrap_or("NULL").to_owned())
        };
        match result {
            Ok(text) => {
                let (preview, clipped) = value_preview(Some(&text));
                self.clipped = clipped;
                self.formatted = formatted;
                self.error = None;
                self.input
                    .update(cx, |input, cx| input.set_value(preview, window, cx));
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
}
impl Render for ValueInspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let source = self.value().map_or_else(String::new, |v| {
            format!(
                "{} · {} {}",
                v.source_label,
                self.label("행", "Row"),
                v.row_index + 1
            )
        });
        let value_label = self
            .value()
            .map_or_else(String::new, |v| match v.raw.as_deref() {
                None => self.label("SQL NULL", "SQL NULL").to_owned(),
                Some("") => self
                    .label("빈 문자열 · 0 bytes", "Empty string · 0 bytes")
                    .to_owned(),
                Some(raw) => format!("{} bytes", raw.len()),
            });
        let column = self.value().map(|v| v.column.clone()).unwrap_or_default();
        let source_tooltip = source.clone();
        let column_tooltip = column.clone();
        let json_hint = self.label(
            "숫자와 문자열 원문을 보존하여 JSON 정렬",
            "Format JSON preserving original numbers and strings",
        );
        let copy_hint = self.label(
            "전체 원문 복사 · SQL NULL: \\N · 빈 문자열: 빈 값",
            "Copy full original value · SQL NULL: \\N · empty string: empty value",
        );
        let mut panel = div()
            .id("value-inspector")
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
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.label("값 보기", "Value inspector")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_color(rgb(MUTED))
                            .text_size(px(11.))
                            .child(self.label("읽기 전용", "Read only")),
                    )
                    .child(
                        Button::new("close-value-inspector")
                            .ghost()
                            .small()
                            .label(self.label("닫기", "Close"))
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(ValueInspectorEvent::Close)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("inspector-source")
                    .min_w_0()
                    .text_ellipsis()
                    .text_color(rgb(MUTED))
                    .text_size(px(11.))
                    .aria_label(source.clone())
                    .tooltip(move |_, cx| cx.new(|_| Tooltip::new(source_tooltip.clone())).into())
                    .child(source),
            );
        if matches!(self.snapshot, InspectionSnapshot::Row(_)) {
            // Capture rejects more than 256 fields; long values are only rendered on selection.
            let mut fields = div()
                .id("inspector-row-fields")
                .max_h(px(180.))
                .overflow_y_scroll()
                .flex()
                .flex_col();
            for (index, value) in self.snapshot.values().iter().enumerate() {
                let summary = match value.raw.as_deref() {
                    None => "NULL".to_owned(),
                    Some("") => "\"\"".to_owned(),
                    Some(raw) => caption(raw).replace(['\n', '\r', '\t'], " "),
                };
                fields = fields.child(
                    Button::new(("inspect-row-field", index))
                        .ghost()
                        .small()
                        .selected(index == self.selected)
                        .label(format!(
                            "{} · {} = {}",
                            index + 1,
                            caption(&value.column),
                            summary
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.selected = index;
                            this.show_value(false, window, cx);
                        })),
                );
            }
            panel = panel
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child(self.label("행 필드", "Row fields")),
                )
                .child(fields);
        }
        panel =
            panel
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("inspector-column")
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .font_weight(FontWeight::SEMIBOLD)
                                .aria_label(column.clone())
                                .tooltip(move |_, cx| {
                                    cx.new(|_| Tooltip::new(column_tooltip.clone())).into()
                                })
                                .child(column),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(rgb(MUTED))
                                .text_size(px(11.))
                                .child(value_label),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("inspect-raw")
                                .ghost()
                                .small()
                                .selected(!self.formatted)
                                .label(self.label("원문", "Raw"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.show_value(false, window, cx)
                                })),
                        )
                        .child(
                            Button::new("inspect-json")
                                .ghost()
                                .small()
                                .selected(self.formatted)
                                .label("JSON")
                                .tooltip(json_hint)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.show_value(true, window, cx)
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("inspect-copy-raw")
                                .ghost()
                                .small()
                                .label(self.label("원문 복사", "Copy raw"))
                                .tooltip(copy_hint)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(value) = this.value() {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            value.raw_copy().to_owned(),
                                        ));
                                    }
                                })),
                        ),
                )
                .child(
                    div()
                        .id("inspector-value-details")
                        .text_color(rgb(MUTED))
                        .text_size(px(11.))
                        .child(self.label(
                            "텍스트 스냅샷 · DB 자료형 미제공",
                            "Text snapshot · DB type unavailable",
                        )),
                );
        if self.clipped {
            panel = panel.child(div().text_color(rgb(0xf2bd6b)).child(self.label(
                "앞 64 KiB만 표시 · 원문 복사는 전체 값 유지",
                "Showing first 64 KiB · Copy raw retains the full value",
            )));
        }
        if let Some(error) = &self.error {
            panel = panel.child(div().text_color(rgb(0xf2bd6b)).child(format!(
                "{}: {}",
                self.label("JSON 표시 불가", "Cannot format JSON"),
                error
            )));
        }
        panel.child(
            div().flex_1().min_h_0().child(
                Textarea::new(&self.input)
                    .readonly(true)
                    .h_full()
                    .aria_label(self.label("읽기 전용 값", "Read-only value")),
            ),
        )
    }
}
