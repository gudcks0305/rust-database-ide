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
    CloseTable => ("테이블 닫기", "Close table"),
    TableData => ("테이블 데이터", "Table data"),
    Transaction => ("트랜잭션", "Transaction"),
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
    ToolsNoProfileCapturedOfflineToolsAvailable => ("연결 프로필 없음 · 오프라인 도구 사용 가능", "No profile captured · offline tools available"),
    ToolsReadyDatabaseWorkflowsUseDedicatedRead => ("준비됨 · 전용 읽기 전용 세션 사용", "Ready · database workflows use dedicated read-only sessions"),
    ToolsNotLoaded => ("불러오지 않음", "Not loaded"),
    ToolsActionNeedsAttention => ("입력 또는 작업을 확인하세요", "Action needs attention"),
    ToolsCatalogCapturedCompleteWithinExplicitSchema => ("카탈로그 수집 완료 · 지정한 스키마 범위 전체", "Catalog captured · complete within explicit schema scope"),
    ToolsDesiredSchemaLoadedCompareDirectionDesired => ("목표 스키마 불러옴 · 비교 방향: 목표 → 수집한 실제 상태", "Desired schema loaded · compare direction: desired → captured actual"),
    ToolsMigrationPreviewReadyNoSQLExecuted => ("마이그레이션 미리보기 준비됨 · SQL 실행 없음", "Migration preview ready · no SQL executed"),
    ToolsPlanOnlyANALYZENeverRequestedNo => ("예상 계획만 표시 · ANALYZE 요청 없음 · 실제 실행 시간 없음", "Plan only · ANALYZE never requested · no actual timing metrics"),
    ToolsTaggedJSONLoadedSelectExplicitUnique => ("태그 JSON 불러옴 · 중복·NULL 없는 키 열을 지정하세요", "Tagged JSON loaded · select explicit unique non-NULL keys"),
    ToolsExactTypedDataComparisonReadyBefore => ("타입을 보존한 데이터 비교 완료 · 이전 → 이후", "Exact typed data comparison ready · before → after"),
    ToolsSQLDraftReadyCopySaveOr => ("SQL 초안 준비됨 · 복사·저장하거나 새 콘솔에서 검토하세요 · 실행 없음", "SQL draft ready · copy/save or open a new console for explicit review; nothing executed"),
    ToolsExportSavedAtomicallyExistingFilesAre => ("내보내기 원자적 저장 완료 · 기존 파일 덮어쓰기 없음", "Export saved atomically · existing files are never overwritten"),
    ToolsFileSelectionCancelled => ("파일 선택 취소됨", "File selection cancelled"),
    ToolsOperationFailedPreviousSuccessfulInputsRetained => ("작업 실패 · 이전에 성공한 입력 유지", "Operation failed · previous successful inputs retained"),
    ToolsReadOnlyWorkerStoppedBeforeReturning => ("읽기 전용 작업자가 결과 반환 전에 종료됨", "Read-only worker stopped before returning a result"),
    ToolsCancelledPendingReadOnlySessionRetiring => ("취소됨 · 대기 중인 읽기 전용 세션 종료 중 · 이전 결과 무시", "Cancelled · pending read-only session retiring; stale result discarded"),
    ToolsSchemaCaptureCurrentlyRequiresAPostgreSQL => ("스키마 수집은 이 도구를 열 때 지정한 PostgreSQL 프로필이 필요합니다", "Schema capture currently requires a PostgreSQL profile captured when this panel opens"),
    ToolsCapturingScopedCatalog => ("지정한 범위의 카탈로그 수집 중…", "Capturing scoped catalog…"),
    ToolsEXPLAINCurrentlyRequiresAPostgreSQLProfile => ("EXPLAIN은 이 도구를 열 때 지정한 PostgreSQL 프로필이 필요합니다", "EXPLAIN currently requires a PostgreSQL profile captured when this panel opens"),
    ToolsPlanningExactlyOneStatement => ("단일 SQL 문장의 예상 계획 생성 중…", "Planning exactly one statement…"),
    ToolsLoadDesiredSchemaAndCaptureActual => ("목표 스키마를 불러오고 실제 카탈로그를 먼저 수집하세요", "Load desired schema and capture actual catalog first"),
    ToolsComparingDesiredActual => ("목표 → 실제 상태 비교 중…", "Comparing desired → actual…"),
    ToolsOpenBothBeforeAndAfterTagged => ("이전·이후 태그 JSON 파일을 모두 여세요", "Open both before and after tagged JSON files first"),
    ToolsComparingTypedRowsByExplicitIdentity => ("지정한 키로 타입을 보존한 행 비교 중…", "Comparing typed rows by explicit identity…"),
    ToolsChooseLocalInput => ("로컬 입력 파일을 선택하세요…", "Choose local input…"),
    ToolsOpenLocalSchemaOrTaggedJSON => ("로컬 스키마 또는 태그 JSON 열기", "Open local schema or tagged JSON"),
    ToolsNoInputFileSelected => ("선택한 입력 파일 없음", "No input file selected"),
    ToolsNativeFileDialogFailed => ("파일 선택 대화상자 오류", "Native file dialog failed"),
    ToolsCouldNotOpenSelectedFile => ("선택한 파일을 열 수 없습니다", "Could not open selected file"),
    ToolsFileWorkerStopped => ("파일 작업자가 종료되었습니다", "File worker stopped"),
    ToolsCreateTheRequestedOutputFirst => ("내보낼 결과를 먼저 생성하세요", "Create the requested output first"),
    ToolsGenerateAnObjectSQLDraftFirst => ("객체 SQL 초안을 먼저 생성하세요", "Generate an object SQL draft first"),
    ToolsChooseNewExportDestination => ("새 내보내기 경로를 선택하세요…", "Choose new export destination…"),
    ToolsNativeSaveDialogFailed => ("저장 대화상자 오류", "Native save dialog failed"),
    ToolsExportWorkerStopped => ("내보내기 작업자가 종료되었습니다", "Export worker stopped"),
    ToolsCaptureACompletePostgreSQLCatalogWithin => ("지정한 1–32개 스키마 범위의 PostgreSQL 카탈로그 전체를 수집합니다.", "Capture a complete PostgreSQL catalog within 1–32 explicitly selected schemas."),
    ToolsSchemaScopeCommaSeparated => ("스키마 범위, 쉼표로 구분", "Schema scope, comma separated"),
    ToolsCaptureSchema => ("스키마 수집", "Capture schema"),
    ToolsSaveSnapshotJSON => ("스냅샷 JSON 저장…", "Save snapshot JSON…"),
    ToolsOpenDesiredJSONDDL => ("목표 JSON / DDL 열기…", "Open desired JSON / DDL…"),
    ToolsPostgreSQLOnlyLiveCatalog => ("PostgreSQL 전용 실시간 카탈로그", "PostgreSQL-only live catalog"),
    ToolsSchemaCaptureObjectBrowsingDraftsAnd => ("스키마 수집·객체 탐색과 초안·EXPLAIN은 PostgreSQL 프로필이 필요합니다. 현재 엔진의 실시간 작업은 미지원입니다. 오프라인 목표 스키마와 데이터 비교는 사용할 수 있습니다.", "Schema capture, object browsing/drafts and EXPLAIN require a PostgreSQL profile. This captured engine is not supported by these live workflows; offline desired-schema and data compare remain available."),
    ToolsCapturedActual => ("수집한 실제 상태", "Captured actual"),
    ToolsSchemasTablesTablesColumnsColumns => ("스키마 {value_a}개 · 테이블 {tables}개 · 열 {columns}개", "{value_a} schemas · {tables} tables · {columns} columns"),
    ToolsScopeComplete => ("범위: {value_a} · 전체 수집: {value_b}", "Scope: {value_a} · complete: {value_b}"),
    ToolsDatabase => ("데이터베이스: {value_a} · {value_b}", "Database: {value_a} · {value_b}"),
    ToolsDesiredSchema => ("목표 스키마", "Desired schema"),
    ToolsTables => ("{value_a} · 테이블 {value_b}개", "{value_a} · {value_b} tables"),
    ToolsScope => ("범위: {value_a} · {value_b}", "Scope: {value_a} · {value_b}"),
    ToolsCatalogAndPlanRequestsHave15 => ("카탈로그·계획 요청 제한 시간은 문장당 15초입니다. 상위 프로필 변경은 현재 도구의 연결 대상을 바꾸지 않습니다. 파일은 백그라운드에서 처리합니다. SQLite/MySQL 실시간 도구는 미지원입니다.", "Catalog and plan requests have 15 s statement limits; parent profile changes do not retarget this panel. Files are processed in background workers. SQLite/MySQL live tools remain unavailable here."),
    ToolsDesiredCapturedActual => ("목표 → 수집한 실제 상태", "Desired → captured actual"),
    ToolsPreviewOperationsTOActualToMake => ("실제 데이터베이스를 목표 상태로 바꾸는 작업을 미리 봅니다.", "Preview operations TO actual to make it match desired."),
    ToolsPreviewOnlyNoDatabaseApplyAction => ("미리보기 전용입니다. 데이터베이스 적용 기능은 없습니다. 내보낸 SQL은 별도 검토와 실행이 필요합니다.", "Preview only. No database apply action is exposed; exported SQL requires separate review and execution."),
    ToolsCompareSchemas => ("스키마 비교", "Compare schemas"),
    ToolsExportSQLPreview => ("SQL 미리보기 내보내기…", "Export SQL preview…"),
    ToolsOperationsDestructiveUnsupported => ("작업 {value_a}개 · 파괴 작업: {value_b} · 미지원: {value_c}", "{value_a} operations · destructive: {value_b} · unsupported: {value_c}"),
    ToolsDesiredActualImplicitCommit => ("목표: {value_a} → 실제: {value_b} · 암시적 커밋: {value_c}", "Desired: {value_a} → Actual: {value_b} · implicit commit: {value_c}"),
    ToolsMigrationOperationsDesiredToActual => ("목표에서 실제 상태로 향하는 마이그레이션 작업", "Migration operations, desired to actual"),
    ToolsDestructiveDataValidationBlocked => ("파괴 작업: {value_a} · 데이터 검증 필요: {value_b} · 차단됨: {value_c}", "Destructive: {value_a} · data validation: {value_b} · blocked: {value_c}"),
    ToolsPreviewCappedAt300OperationsExport => ("작업 미리보기는 300개까지 표시합니다. 내보내기에는 제한 범위 내 계획 전체가 포함됩니다.", "Preview capped at 300 operations. Export contains the complete bounded plan."),
    ToolsCaptureActualCatalogAndLoadDesired => ("스키마 탭에서 실제 카탈로그를 수집하고 목표 스키마를 불러온 뒤 비교하세요.", "Capture actual catalog and load desired schema in Schema tab, then compare."),
    ToolsExportERSVG => ("ER SVG 내보내기…", "Export ER SVG…"),
    Tools80Tables24ColumnsTable200 => ("최대 테이블 80개 · 테이블당 열 24개 · 관계 200개", "80 tables · 24 columns/table · 200 edges maximum"),
    ToolsTablesForeignKeys => ("테이블 {value_a}개 · 외래 키 {value_b}개", "{value_a} tables · {value_b} foreign keys"),
    ToolsForeignKeysFromCapturedCatalogNo => ("수집한 카탈로그의 외래 키입니다. 관계를 추정하지 않습니다. SVG 미리보기와 접근성 표는 같은 제한된 그래프를 사용합니다.", "Foreign keys from captured catalog. No inferred relationships. SVG preview and accessible table share the same bounded graph."),
    ToolsForeignKeyRelationships => ("외래 키 관계", "Foreign key relationships"),
    ToolsOutsideDisplayedScope => ("표시 범위 밖", "Outside displayed scope"),
    ToolsUnnamedForeignKey => ("이름 없는 외래 키", "unnamed foreign key"),
    ToolsTargetInScopeColumnEndpointsVisible => ("대상이 범위 내에 있음: {value_a} · 연결 열 표시됨: {value_b}", "Target in scope: {value_a} · column endpoints visible: {value_b}"),
    ToolsERGenerationFailedE => ("ER 생성 실패: {e}", "ER generation failed: {e}"),
    ToolsCaptureASchemaFirstRelationshipsAnd => ("스키마를 먼저 수집하세요. 해당 스냅샷으로 관계와 SVG를 생성합니다.", "Capture a schema first. Relationships and SVG will be generated from that exact snapshot."),
    ToolsPlanOnlyNoANALYZE => ("예상 계획만 · ANALYZE 없음", "Plan only · no ANALYZE"),
    ToolsExactlyOneParsedSQLStatementPlanned => ("단일 SQL 문장을 파싱하여 전용 읽기 전용 트랜잭션에서 계획을 생성합니다.", "Exactly one parsed SQL statement, planned in a dedicated read-only transaction."),
    ToolsEstimatedCostsRowsArePlannerEstimates => ("비용·행 수는 플래너 추정값입니다. 계획 중 잠금이 발생할 수 있으며 문장·잠금 제한 시간을 적용합니다.", "Estimated costs/rows are planner estimates. Planning may take locks; statement and lock timeouts are enforced."),
    ToolsExactlyOneSQLStatementForPlan => ("예상 EXPLAIN 계획을 위한 단일 SQL 문장", "Exactly one SQL statement for plan-only EXPLAIN"),
    ToolsEXPLAINPlanOnly => ("EXPLAIN 예상 계획", "EXPLAIN plan only"),
    ToolsEXPLAINUnavailableForCapturedEngine => ("현재 엔진에서 EXPLAIN 사용 불가", "EXPLAIN unavailable for captured engine"),
    ToolsPostgreSQLPlanOnlyWorkflowIsDisabled => ("PostgreSQL 예상 계획 기능이 비활성화되었습니다. PostgreSQL 프로필로 도구를 다시 여세요.", "PostgreSQL plan-only workflow is disabled. Reopen tools with a PostgreSQL profile to request a live plan."),
    ToolsSQLCapturedForThisPlan => ("이 계획의 SQL 원문", "SQL captured for this plan"),
    ToolsSQLInputChangedDisplayedPlanBelongs => ("SQL 입력이 바뀌었습니다. 표시한 계획은 위 SQL 원문 기준입니다. EXPLAIN을 다시 실행하여 갱신하세요.", "SQL input changed; displayed plan belongs to the captured SQL above. Run EXPLAIN again to refresh."),
    ToolsExpandOrCollapseNodesPreviewCapped => ("노드 펼치기·접기 · 미리보기는 보이는 노드 300개까지", "Expand or collapse nodes · preview capped at 300 visible nodes."),
    ToolsEstimatedQueryPlanTree => ("쿼리 예상 계획 트리", "Estimated query plan tree"),
    ToolsEstimatedCostEstimatedRows => ("{value_a} · 예상 비용 {value_b} · 예상 행 수 {value_c}", "{value_a} · estimated cost {value_b} · estimated rows {value_c}"),
    ToolsCostRowsWidth => ("비용 {value_a}…{value_b} · 행 수 {value_c} · 너비 {value_d}", "cost {value_a}…{value_b} · rows {value_c} · width {value_d}"),
    ToolsCompleteTaggedJSONInputsOnlyExact => ("태그 JSON 전체 입력만 지원합니다. 셀 타입·값을 정확히 비교합니다. 입력당 10,000행 / 16 MiB 제한. 이전 → 이후 방향이며 행 위치나 추정 키를 사용하지 않습니다.", "Complete tagged JSON inputs only · exact cell kinds and values · 10,000 rows / 16 MiB per input. Before → after; no positional or guessed identity."),
    ToolsOpenBeforeJSON => ("이전 JSON 열기…", "Open before JSON…"),
    ToolsOpenAfterJSON => ("이후 JSON 열기…", "Open after JSON…"),
    ToolsComparisonInputs => ("비교 입력", "Comparison inputs"),
    ToolsBeforeRows => ("이전: {value_a} · {value_b}행", "Before: {value_a} · {value_b} rows"),
    ToolsAfterRows => ("이후: {value_a} · {value_b}행", "After: {value_a} · {value_b} rows"),
    ToolsExplicitKeyColumnNamesCommaSeparated => ("명시적 키 열 이름, 쉼표로 구분", "Explicit key column names, comma separated"),
    ToolsCompareByExplicitKeys => ("지정한 키로 비교", "Compare by explicit keys"),
    ToolsAvailableColumns => ("사용 가능한 열: {value_a}", "Available columns: {value_a}"),
    ToolsInsertedDeletedChangedUnchanged => ("추가 {value_a}개 · 삭제 {value_b}개 · 변경 {value_c}개 · 동일 {value_d}개", "{value_a} inserted · {value_b} deleted · {value_c} changed · {value_d} unchanged"),
    ToolsComparedCompleteInputsRowsKeyColumn => ("전체 입력 비교: {value_a} → {value_b}행 · 키 열 인덱스: {value_c}", "Compared complete inputs: {value_a} → {value_b} rows · key column indices: {value_c}"),
    ToolsExactTypedRowDifferencesBeforeTo => ("타입을 보존한 정확한 행 차이, 이전에서 이후로", "Exact typed row differences, before to after"),
    ToolsKey => ("{value_a}. {value_b} · 키 {value_c}", "{value_a}. {value_b} · key {value_c}"),
    ToolsBefore => ("이전: {value_a}", "Before: {value_a}"),
    ToolsAfter => ("이후: {value_a}", "After: {value_a}"),
    ToolsRowPreviewCappedAt100Changes => ("행 미리보기는 변경 100개까지 표시합니다. 요약 수치는 불러온 모든 행을 포함합니다.", "Row preview capped at 100 changes; summary counts cover all loaded rows."),
    ToolsDatabaseTools => ("데이터베이스 도구", "Database tools"),
    ToolsConnectionFixedAtPanelOpenDedicated => ("도구를 열 때 연결 고정 · 전용 읽기 전용 세션", "Connection fixed at panel open · dedicated read-only sessions"),
    ToolsCancel => ("취소", "Cancel"),
    ToolsCaptureAPostgreSQLSchemaBeforeGenerating => ("객체 초안을 생성하기 전에 PostgreSQL 스키마를 수집하세요", "Capture a PostgreSQL schema before generating object drafts"),
    ToolsValidatingExactlyOneObjectSQLDraft => ("단일 객체 SQL 초안 검증 중…", "Validating exactly one object SQL draft…"),
    ToolsSelectACapturedTableForThe => ("인덱스 초안의 수집한 테이블을 선택하세요", "Select a captured table for the index draft"),
    ToolsSelectACapturedOrdinaryViewTo => ("삭제 초안을 생성할 수집한 일반 뷰를 선택하세요", "Select a captured ordinary view to draft its removal"),
    ToolsCaptureAPostgreSQLSchemaFirst => ("PostgreSQL 스키마를 먼저 수집하세요", "Capture a PostgreSQL schema first"),
    ToolsValidatingDestructiveDROPINDEXDraft => ("파괴 작업 DROP INDEX 초안 검증 중…", "Validating destructive DROP INDEX draft…"),
    ToolsIndexNotPresentInCapturedTable => ("수집한 테이블에 해당 인덱스가 없습니다", "Index not present in captured table"),
    ToolsSQLDraftSentToANew => ("검토용 새 콘솔에 SQL 초안 전달됨 · 실행 없음", "SQL draft sent to a new console for review · not executed"),
    ToolsCapturedPostgreSQLObjects => ("수집한 PostgreSQL 객체", "Captured PostgreSQL objects"),
    ToolsBrowseActualTableViewMetadataThen => ("실제 테이블·뷰 메타데이터를 탐색하고 검토용 단일 SQL 문장을 생성합니다.", "Browse actual table/view metadata, then generate one SQL statement for review."),
    ToolsDraftsNeverExecuteHereCopySave => ("여기서는 초안을 실행하지 않습니다. 복사·저장하거나 수집 당시 프로필로 새 콘솔을 여세요. 실행은 작업 공간에서 별도로 선택해야 합니다.", "Drafts never execute here. Copy, save or open a new console with the captured profile. Run remains an explicit workbench action."),
    ToolsCaptureSchemaInSchemaTabTo => ("스키마 탭에서 스키마를 수집하여 실제 객체를 불러오세요.", "Capture schema in Schema tab to load actual objects."),
    ToolsLiveObjectCaptureAndPostgreSQLObject => ("현재 엔진의 실시간 객체 수집과 PostgreSQL 객체 초안은 비활성화되어 있습니다. PostgreSQL 프로필로 도구를 다시 여세요.", "Live object capture and PostgreSQL object drafts are disabled for this captured engine. Reopen tools with a PostgreSQL profile."),
    ToolsTotalTablesTablesTotalViewsViews => ("테이블 {total_tables}개 · 뷰 {total_views}개 · 수집 범위 {value_a}", "{total_tables} tables · {total_views} views · captured scope {value_a}"),
    ToolsTable => ("테이블 {value_a}{value_b}", "Table {value_a}{value_b}"),
    ToolsMaterializedView => ("구체화된 뷰", "Materialized view"),
    ToolsView => ("뷰", "View"),
    ToolsUnsupportedObjectsInShowingFirst100 => ("미지원 객체 {value_a}개 · {value_b} · 처음 100개 표시 · 상세 정보는 스냅샷 JSON 참조", "{value_a} unsupported objects in {value_b} · showing first 100 markers; full details in snapshot JSON"),
    ToolsUnsupportedCatalogObjectIn => ("{value_a}의 미지원 카탈로그 객체: {value_b}", "Unsupported catalog object in {value_a}: {value_b}"),
    ToolsColumnsForeignKeysIndexes => ("열 {value_a}개 · 외래 키 {value_b}개 · 인덱스 {value_c}개", "{value_a} columns · {value_b} foreign keys · {value_c} indexes"),
    ToolsUnsupportedTableFeature => ("미지원 테이블 기능: {value_a}", "Unsupported table feature: {value_a}"),
    ToolsCapturedColumnsAndDefaults => ("수집한 열과 기본값", "Captured columns and defaults"),
    ToolsNullableDefaultGeneratedIdentity => ("NULL 허용: {value_a} · 기본값: {value_b} · 생성 열/identity: {value_c}", "Nullable: {value_a} · default: {value_b} · generated/identity: {value_c}"),
    ToolsColumnPreviewCappedAt300Full => ("열 미리보기는 300개까지 표시합니다. 수집 범위 전체는 스냅샷 JSON으로 확인할 수 있습니다.", "Column preview capped at 300. Full bounded capture remains available in snapshot JSON."),
    ToolsPrimaryKey => ("기본 키 · {value_a}", "Primary key · {value_a}"),
    ToolsForeignKey => ("외래 키 · {value_a}", "Foreign key · {value_a}"),
    ToolsDELETEUPDATEDeferrable => ("{value_a} → {value_b} ({value_c}) · DELETE {value_d} · UPDATE {value_e} · 지연 가능 {value_f}", "{value_a} → {value_b} ({value_c}) · DELETE {value_d} · UPDATE {value_e} · deferrable {value_f}"),
    ToolsIndex => ("인덱스 {value_a} · {value_b}{value_c}", "Index {value_a} · {value_b}{value_c}"),
    ToolsColumnsExpressionsINCLUDEPredicate => ("열: {value_a} · 표현식: {value_b} · INCLUDE: {value_c} · 조건식: {value_d}", "Columns: {value_a} · expressions: {value_b} · INCLUDE: {value_c} · predicate: {value_d}"),
    ToolsConstraintOwnedUnsupportedMetadata => ("제약 조건 소유: {value_a} · 미지원 메타데이터: {value_b}", "Constraint owned: {value_a} · unsupported metadata: {value_b}"),
    ToolsAdvancedDefinitionShownFromCapturedMetadata => ("고급 정의는 수집한 메타데이터를 표시합니다. 기본 초안 폼은 표현식·조건식·INCLUDE·opclass·collation 재생성을 지원하지 않습니다.", "Advanced definition shown from captured metadata; expression/predicate/INCLUDE/opclass/collation recreation is unsupported by this basic draft form."),
    ToolsDraftDROPINDEXDestructive => ("DROP INDEX 초안 · 파괴 작업", "Draft DROP INDEX · destructive"),
    ToolsForeignKeyIndexDetailPreviewCapped => ("외래 키·인덱스 상세 미리보기는 각각 200개까지입니다. 전체 메타데이터는 스냅샷 JSON으로 확인하세요.", "Foreign key/index detail preview capped at 200 each; complete captured metadata remains in snapshot JSON."),
    ToolsCreateIndexDraftOrdinaryBtreeOnly => ("인덱스 생성 초안 · 일반 btree 전용", "Create index draft · ordinary btree only"),
    ToolsNewIndexName => ("새 인덱스 이름", "New index name"),
    ToolsExactCapturedColumnNamesForIndex => ("인덱스의 정확한 수집 열 이름, 쉼표로 구분", "Exact captured column names for index, comma separated"),
    ToolsUniqueYes => ("고유: 예", "Unique: yes"),
    ToolsUniqueNo => ("고유: 아니요", "Unique: no"),
    ToolsGenerateCREATEINDEXDraft => ("CREATE INDEX 초안 생성", "Generate CREATE INDEX draft"),
    ToolsCaptured => ("수집한 {value_a} {value_b}", "Captured {value_a} {value_b}"),
    ToolsMaterializedView2 => ("구체화된 뷰", "materialized view"),
    ToolsMaterializedViewRefreshStorageDropCreate => ("기본 뷰 폼은 구체화된 뷰의 갱신·저장·삭제·생성을 지원하지 않습니다.", "Materialized view refresh/storage/drop/create is unsupported in this basic view form."),
    ToolsDefinitionCapturedFromPostgreSQLViewOwner => ("PostgreSQL에서 수집한 정의입니다. 뷰 소유자·ACL·reloptions/보안 설정·트리거는 기본 폼 범위 밖입니다.", "Definition captured from PostgreSQL. View owner, ACL, reloptions/security settings and triggers are outside this basic form."),
    ToolsOrdinaryViewSQLDrafts => ("일반 뷰 SQL 초안", "Ordinary view SQL drafts"),
    ToolsSchemaAndViewNameAreQuoted => ("스키마·뷰 이름은 식별자로 인용합니다. 정의는 단일 쿼리여야 합니다. 실행 전에 함수와 의존성을 검토하세요. 기존 일반 뷰는 파괴 작업인 교체를 명시해야 합니다.", "Schema and view name are quoted as identifiers. Definition must parse as one query; review functions and dependencies before running. Existing ordinary views require explicit destructive Replace."),
    ToolsViewSchemaInCapturedScope => ("수집 범위 내 뷰 스키마", "View schema in captured scope"),
    ToolsViewName => ("뷰 이름", "View name"),
    ToolsViewDefinitionExactlyOneQuery => ("뷰 정의, 단일 쿼리", "View definition, exactly one query"),
    ToolsDraftCREATEVIEW => ("CREATE VIEW 초안", "Draft CREATE VIEW"),
    ToolsDraftREPLACEVIEWDestructive => ("REPLACE VIEW 초안 · 파괴 작업", "Draft REPLACE VIEW · destructive"),
    ToolsDraftDROPVIEWDestructive => ("DROP VIEW 초안 · 파괴 작업", "Draft DROP VIEW · destructive"),
    ToolsDESTRUCTIVESQLDraftReviewRequired => ("파괴 작업 SQL 초안 · 검토 필요", "DESTRUCTIVE SQL draft · review required"),
    ToolsSQLDraftReviewRequired => ("SQL 초안 · 검토 필요", "SQL draft · review required"),
    ToolsThisDraftBelongsToTheCaptured => ("이 초안은 위 수집 대상 기준입니다. 폼을 수정해도 바뀌지 않으므로 변경을 반영하려면 다시 생성하세요. SQL 실행은 없었습니다.", "This draft belongs to the captured target above. It is unchanged by later form edits; regenerate to reflect edits. No SQL has been executed."),
    ToolsCopySQLDraft => ("SQL 초안 복사", "Copy SQL draft"),
    ToolsSQLDraftCopiedNothingExecuted => ("SQL 초안 복사됨 · 실행 없음", "SQL draft copied · nothing executed"),
    ToolsSaveSQLDraft => ("SQL 초안 저장…", "Save SQL draft…"),
    ToolsOpenDraftInNewConsole => ("새 콘솔에서 초안 열기", "Open draft in new console"),
    ToolsIdentifiersRequire163UTF8 => ("식별자는 제어 문자 없이 UTF-8 기준 1–63바이트여야 합니다", "Identifiers require 1–63 UTF-8 bytes and no control characters"),
    ToolsObjectSQLDraftExceeds64KB => ("객체 SQL 초안이 64 KB 제한을 초과했습니다", "Object SQL draft exceeds 64 KB"),
    ToolsObjectDraftMustParseAsPostgreSQL => ("객체 초안은 PostgreSQL SQL로 파싱 가능해야 합니다", "Object draft must parse as PostgreSQL SQL"),
    ToolsObjectDraftRequiresExactlyOneStatement => ("객체 초안은 단일 문장이어야 합니다", "Object draft requires exactly one statement"),
    ToolsUnsupportedObjectDraftStatement => ("지원하지 않는 객체 초안 문장입니다", "Unsupported object draft statement"),
    ToolsTableNoLongerExistsInThe => ("수집한 스냅샷에 테이블이 없습니다. 테이블을 다시 선택하세요", "Table no longer exists in the captured snapshot; select a table again"),
    ToolsViewNoLongerExistsInThe => ("수집한 스냅샷에 뷰가 없습니다. 뷰를 다시 선택하세요", "View no longer exists in the captured snapshot; select a view again"),
    ToolsIndexNameConflictsWithARelation => ("인덱스 이름이 수집한 스키마의 관계 객체와 충돌합니다", "Index name conflicts with a relation in the captured schema"),
    ToolsChooseDistinctExactCapturedColumnNames => ("수집한 열 이름을 중복 없이 쉼표로 구분해 지정하세요. 표현식·조건식·INCLUDE는 미지원입니다", "Choose distinct exact captured column names, comma separated; expressions/predicates/INCLUDE are unsupported"),
    ToolsCreateOrdinaryBtreeIndexReviewLocking => ("일반 btree 인덱스 생성 · 잠금·고유성 검증·이름 충돌을 검토하세요", "Create ordinary btree index · review locking, uniqueness validation and name conflicts"),
    ToolsConstraintOwnedIndexCannotBeDropped => ("제약 조건 소유 인덱스는 단독 삭제할 수 없습니다. 제약 조건을 별도로 관리하세요", "Constraint-owned index cannot be dropped independently; manage its constraint separately"),
    ToolsDESTRUCTIVERemoveIndexRESTRICTPreventsDependent => ("파괴 작업 · 인덱스 삭제 · RESTRICT로 의존 객체 연쇄 삭제 방지", "DESTRUCTIVE · remove index · RESTRICT prevents dependent-object cascade"),
    ToolsViewSchemaMustBelongToThe => ("뷰 스키마는 지정한 수집 범위 안에 있어야 합니다", "View schema must belong to the explicit captured scope"),
    ToolsViewNameConflictsWithACaptured => ("뷰 이름이 수집한 관계 객체와 충돌합니다. 새 이름을 지정하거나 일반 뷰 교체를 선택하세요", "View name conflicts with a captured relation; choose a new name or explicit Replace for an ordinary view"),
    ToolsMaterializedViewManagementIsUnsupportedNo => ("구체화된 뷰 관리는 미지원입니다. 일반 뷰 초안을 생성하지 않았습니다", "Materialized view management is unsupported; no ordinary view draft generated"),
    ToolsCapturedViewAlreadyExistsChooseExplicit => ("수집한 뷰가 이미 있습니다. 뷰 교체 초안을 명시적으로 선택하세요", "Captured view already exists; choose explicit Replace view draft"),
    ToolsReplaceRequiresAnExistingOrdinaryView => ("교체하려면 수집한 스냅샷에 기존 일반 뷰가 있어야 합니다", "Replace requires an existing ordinary view in the captured snapshot"),
    ToolsViewDefinitionMustBeExactlyOne => ("뷰 정의는 단일 SELECT/쿼리 문장이어야 합니다", "View definition must be exactly one SELECT/query statement"),
    ToolsDESTRUCTIVEReplaceOrdinaryViewDefinitionReview => ("파괴 작업 · 일반 뷰 정의 교체 · 열·의존성 호환성을 검토하세요", "DESTRUCTIVE · replace ordinary view definition · review column/dependency compatibility"),
    ToolsCreateOrdinaryViewQueryFunctionsAnd => ("일반 뷰 생성 · 쿼리 함수와 의존성을 검토하세요", "Create ordinary view · query functions and dependencies require review"),
    ToolsMaterializedViewManagementIsUnsupported => ("구체화된 뷰 관리는 미지원입니다", "Materialized view management is unsupported"),
    ToolsDESTRUCTIVERemoveOrdinaryViewRESTRICTPrevents => ("파괴 작업 · 일반 뷰 삭제 · RESTRICT로 의존 객체 연쇄 삭제 방지", "DESTRUCTIVE · remove ordinary view · RESTRICT prevents dependent-object cascade"),
    ToolsSchema => ("스키마", "Schema"),
    ToolsObjects => ("객체", "Objects"),
    ToolsMigration => ("마이그레이션", "Migration"),
    ToolsRelationships => ("관계", "Relationships"),
    ToolsEXPLAIN => ("EXPLAIN", "EXPLAIN"),
    ToolsDataCompare => ("데이터 비교", "Data compare"),
    ToolsChoose132DistinctUserSchemas => ("서로 다른 사용자 스키마 1–32개를 쉼표로 구분해 지정하세요. 시스템 스키마는 제외합니다", "Choose 1–32 distinct user schemas, comma separated (system schemas excluded)"),
    ToolsEnterDistinctUnambiguousColumnNamesSeparated => ("열 이름을 중복·모호함 없이 쉼표로 구분해 입력하세요. 이름은 정확히 일치해야 합니다", "Enter distinct, unambiguous column names separated by commas; names must match exactly"),
    ToolsOperationCancelled => ("작업 취소됨", "Operation cancelled"),
    ToolsCouldNotStartReadOnlyTools => ("읽기 전용 도구 런타임을 시작할 수 없습니다", "Could not start read-only tools runtime"),
    ToolsOperationCancelledDedicatedReadOnlySession => ("작업 취소됨 · 전용 읽기 전용 세션 종료됨", "Operation cancelled; dedicated read-only session retired"),
    ToolsToolsOperationTimedOutDedicatedRead => ("도구 작업 제한 시간 초과 · 전용 읽기 전용 세션 종료됨", "Tools operation timed out; dedicated read-only session retired"),
    ToolsCouldNotOpenSelectedInput => ("선택한 입력 파일을 열 수 없습니다", "Could not open selected input"),
    ToolsCouldNotReadSelectedInput => ("선택한 입력 파일을 읽을 수 없습니다", "Could not read selected input"),
    ToolsSchemaInputExceeds16MBLimit => ("스키마 입력이 16 MB 제한을 초과했습니다", "Schema input exceeds 16 MB limit"),
    ToolsSchemaInputMustBeUTF8 => ("스키마 입력은 UTF-8이어야 합니다", "Schema input must be UTF-8"),
    ToolsNoFileSelected => ("선택한 파일 없음", "No file selected"),
    ToolsUnnamed => ("이름 없음", "unnamed"),
    ToolsNone => ("없음", "none"),
    ToolsToolsOperationFailedError => ("도구 작업 실패: {error}", "Tools operation failed: {error}"),
    ToolsGPUIBuiltInComponentTextMay => ("GPUI 기본 구성 요소의 문구는 영어로 남을 수 있습니다.", "GPUI built-in component text may remain English."),
    ToolsYes => ("예", "yes"),
    ToolsNo => ("아니요", "no"),
    ToolsDataInsert => ("추가", "INSERT"),
    ToolsDataDelete => ("삭제", "DELETE"),
    ToolsDataChange => ("변경", "CHANGE"),
    Quit => ("종료", "Quit"),
    SaveAs => ("다른 이름으로 저장", "Save as"),
    RunScript => ("스크립트 실행", "Run script"),
    RunCurrent => ("현재 문장 실행", "Run current statement"),
    ConnectAndRun => ("연결 후 실행", "Connect and run"),
    Commit => ("커밋", "Commit"),
    Rollback => ("롤백", "Rollback"),
    AutoCommit => ("자동 커밋", "Auto commit"),
    ManualCommit => ("수동 커밋", "Manual commit"),
    FormatSql => ("SQL 서식 정리", "Format SQL"),
    CompleteSql => ("SQL 자동완성", "Complete SQL"),
    InspectSql => ("SQL 검사", "Inspect SQL"),
    Structure => ("구조", "Structure"),
    OpenData => ("데이터 열기", "Open data"),
    NewSqlDraft => ("새 SQL 초안", "New SQL draft"),
    CopyFields => ("필드 복사", "Copy fields"),
    DisconnectConsole => ("콘솔 연결 해제", "Disconnect console"),
    ConnectConsole => ("콘솔 연결", "Connect console"),
    SessionPassword => ("이 세션의 비밀번호", "Session password"),
    ApplyDatabase => ("작업 데이터베이스 변경", "Apply database"),
    EnterDatabaseName => ("데이터베이스 이름 입력", "Enter database name"),
    RecoveryOn => ("편집 내용 복구 켜짐", "Editor recovery on"),
    RecoveryOff => ("편집 내용 복구 꺼짐", "Editor recovery off"),
    SqlRecovery => ("SQL 편집 내용 복구", "SQL editor recovery"),
    History => ("기록", "History"),
    Advanced => ("고급", "Advanced"),
    DatabaseTools => ("데이터베이스 도구", "Database tools"),
    ColumnName => ("열 이름", "Column name"),
    DataType => ("자료형", "Data type"),
    Nullable => ("NULL 허용", "Nullable"),
    Key => ("키", "Key"),
    DefaultValue => ("기본값", "Default value"),
    Reload => ("다시 불러오기", "Reload"),
    PreviousPage => ("이전 페이지", "Previous page"),
    NextPage => ("다음 페이지", "Next page"),
    Find => ("찾기", "Find"),
    FindLoadedPage => ("현재 불러온 페이지에서 찾기", "Find in this loaded page"),
    NewRow => ("새 행", "New row"),
    InsertNewRow => ("새 행 입력", "Insert new row"),
    EditRowOnPage => ("현재 페이지의 {row}행 편집", "Edit row {row} on this page"),
    SelectRowToEdit => ("편집할 행 선택", "Select a row to edit"),
    ReadOnlyReason => ("읽기 전용: {reason}", "Read-only: {reason}"),
    GeneratedIdentity => ("생성 열/식별 열", "Generated/identity"),
    DefaultExpression => ("기본값: {value}", "Default: {value}"),
    GeneratedExpression => ("생성식: {value}", "Generated: {value}"),
    RowValueFor => ("{column} 열의 행 값", "Row value for {column}"),
    StageInsert => ("삽입 변경 보관", "Stage insert"),
    StageUpdate => ("수정 변경 보관", "Stage update"),
    StageDelete => ("삭제 변경 보관", "Stage delete"),
    StagedEdits => ("보관한 변경 {count}개", "{count} staged edits"),
    PreviewTransaction => ("트랜잭션 미리보기", "Preview transaction"),
    DiscardStaged => ("보관한 변경 버리기", "Discard staged"),
    ApplyPreview => ("미리본 변경 적용", "Apply preview"),
    TableViewerHint => ("읽기 전용 테이블 보기 · 행 편집 검증 대기 중 · 각 페이지는 제한된 SELECT를 새로 실행합니다", "Read-only table viewer · row editing validation pending · each page is a fresh bounded SELECT"),
    TableApplyDisabled => ("행 편집 검증 대기 중 · 적용 비활성화", "Row editing validation pending · Apply disabled"),
    TableApplyDisabledError => ("행 편집 검증이 완료되지 않아 이 버전에서는 적용할 수 없습니다.", "Row editing validation pending. Apply is disabled in this release."),
    CapturingTableMetadata => ("확인된 테이블 메타데이터를 불러오는 중…", "Capturing verified table metadata…"),
    CapturingLiveMetadata => ("실제 메타데이터 전체를 불러오는 중…", "Capturing complete live metadata…"),
    RevalidatingTable => ("적용 전 테이블 식별자와 카탈로그를 다시 확인하는 중…", "Revalidating table identity and catalog before Apply…"),
    TableMetadataChanged => ("테이블 메타데이터 또는 식별자가 바뀌었습니다. 다시 불러온 뒤 변경을 보관하세요. 적용된 내용은 없습니다.", "Table metadata/identity changed. Reload and stage edits again; nothing was applied."),
    ApplyingTableBatch => ("명시적 매개변수 배치를 적용하는 중…", "Applying explicit parameterized batch…"),
    ReadingTablePage => ("현재 연결에서 제한된 페이지를 읽는 중…", "Reading a bounded page on the existing connection…"),
    TableCatalogFallback => ("카탈로그 보기를 사용할 수 없습니다: {error}. 읽기 전용 SELECT로 엽니다.", "Catalog viewer unavailable: {error}. Opening a read-only SELECT instead."),
    TableControlStatus => ("트랜잭션 제어 {outcome} · 현재 상태 {transaction}. 행을 다시 불러오세요. 보관한 변경을 반복 실행하지 않습니다.", "Transaction control {outcome} · current wire {transaction}. Reload rows; staged edits will not be replayed."),
    TableApplyStatus => ("{outcome} · 트랜잭션 {transaction} · {acknowledgement}. 추가 편집 전에 다시 불러오세요. 배치는 반복 실행하지 않습니다.", "{outcome} · transaction {transaction} · {acknowledgement}. Reload before any further edits; no batch is replayed."),
    CommitAcknowledged => ("커밋 확인됨", "Commit acknowledged"),
    SubmittedNotCommitted => ("열린 트랜잭션에 제출됨 · 커밋되지 않음", "Submitted into open transaction; not committed"),
    NoBatchAcknowledgement => ("배치 성공 응답 없음", "No successful batch acknowledgement"),
    TableReadReloadRequired => ("행 읽기 {outcome} · 다시 불러와야 함", "Row read {outcome}; reload required"),
    UnexpectedTableProjection => ("조회 결과의 열 구성이 테이블과 다릅니다", "Read returned an unexpected table projection"),
    LoadedTablePage => ("서버 페이지 {page} · 불러온 행 {rows}개 · 전체 행 수 알 수 없음", "Server page {page} · {rows} loaded rows · table total unknown"),
    MetadataUnavailable => ("메타데이터를 사용할 수 없음", "Metadata unavailable"),
    TableReadOnly => ("읽기 전용 테이블", "Table is read-only"),
    SelectRowFirst => ("먼저 행을 선택하세요", "Select a row first"),
    OriginalRowUnavailable => ("원본 행을 사용할 수 없음", "Original row unavailable"),
    GeneratedColumnReadOnly => ("생성 열/식별 열 {column}은 읽기 전용입니다", "Generated/identity column {column} is read-only"),
    DiscardRowFormTitle => ("보관하지 않은 행 입력 변경을 버리시겠습니까?", "Discard unstaged row form changes?"),
    DiscardRowFormHint => ("현재 행의 변경을 보관하면 로컬 트랜잭션 미리보기에 유지됩니다.", "Stage the current row to keep it in the local transaction preview."),
    KeepForm => ("입력 유지", "Keep form"),
    DiscardFormChanges => ("입력 변경 버리기", "Discard form changes"),
    NewRowFormStatus => ("새 행 입력 · 변경 보관, 미리보기, 적용 전까지 로컬에만 유지됩니다", "New row form · changes remain local until Stage, Preview and Apply"),
    StagedRowEditsStatus => ("행 변경 {count}개 보관됨 · SQL 실행 없음", "{count} staged row edits · no SQL executed"),
    StageFormBeforePreview => ("배치를 미리보기 전에 현재 행의 입력 변경을 보관하거나 버리세요.", "Stage the current row form or discard its changes before previewing the batch."),
    TransactionPreviewReady => ("트랜잭션 미리보기 준비됨. 적용은 별도로 명시적으로 제출해야 합니다.", "Transaction preview ready. Apply is a separate explicit submit gesture."),
    DiscardReloadTitle => ("로컬 행 변경을 버리고 다시 불러오시겠습니까?", "Discard local row edits and reload?"),
    DiscardReloadHint => ("보관한 변경과 입력 중인 변경을 모두 버립니다. 작업은 자동으로 반복 실행하지 않습니다.", "Both staged edits and unstaged form changes will be discarded. Nothing is replayed automatically."),
    KeepEdits => ("변경 유지", "Keep edits"),
    DiscardAndReload => ("버리고 다시 불러오기", "Discard and reload"),
    LocalBuffersDiscarded => ("로컬 변경을 버렸습니다. SQL은 실행하지 않았으며 기존 실행 및 트랜잭션 결과는 바뀌지 않습니다.", "Local buffers discarded. Discard submits no SQL; prior execution/transaction outcomes are unchanged."),
    NoMatchLoadedPage => ("현재 페이지에서 일치하는 값 없음 · 로컬 검색만 수행", "No match in the currently loaded page (client search only)"),
    BoundParameters => ("바인딩 매개변수", "Bound parameters"),
    GuardedTransactionHint => ("명시적으로 검증하는 트랜잭션 · DEFAULT는 데이터베이스에서 평가 · 원본 셀 조건 및 문장당 정확히 1행 영향 확인 · 롤백 후에도 시퀀스는 증가할 수 있음 · 값 비교로 ABA 변경은 감지하지 못함", "Explicit guarded transaction · DEFAULT is evaluated by the database · original-cell predicates · exactly 1 affected row per statement · server defaults may advance sequences even after rollback · value checks do not detect ABA changes"),
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
