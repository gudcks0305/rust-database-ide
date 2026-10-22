//! Native preferences. Host persists accepted changes and applies runtime behavior.
use dbide_i18n::{Locale, Message, tr};
use dbide_settings::AppSettings;
use gpui::{prelude::*, *};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferencesEvent {
    LocaleChosen(Locale),
    FontSizeChosen(u16),
    TabWidthChosen(u8),
    PageSizeChosen(u16),
    RecoveryChosen(bool),
    TransferRequested,
}
impl EventEmitter<PreferencesEvent> for PreferencesPanel {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Section {
    #[default]
    Editor,
    Results,
    Recovery,
    Language,
    Keyboard,
    Transfer,
}
impl Section {
    const ALL: [Self; 6] = [
        Self::Editor,
        Self::Results,
        Self::Recovery,
        Self::Language,
        Self::Keyboard,
        Self::Transfer,
    ];
    fn title(self, locale: Locale) -> &'static str {
        match self {
            Self::Editor => text(locale, "화면·편집기", "Appearance / Editor"),
            Self::Results => text(locale, "쿼리 결과", "Results"),
            Self::Recovery => text(locale, "SQL 복구", "SQL recovery"),
            Self::Language => tr(locale, Message::Language),
            Self::Keyboard => text(locale, "키보드 단축키", "Keyboard shortcuts"),
            Self::Transfer => text(locale, "설정 이동", "Settings transfer"),
        }
    }
}
fn text(locale: Locale, ko: &'static str, en: &'static str) -> &'static str {
    if locale == Locale::Ko { ko } else { en }
}
fn note(value: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(rgb(0x9aa9bd))
        .child(value.into())
}
fn group(title: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(rgb(0x303d4f))
        .bg(rgb(0x1a2431))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
}

pub struct PreferencesPanel {
    locale: Locale,
    settings: AppSettings,
    section: Section,
    recovery_enabled: bool,
    recovery_available: bool,
}
impl PreferencesPanel {
    /// Pure construction. Parent initializes GPUI/component and supplies settings.
    pub fn new(locale: Locale, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            locale,
            settings: AppSettings {
                locale,
                ..AppSettings::default()
            },
            section: Section::default(),
            recovery_enabled: false,
            recovery_available: false,
        }
    }
    /// Call only after the host accepts and persists the change.
    pub fn set_settings(&mut self, settings: AppSettings, cx: &mut Context<Self>) {
        self.locale = settings.locale;
        self.settings = settings;
        cx.notify();
    }
    pub fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.locale = locale;
        self.settings.locale = locale;
        cx.notify();
    }
    pub fn set_recovery_state(&mut self, enabled: bool, available: bool, cx: &mut Context<Self>) {
        self.recovery_enabled = enabled;
        self.recovery_available = available;
        cx.notify();
    }
    pub fn locale(&self) -> Locale {
        self.locale
    }
    fn emit(&mut self, event: PreferencesEvent, cx: &mut Context<Self>) {
        cx.emit(event);
    }
    fn content(&self, cx: &mut Context<Self>) -> Div {
        let locale = self.locale;
        match self.section {
            Section::Editor => div().flex().flex_col().gap_3()
                .child(group(text(locale, "SQL 편집기", "SQL editor"))
                    .child(note(text(locale, "글꼴 크기 · 선택하면 열린 SQL 편집기에 적용됩니다.", "Font size · applies to open SQL editors.")))
                    .child(div().flex().gap_2().flex_wrap().children([10_u16, 12, 14, 16, 18, 20, 24].into_iter().map(|value| {
                        Button::new(("preferences-font", value as usize)).label(format!("{value} px")).small()
                            .when(self.settings.editor_font_size == value, |b| b.primary())
                            .on_click(cx.listener(move |s, _, _, cx| s.emit(PreferencesEvent::FontSizeChosen(value), cx)))
                    })))
                    .child(note(text(locale, "탭 너비 · 공백 수", "Tab width · spaces")))
                    .child(div().flex().gap_2().children([2_u8, 4, 8].into_iter().map(|value| {
                        Button::new(("preferences-tab", value as usize)).label(value.to_string()).small()
                            .when(self.settings.editor_tab_width == value, |b| b.primary())
                            .on_click(cx.listener(move |s, _, _, cx| s.emit(PreferencesEvent::TabWidthChosen(value), cx)))
                    }))))
                .child(group(text(locale, "화면 구성", "Appearance"))
                    .child(note(text(locale, "어두운 테마 · SQL 고정폭 글꼴", "Dark theme · monospaced SQL")))
                    .child(note(text(locale, "탐색기 경계와 편집기 아래 경계를 드래그하여 크기를 바꾸세요. 창을 다시 열어도 유지됩니다.", "Drag the explorer edge or the boundary below the editor to resize. Sizes persist across restarts.")))),
            Section::Results => group(text(locale, "테이블 데이터", "Table data"))
                .child(note(text(locale, "페이지당 행 수 · 다음 테이블 조회부터 적용됩니다.", "Rows per page · applies to subsequent table loads.")))
                .child(div().flex().gap_2().flex_wrap().children([50_u16, 100, 250, 500].into_iter().map(|value| {
                    Button::new(("preferences-page", value as usize)).label(value.to_string()).small()
                        .when(self.settings.table_page_size == value, |b| b.primary())
                        .on_click(cx.listener(move |s, _, _, cx| s.emit(PreferencesEvent::PageSizeChosen(value), cx)))
                })))
                .child(note(text(locale, "쿼리 실행 결과의 안전 제한은 별도로 적용됩니다. 행 수를 바꾸어도 SQL을 자동 실행하지 않습니다.", "Query result safety limits apply separately. Changing this setting does not run SQL."))),
            Section::Recovery => group(text(locale, "SQL 초안 복구", "SQL draft recovery"))
                .child(note(text(locale, "선택 사항 · 기본값 꺼짐", "Optional · off by default")))
                .child(div().flex().gap_2().children([false, true].into_iter().enumerate().map(|(index, enabled)| {
                    Button::new(("preferences-recovery", index))
                        .label(if enabled { text(locale, "사용", "Enable") } else { text(locale, "사용 안 함", "Disable") })
                        .small().disabled(!self.recovery_available)
                        .when(self.recovery_enabled == enabled, |b| b.primary())
                        .on_click(cx.listener(move |s, _, _, cx| s.emit(PreferencesEvent::RecoveryChosen(enabled), cx)))
                })))
                .child(note(text(locale, "사용하면 SQL 원문과 로컬 파일 경로를 이 기기에 저장합니다(0600 권한). SQL에 포함된 비밀번호·개인정보도 저장될 수 있습니다. 원본 SQL 파일은 변경하지 않습니다.", "When enabled, SQL text and local file paths are saved on this device (0600 permissions). SQL may contain passwords or personal data. Original SQL files are not changed.")))
                .child(note(text(locale, "끄면 기존 로컬 스냅샷을 보존하되 복원하지 않습니다. 민감한 SQL은 이 설정을 켜기 전에 제거하세요.", "Disabling retains the prior local snapshot without restoring it. Remove sensitive SQL before enabling.")))
                .when(!self.recovery_available, |el| el.child(note(text(locale, "복구 설정을 불러오는 중이거나 저장소를 사용할 수 없습니다. 작업 공간의 상태 안내를 확인하세요.", "Recovery settings are loading or storage is unavailable. Check the workspace status.")))),
            Section::Language => group(tr(locale, Message::Language))
                .child(div().flex().gap_2().children(Locale::ALL.iter().copied().enumerate().map(|(index, choice)| {
                    Button::new(("preferences-language", index))
                        .label(match choice { Locale::Ko => tr(locale, Message::Korean), Locale::En => tr(locale, Message::English) })
                        .small().when(choice == locale, |b| b.primary())
                        .on_click(cx.listener(move |s, _, _, cx| s.emit(PreferencesEvent::LocaleChosen(choice), cx)))
                })))
                .child(note(tr(locale, Message::LanguageExplanation)))
                .child(note(tr(locale, Message::OriginalDataPreserved))),
            Section::Transfer => group(text(locale, "설정 내보내기·가져오기", "Export / import settings"))
                .child(note(text(locale, "연결 프로필·화면 설정·SQL 복구 사용 여부를 JSON 파일로 옮깁니다.", "Transfer connection profiles, appearance settings, and SQL recovery preference in a JSON file.")))
                .child(note(text(locale, "비밀번호·Keychain·쿼리 기록·SQL 초안·실제 DB 파일은 포함되지 않습니다.", "Passwords, Keychain entries, query history, SQL drafts, and database files are excluded.")))
                .child(Button::new("preferences-transfer").small().label(text(locale, "설정 이동 열기…", "Open settings transfer…"))
                    .on_click(cx.listener(|s, _, _, cx| s.emit(PreferencesEvent::TransferRequested, cx))))
                .child(note(text(locale, "가져오기는 새 설정 폴더와 새 창을 사용합니다. 기존 설정을 덮어쓰거나 DB에 자동 연결하지 않습니다.", "Import uses a new settings folder and window. Existing settings are preserved; databases are not automatically connected."))),
            Section::Keyboard => group(text(locale, "키보드 단축키", "Keyboard shortcuts"))
                .children([
                    ("⌘ ,", tr(locale, Message::Preferences)),
                    ("⌘ ↵", text(locale, "스크립트 실행", "Run script")),
                    ("⌘ ⇧ ↵", text(locale, "현재 구문 실행", "Run current statement")),
                    ("⌘ Space", text(locale, "SQL 자동 완성", "SQL completion")),
                    ("⌘ ⇧ F", text(locale, "SQL 서식 정리", "Format SQL")),
                    ("⌘ O", text(locale, "SQL 파일 열기", "Open SQL file")),
                    ("⌘ S", text(locale, "SQL 파일 저장", "Save SQL file")),
                    ("⌘ ⇧ S", text(locale, "다른 이름으로 저장", "Save SQL as")),
                ].into_iter().map(|(keys, label)| div().flex().justify_between().gap_4()
                    .child(note(label))
                    .child(div().font_family("Menlo").text_size(px(12.)).text_color(rgb(0xc4d4e9)).child(keys))))
                .child(note(text(locale, "macOS 단축키 · 현재는 키 변경을 지원하지 않습니다.", "macOS shortcuts · key remapping is not currently supported."))),
        }
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
            .p_4()
            .gap_3()
            .bg(rgb(0x111a26))
            .text_color(rgb(0xdce4ef))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(20.))
                    .child(tr(locale, Message::Preferences)),
            )
            .child(note(text(
                locale,
                "이 기기에 저장 · 변경 사항은 저장 후 즉시 적용",
                "Saved on this device · changes apply after saving",
            )))
            .child(
                div().flex().gap_2().flex_wrap().children(
                    Section::ALL
                        .into_iter()
                        .enumerate()
                        .map(|(index, section)| {
                            Button::new(("preferences-section", index))
                                .label(section.title(locale))
                                .small()
                                .when(self.section == section, |button| button.primary())
                                .on_click(cx.listener(move |s, _, _, cx| {
                                    s.section = section;
                                    cx.notify();
                                }))
                        }),
                ),
            )
            .child(
                div()
                    .id("preferences-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.content(cx)),
            )
    }
}
