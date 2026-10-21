//! Offline Korean/English application messages. SQL and server data remain verbatim.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

pub const MAX_ARGUMENTS: usize = 16;
pub const MAX_ARGUMENT_BYTES: usize = 4096;
pub const MAX_FORMATTED_BYTES: usize = 64 * 1024;

/// Stable persisted values are exactly `"ko"` and `"en"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Locale {
    #[default]
    #[serde(rename = "ko")]
    Ko,
    #[serde(rename = "en")]
    En,
}
impl Locale {
    pub const ALL: &[Self] = &[Self::Ko, Self::En];
    pub const fn code(self) -> &'static str {
        match self {
            Self::Ko => "ko",
            Self::En => "en",
        }
    }
}

// One row defines a typed key and both languages. Adding a key cannot omit a locale.
macro_rules! catalog {
    ($($key:ident => ($ko:literal, $en:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Message { $($key),+ }
        impl Message {
            pub const ALL: &'static [Self] = &[$(Self::$key),+];
            pub const fn id(self) -> &'static str { match self { $(Self::$key => stringify!($key)),+ } }
            pub fn from_id(id: &str) -> Option<Self> { match id { $(stringify!($key) => Some(Self::$key)),+, _ => None } }
        }
        pub const fn tr(locale: Locale, message: Message) -> &'static str {
            match message { $(Message::$key => match locale { Locale::Ko => $ko, Locale::En => $en }),+ }
        }
    };
}
catalog! {
    AppName => ("데이터베이스 IDE", "Database IDE"),
    LocalWorkspace => ("로컬 작업 공간", "Local workspace"),
    MenuFile => ("파일", "File"),
    MenuEdit => ("편집", "Edit"),
    MenuView => ("보기", "View"),
    MenuQuery => ("쿼리", "Query"),
    MenuDatabase => ("데이터베이스", "Database"),
    Preferences => ("환경설정", "Preferences"),
    Language => ("언어", "Language"),
    Korean => ("한국어", "한국어"),
    English => ("English", "English"),
    Close => ("닫기", "Close"),
    Save => ("저장", "Save"),
    Open => ("열기", "Open"),
    Copy => ("복사", "Copy"),
    Retry => ("다시 시도", "Retry"),
    Undo => ("실행 취소", "Undo"),
    Redo => ("다시 실행", "Redo"),
    Cancel => ("취소", "Cancel"),
    Connections => ("연결", "Connections"),
    NewConnection => ("새 연결", "New connection"),
    EditConnection => ("연결 편집", "Edit connection"),
    TestConnection => ("연결 테스트", "Test connection"),
    SaveConnection => ("연결 저장", "Save connection"),
    RemoveConnection => ("연결 삭제", "Remove connection"),
    Connect => ("연결", "Connect"),
    Disconnect => ("연결 해제", "Disconnect"),
    ConnectionName => ("연결 이름", "Connection name"),
    Host => ("호스트", "Host"),
    Port => ("포트", "Port"),
    Database => ("데이터베이스", "Database"),
    Username => ("사용자 이름", "Username"),
    Password => ("비밀번호", "Password"),
    DatabaseFile => ("데이터베이스 파일", "Database file"),
    TlsMode => ("TLS 모드", "TLS mode"),
    ReadOnly => ("읽기 전용", "Read-only"),
    ConnectionTimeout => ("연결 제한 시간(초)", "Connection timeout (seconds)"),
    Explorer => ("탐색기", "Explorer"),
    Refresh => ("새로고침", "Refresh"),
    SearchObjects => ("객체 검색", "Search objects"),
    Tables => ("테이블", "Tables"),
    Views => ("뷰", "Views"),
    Columns => ("열", "Columns"),
    Indexes => ("인덱스", "Indexes"),
    Schemas => ("스키마", "Schemas"),
    SelectConnection => ("연결 선택", "Select connection"),
    SelectDatabase => ("데이터베이스 선택", "Select database"),
    WorkingDatabase => ("작업 데이터베이스", "Working database"),
    OpenConsoleInDatabase => ("이 데이터베이스에서 콘솔 열기", "Open console in this database"),
    ConsoleContext => ("콘솔 연결 정보", "Console context"),
    NoDatabaseSelected => ("선택한 데이터베이스 없음", "No database selected"),
    RefreshDatabases => ("데이터베이스 목록 새로고침", "Refresh databases"),
    DatabaseUnavailable => ("데이터베이스를 사용할 수 없음", "Database unavailable"),
    DatabaseContext => ("데이터베이스 연결 정보", "Database context"),
    NoConnection => ("연결 없음", "No connection"),
    NotConnected => ("연결되지 않음", "Not connected"),
    Connecting => ("연결 중…", "Connecting…"),
    Connected => ("연결됨", "Connected"),
    Disconnecting => ("연결 해제 중…", "Disconnecting…"),
    ConnectionSucceeded => ("연결 테스트 성공", "Connection test succeeded"),
    ConnectionFailed => ("연결 실패: {error}", "Connection failed: {error}"),
    QueryFailed => ("쿼리 실패: {error}", "Query failed: {error}"),
    Error => ("오류", "Error"),
    ErrorDetails => ("오류 상세 정보", "Error details"),
    Run => ("실행", "Run"),
    RunSelection => ("선택 영역 실행", "Run selection"),
    NewConsole => ("새 콘솔", "New console"),
    QueryHistory => ("쿼리 기록", "Query history"),
    Results => ("결과", "Results"),
    Messages => ("메시지", "Messages"),
    NoResults => ("결과 없음", "No results"),
    RowsReturned => ("결과 {count}행", "{count} rows returned"),
    DirtyDocument => ("저장하지 않은 변경사항", "Unsaved changes"),
    UnsavedChanges => ("변경사항을 저장하지 않았습니다. 닫기 전에 저장하시겠습니까?", "This document has unsaved changes. Save before closing?"),
    KeepEditing => ("계속 편집", "Keep editing"),
    DiscardChanges => ("변경사항 버리기", "Discard changes"),
    RecoveryAvailable => ("복구할 편집 내용이 있습니다", "Editor recovery is available"),
    RestoreRecovery => ("편집 내용 복구", "Restore editor recovery"),
    SaveRecovery => ("복구본 저장", "Save recovery snapshot"),
    RecoveryFailed => ("편집 내용 복구 실패: {error}", "Editor recovery failed: {error}"),
    CancelRequested => ("취소 요청됨…", "Cancellation requested…"),
    Cancelled => ("취소됨", "Cancelled"),
    SessionRetired => ("세션이 종료되었습니다. 새로 연결해야 합니다.", "Session retired. Reconnect before continuing."),
    TransactionUnknown => ("트랜잭션 결과를 확인할 수 없습니다. 작업을 반복하기 전에 데이터베이스 상태를 확인하세요.", "Transaction outcome is unknown. Check database state before replaying the operation."),
    ReadOnlySession => ("읽기 전용 세션", "Read-only session"),
    ActiveTransaction => ("트랜잭션 진행 중", "Transaction active"),
    IdleSession => ("대기 중인 세션", "Session idle"),
    ConnectedTo => ("{name}에 연결됨", "Connected to {name}"),
    ConnectingTo => ("{name}에 연결 중…", "Connecting to {name}…"),
    SelectedDatabase => ("작업 데이터베이스: {database}", "Working database: {database}"),
    ContextUnavailable => ("이 콘솔의 연결 정보를 사용할 수 없습니다", "This console's connection context is unavailable"),
    UnsupportedOperation => ("이 엔진에서 지원하지 않는 작업입니다", "This operation is not supported by this engine"),
    InvalidInput => ("입력값을 확인하세요", "Check the input values"),
    Ready => ("준비됨", "Ready"),
    Loading => ("불러오는 중…", "Loading…"),
    ConnectionRequired => ("먼저 데이터베이스에 연결하세요", "Connect to a database first"),
    SaveFailed => ("저장 실패: {error}", "Save failed: {error}"),
    OpenFailed => ("파일 열기 실패: {error}", "Could not open file: {error}"),
    SettingsSaved => ("환경설정 저장됨", "Preferences saved"),
    LanguageExplanation => ("앱 메뉴와 안내 문구의 언어를 선택하세요.", "Choose the language for menus and messages."),
    OriginalDataPreserved => ("SQL·식별자·서버 원본 오류는 유지됩니다. / SQL, identifiers and original server diagnostics stay unchanged.", "SQL, identifiers and original server diagnostics stay unchanged. / SQL·식별자·서버 원본 오류는 유지됩니다."),
}

/// Application-owned runtime locale, with no implicit global mutation or persistence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Localizer {
    locale: Locale,
}
impl Localizer {
    pub const fn new(locale: Locale) -> Self {
        Self { locale }
    }
    pub const fn locale(self) -> Locale {
        self.locale
    }
    pub fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
    }
    pub const fn text(self, message: Message) -> &'static str {
        tr(self.locale, message)
    }
    pub fn format(
        self,
        message: Message,
        arguments: &[(&str, &str)],
    ) -> Result<String, FormatError> {
        format(self.locale, message, arguments)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    TooManyArguments,
    ArgumentTooLarge,
    InvalidArgumentName,
    DuplicateArgument,
    MissingArgument,
    UnknownArgument,
    InvalidTemplate,
    OutputTooLarge,
}
impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooManyArguments => "too many translation arguments",
            Self::ArgumentTooLarge => "translation argument exceeds byte limit",
            Self::InvalidArgumentName => "invalid translation argument name",
            Self::DuplicateArgument => "duplicate translation argument",
            Self::MissingArgument => "required translation argument is missing",
            Self::UnknownArgument => "unknown translation argument",
            Self::InvalidTemplate => "invalid translation template",
            Self::OutputTooLarge => "formatted translation exceeds byte limit",
        })
    }
}
impl std::error::Error for FormatError {}
fn name_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|c| c.is_ascii_lowercase() || c == b'_')
}
fn placeholders(template: &str) -> Result<BTreeSet<&str>, FormatError> {
    let mut names = BTreeSet::new();
    let mut remaining = template;
    while let Some(index) = remaining.find('{') {
        if remaining[..index].contains('}') {
            return Err(FormatError::InvalidTemplate);
        }
        let tail = &remaining[index + 1..];
        let end = tail.find('}').ok_or(FormatError::InvalidTemplate)?;
        let name = &tail[..end];
        if !name_valid(name) {
            return Err(FormatError::InvalidTemplate);
        }
        names.insert(name);
        remaining = &tail[end + 1..];
    }
    if remaining.contains('}') {
        return Err(FormatError::InvalidTemplate);
    }
    Ok(names)
}
/// Arguments are inserted verbatim once, including SQL/identifiers/raw server errors.
/// This is text interpolation, not HTML/SQL escaping or an execution API.
pub fn format(
    locale: Locale,
    message: Message,
    arguments: &[(&str, &str)],
) -> Result<String, FormatError> {
    if arguments.len() > MAX_ARGUMENTS {
        return Err(FormatError::TooManyArguments);
    }
    let template = tr(locale, message);
    let expected = placeholders(template)?;
    let mut provided = BTreeSet::new();
    for (name, value) in arguments {
        if !name_valid(name) {
            return Err(FormatError::InvalidArgumentName);
        }
        if value.len() > MAX_ARGUMENT_BYTES {
            return Err(FormatError::ArgumentTooLarge);
        }
        if !provided.insert(*name) {
            return Err(FormatError::DuplicateArgument);
        }
        if !expected.contains(name) {
            return Err(FormatError::UnknownArgument);
        }
    }
    if provided != expected {
        return Err(FormatError::MissingArgument);
    }
    let mut result = String::new();
    let mut remaining = template;
    while let Some(index) = remaining.find('{') {
        push(&mut result, &remaining[..index])?;
        let tail = &remaining[index + 1..];
        let end = tail.find('}').ok_or(FormatError::InvalidTemplate)?;
        let value = arguments
            .iter()
            .find(|(name, _)| *name == &tail[..end])
            .map(|(_, value)| *value)
            .ok_or(FormatError::MissingArgument)?;
        push(&mut result, value)?;
        remaining = &tail[end + 1..];
    }
    push(&mut result, remaining)?;
    Ok(result)
}
fn push(target: &mut String, value: &str) -> Result<(), FormatError> {
    if target
        .len()
        .checked_add(value.len())
        .is_none_or(|len| len > MAX_FORMATTED_BYTES)
    {
        return Err(FormatError::OutputTooLarge);
    }
    target.push_str(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_complete_and_placeholder_parity() {
        let mut ids = BTreeSet::new();
        for &message in Message::ALL {
            assert!(ids.insert(message.id()), "duplicate message id");
            assert_eq!(Message::from_id(message.id()), Some(message));
            let ko = tr(Locale::Ko, message);
            let en = tr(Locale::En, message);
            assert!(!ko.trim().is_empty(), "Korean missing: {}", message.id());
            assert!(!en.trim().is_empty(), "English missing: {}", message.id());
            assert_eq!(
                placeholders(ko).unwrap(),
                placeholders(en).unwrap(),
                "placeholder mismatch: {}",
                message.id()
            );
        }
        assert!(Message::from_id("MissingKey").is_none());
        assert!(Message::ALL.len() >= 60);
    }
    #[test]
    fn locale_persistence_and_korean_default() {
        assert_eq!(Locale::default(), Locale::Ko);
        assert_eq!(Localizer::default().text(Message::Preferences), "환경설정");
        for &locale in Locale::ALL {
            let json = serde_json::to_string(&locale).unwrap();
            assert_eq!(json, format!("\"{}\"", locale.code()));
            assert_eq!(serde_json::from_str::<Locale>(&json).unwrap(), locale);
        }
        assert!(serde_json::from_str::<Locale>("\"fr\"").is_err());
        assert!(serde_json::from_str::<Locale>("\"ko-KR\"").is_err());
        let mut runtime = Localizer::default();
        runtime.set_locale(Locale::En);
        assert_eq!(runtime.text(Message::Preferences), "Preferences");
    }
    #[test]
    fn bounded_arguments_and_missing_unknown_duplicate_rejected() {
        assert_eq!(
            format(Locale::Ko, Message::ConnectionFailed, &[]),
            Err(FormatError::MissingArgument)
        );
        assert_eq!(
            format(Locale::En, Message::Run, &[("unused", "x")]),
            Err(FormatError::UnknownArgument)
        );
        assert_eq!(
            format(
                Locale::Ko,
                Message::ConnectionFailed,
                &[("error", "a"), ("error", "b")]
            ),
            Err(FormatError::DuplicateArgument)
        );
        assert_eq!(
            format(
                Locale::Ko,
                Message::ConnectionFailed,
                &[("error", &"x".repeat(MAX_ARGUMENT_BYTES + 1))]
            ),
            Err(FormatError::ArgumentTooLarge)
        );
        assert_eq!(
            format(
                Locale::Ko,
                Message::Run,
                &vec![("x", ""); MAX_ARGUMENTS + 1]
            ),
            Err(FormatError::TooManyArguments)
        );
        assert_eq!(
            format(Locale::Ko, Message::Run, &[("not-a-name", "")]),
            Err(FormatError::InvalidArgumentName)
        );
        let mut output = "x".repeat(MAX_FORMATTED_BYTES);
        assert_eq!(push(&mut output, "한"), Err(FormatError::OutputTooLarge));
        assert_eq!(output.len(), MAX_FORMATTED_BYTES);
    }
    #[test]
    fn sql_identifiers_and_raw_errors_remain_verbatim() {
        let raw = "ERROR 42P01: relation \"MixedCase_한글\" missing; SQL: SELECT '{error}' FROM \"MixedCase_한글\";";
        let ko = Localizer::new(Locale::Ko)
            .format(Message::QueryFailed, &[("error", raw)])
            .unwrap();
        let en = Localizer::new(Locale::En)
            .format(Message::QueryFailed, &[("error", raw)])
            .unwrap();
        assert_eq!(ko, format!("쿼리 실패: {raw}"));
        assert_eq!(en, format!("Query failed: {raw}"));
        let database = "ACME Prod / Sales_한국어";
        assert_eq!(
            format(
                Locale::Ko,
                Message::SelectedDatabase,
                &[("database", database)]
            )
            .unwrap(),
            format!("작업 데이터베이스: {database}")
        );
    }
    #[test]
    fn every_message_formats_in_each_locale_with_declared_arguments() {
        for &message in Message::ALL {
            for &locale in Locale::ALL {
                let names = placeholders(tr(locale, message)).unwrap();
                let args: Vec<_> = names
                    .iter()
                    .map(|name| (*name, "fixture_한글 {literal}"))
                    .collect();
                let value = format(locale, message, &args).unwrap();
                assert!(!value.is_empty());
            }
        }
    }
    #[test]
    fn malformed_catalog_syntax_rejected() {
        for template in [
            "missing {end",
            "extra }",
            "bad {Name}",
            "nested {{a}}",
            "empty {}",
        ] {
            assert!(placeholders(template).is_err());
        }
        assert_eq!(placeholders("{name}/{name}").unwrap().len(), 1);
    }
}
