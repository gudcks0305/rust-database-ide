//! Native language preferences. The host owns persistence and actual locale changes.
use dbide_i18n::{Locale, Message, tr};
use gpui::{prelude::*, *};
use gpui_component::{
    Sizable,
    button::{Button, ButtonVariants},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferencesEvent {
    LocaleChosen(Locale),
}
impl EventEmitter<PreferencesEvent> for PreferencesPanel {}

pub struct PreferencesPanel {
    locale: Locale,
}
impl PreferencesPanel {
    /// Parent initializes GPUI/component. No settings reads, writes, or network calls.
    pub fn new(locale: Locale, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { locale }
    }
    /// Call after the host accepts/persists a language selection.
    pub fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.locale = locale;
        cx.notify();
    }
    pub fn locale(&self) -> Locale {
        self.locale
    }
    fn choose(&mut self, locale: Locale, cx: &mut Context<Self>) {
        cx.emit(PreferencesEvent::LocaleChosen(locale));
    }
}
impl Render for PreferencesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.locale;
        div()
            .id("preferences-panel")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .gap_3()
            .p_4()
            .bg(rgb(0x171c24))
            .text_color(rgb(0xdce4ef))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(18.))
                    .child(tr(locale, Message::Preferences)),
            )
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(tr(locale, Message::Language)),
            )
            .child(
                div().flex().gap_2().flex_wrap().children(
                    Locale::ALL
                        .iter()
                        .copied()
                        .enumerate()
                        .map(|(index, choice)| {
                            Button::new(("preferences-language", index))
                                .label(match choice {
                                    Locale::Ko => tr(locale, Message::Korean),
                                    Locale::En => tr(locale, Message::English),
                                })
                                .small()
                                .when(choice == locale, |button| button.primary())
                                .on_click(
                                    cx.listener(move |panel, _, _, cx| panel.choose(choice, cx)),
                                )
                        }),
                ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(0x9ba8bd))
                    .child(tr(locale, Message::LanguageExplanation)),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(0x9ba8bd))
                    .child(tr(locale, Message::OriginalDataPreserved)),
            )
    }
}
