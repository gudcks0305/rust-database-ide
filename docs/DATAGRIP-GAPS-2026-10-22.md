# DataGrip 2026.2 기능 gap 검토

기준일: 2026-10-22. DataGrip 쪽 기능 근거는 JetBrains의 공식 2026.2 도움말만 사용했다. 코드 경로는 현재 작업 트리의 구현 범위를 설명한다. 이 문서는 native QA나 테스트 통과를 주장하지 않으며, DataGrip parity 또는 성능 비교를 뜻하지 않는다.

## 현재 추가된 기능의 경계

| 기능 | 코드 근거와 현재 범위 | 남은 경계 |
|---|---|---|
| 값/행 보기, JSON 형식 보기 | [`value_selection.rs`](../crates/dbide-ui-spike/src/value_selection.rs)에서 보관된 결과 페이지를 불변 스냅샷으로 만들고 [`value_inspector.rs`](../crates/dbide-ui-spike/src/value_inspector.rs)에서 선택 셀·행 필드를 보여준다. JSON 보기와 원문 복사를 분리한다. | 드라이버의 DB 자료형을 표시하지 않고 text representation만 사용한다. 화면 표시는 UTF-8 기준 최대 64 KiB이며, 원문 복사도 보관된 값에 한정된다. 행은 최대 256 fields 및 기본 1 MiB 캡을 따른다. 편집 기능이 아니다. |
| 선택 범위 복사 | [`result_tools.rs`](../crates/dbide-ui-spike/src/result_tools.rs), [`value_selection.rs`](../crates/dbide-ui-spike/src/value_selection.rs). 행·열·사용자 지정 셀 범위를 TSV로 복사하며 NULL은 `\N`으로 표시한다. | 현재 로드된 결과 페이지 안에서만 동작한다. 기본 한도는 10,000 cells 및 1 MiB이며 페이지/미로드 결과를 넘지 않는다. |
| SQL 객체 찾기 | [`object_finder.rs`](../crates/dbide-ui-spike/src/object_finder.rs), [`search.rs`](../crates/dbide-catalog-store/src/search.rs), [`discovery_tools.rs`](../crates/dbide-ui-spike/src/discovery_tools.rs). 현재 프로필에 대응하는 저장 catalog를 읽기 전용으로 검색하고 선택 결과를 navigator에서 다시 연다. | 매 검색 최대 100개 결과, 64 shards, 4 MiB. 결과가 부분집합이면 UI에 표시하며 continuation/page cursor는 없다. 저장된 historical metadata라 객체가 지금 존재한다는 보장은 없다. DB query를 검색용으로 실행하지 않는다. |
| Query history | [`history_panel.rs`](../crates/dbide-ui-spike/src/history_panel.rs), [`history_model.rs`](../crates/dbide-ui-spike/src/history_model.rs). SQL/profile/database 텍스트 검색, profile/outcome 필터, 최신순 탐색이 있고 선택 기록은 원 SQL과 캡처된 대상으로 새 초안에서 연다. | Enter/열기 동작은 SQL을 실행하지 않는다. 즐겨찾기와 saved-query 폴더/분류는 없다. history 저장 자체는 사용자 opt-in 설정에 따름. |
| 선택 SQL 실행 | [`selection_execution.rs`](../crates/dbide-ui-spike/src/selection_execution.rs), [`discovery_tools.rs`](../crates/dbide-ui-spike/src/discovery_tools.rs). 현재 editor 선택의 정확한 UTF-8 범위만 제출하고, 공백 선택이나 잘못된 범위는 거부한다. | caret의 현재 문장 자동 감지는 이 동작에 포함되지 않는다. 선택이 없을 때 주변 문장/script로 fallback하지 않는다. 파라미터 입력 prompt는 별도 기능이다. |

## DataGrip 공식 문서 대조와 구현 우선순위

| 우선 | 공식 DataGrip 동작 | 현 코드에서 확인된 경계 | 작은 다음 단위 |
|---|---|---|---|
| P1 | [Run queries](https://www.jetbrains.com/help/datagrip/run-a-query.html): 이름/위치 파라미터 정의와 실행 전 입력, 선택 코드 조각만 실행, 쿼리 기록 검색 | 선택 SQL은 구현 경로에 있음. 파라미터 prompt는 [CURRENT-STATUS.md](CURRENT-STATUS.md) 기준 pending이며, bridge의 내부 parameterized DML contract가 일반 SQL 사용자 파라미터 UI를 의미하지 않는다. | 문장 literal/comment를 존중해 marker 위치를 찾고, 반복 named marker를 한 입력으로 묶는 prompt를 붙인다. DB별 실제 bind 경로가 확인되지 않으면 지원 범위를 제한한다. |
| P1 | [Metadata and introspection](https://www.jetbrains.com/help/datagrip/introspection.html): 표시 schema 선택이 실제 introspection scope를 결정하며, full/incremental/fragment 범위 구분 | [navigator](../crates/dbide-ui-spike/src/navigator.rs)와 catalog cache는 요청 범위를 제한하고 lazy branch를 불러온다. 다만 사용자가 선택하고 저장하는 schema scope 및 변경분 refresh 설정은 확인되지 않았다. Object finder는 기존 cache의 제한 검색이다. | explorer에서 schema 선택을 보존하고 선택 scope만 새로 불러온다. UI에서 표시 filter와 metadata 조회 scope를 별도로 설명한다. |
| P1 | [Foreign keys](https://www.jetbrains.com/help/datagrip/foreign-keys.html): FK 기반 JOIN 제안 및 data editor의 Related Rows 이동 | FK metadata는 catalog와 object inspector에서 표시되지만, 현재 table data viewer에서 FK cell을 눌러 관련 행을 여는 동작은 확인되지 않았다. | 선택 셀의 FK metadata를 기준으로 대상 relation과 key filter를 구성하고, 조회 대상과 방향을 UI에 보여준다. |
| P1 | [Configure connection](https://www.jetbrains.com/help/datagrip/configuring-database-connections.html): startup script, keep-alive query, auto-disconnect 옵션 | [ProfileForm](../crates/dbide-ui-spike/src/profile_form.rs)은 기본 연결 정보, TLS, read-only, timeout 등을 다룬다. startup script와 keep-alive는 현재 상태 문서에서 미구현으로 기록됐다. | 먼저 startup script를 opt-in으로 추가하고, 연결 생성 시 한 번만 실행해 결과/오류를 별도로 표시한다. keep-alive/auto-disconnect는 세션 상태 계약을 정한 뒤 추가한다. |
| P2 | [Manage sessions](https://www.jetbrains.com/help/datagrip/managing-connection-sessions.html): shared, shared+introspection, dedicated 모드 | 현재 콘솔은 전용 세션 경로를 갖고 통합 검증 기록이 있다. 사용자가 data source별로 session mode를 선택하는 옵션은 확인되지 않았다. | shared/dedicated 두 선택부터 정책으로 명시한다. metadata가 작업 세션을 막지 않는지 별도 acceptance가 필요하다. |
| P2 | [View data](https://www.jetbrains.com/help/datagrip/tables-view-data.html): Table/Tree/Text/Transpose 모드와 value editor | 값 inspector와 행 보기 구현은 있으나, DB type-aware editor, 표 변환 모드, Tree/Text 전환은 확인되지 않았다. | 우선 JSON object/array Tree 보기와 읽기 전용 Text mode를 추가하고, 원본 값 복사와 구분한다. |
| P2 | [Explore data in data editor](https://www.jetbrains.com/help/datagrip/explore-data-in-data-editor.html): 선택 cells 집계와 FK 관련 행 탐색 | 이번 범위의 결과 선택은 TSV 복사 목적이며 숫자 집계 패널은 확인되지 않았다. | 현재 보관된 선택 셀만 대상으로 count/sum/avg를 계산하고 표본 범위를 함께 표시한다. |
| P2 | [Live templates](https://www.jetbrains.com/help/datagrip/using-live-templates.html): 약어 확장, 편집 가능한 템플릿 변수, SQL dialect별 문맥 | repository에는 declarative snippet backend가 있으나 기본 SQL 편집기의 약어 확장/변수 이동과 연결됐는지 확인되지 않았다. | `sel`/`ins` 같은 기본 템플릿 두 개와 Tab으로 변수 이동하는 단순 경로부터 붙인다. |
| P2 | [Data sources and drivers settings](https://www.jetbrains.com/help/datagrip/data-sources-and-drivers-dialog.html): object filter, schema pattern, auto sync, introspection level 설정 | 앱의 navigator 검색은 로드된 객체 이름 검색이다. 이는 DataGrip의 표시 filter나 실제 metadata scope selection과 동등하지 않다. PG table filter/sort/page controls는 [table_editor.rs](../crates/dbide-ui-spike/src/table_editor.rs)에 있으나 generic query 결과에는 별도다. | schema scope와 object display filter를 구분한 뒤, PG와 MySQL 우선으로 schema별 introspection 상세도를 정의한다. |

## 기능 행렬 보정

- **F06:** 기존 “증분/lazy 남음” 문구에서 lazy 구현을 누락했다. 현재 lazy scoped metadata와 private cache가 있으므로 상태를 partial로 유지하되, 사용자 선택 scope와 incremental refresh가 남았다고 적는다.
- **F07:** planned만으로 적혀 있던 상태를 partial로 보정한다. lazy explorer, 로드된 객체 검색, 캐시 finder, 일부 객체 detail/DDL 경로가 있다. 검색/표시 filter와 모든 객체 DDL/source workflow는 완성되지 않았다.
- **F18:** SQL/profile/database 검색과 profile/outcome filter를 제공하므로 “검색 남음”은 낡은 설명이다. favorites와 saved-query 정리는 남는다.
- **F22:** PG table viewer의 typed AND filter, server-side sort, page sizes, loaded-page Find가 있으므로 planned가 아니다. generic query result의 server-side filtering/sorting 및 전체 결과 검색은 남는다.

## 미완료 우선 후보

현재 코드 기준으로 다음 일상 작업 gap이 남는다: 일반 SQL parameter prompt/binding, data-grid FK navigation, 사용자 선택 introspection scope와 schema별 introspection level, startup script/keep-alive, 사용자 선택 shared session mode, type-aware value editing, aggregate view, SQL live templates, generic result server filtering/sorting. 각 항목은 DB와 동작 단위별로 acceptance를 추가해야 하며, 공식 도움말과 이름이 같다는 이유만으로 지원 완료로 계산하지 않는다.
