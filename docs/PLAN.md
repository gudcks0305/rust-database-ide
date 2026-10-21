# 구현 계획

기준일: 2026-10-21. 상태: **M0 종료 조건 미충족, M1 기본 흐름 통합 중; 최종 제품 미완성.** 최신 판정은 [M0/M1 체크리스트](M0-M1-ACCEPTANCE.md), 초기 실험 기록은 [M0-RESULTS.md](M0-RESULTS.md).

우선순위 교정: 연결 → database/schema/table 탐색 → 필드·키 확인 → SQL 콘솔 → 결과 흐름을 먼저 검증한다. 한·영 UI와 Preferences를 포함한 기본 작업 흐름을 보강한 뒤 M3–M6 고급 기능도 계속 구현·검증한다. 이미 작성한 후반 컴포넌트는 보존하되 단계 완료로 집계하지 않는다. M2 행 Apply는 알려진 namespace 재바인딩 결함이 해결될 때까지 비활성화한다.

## 목표와 범위

DataGrip에서 일상적으로 사용하는 기능 대부분을 Rust 제품으로 구현한다.
기능 범위는 [FEATURE-MATRIX.md](FEATURE-MATRIX.md)의 F01–F42로 추적한다.
초기 PostgreSQL 수직 구현은 첫 검증 단위이며 최종 범위 축소가 아니다.
DB 엔진 수만으로 완성도를 표현하지 않고 연결/실행/타입/편집/메타데이터/SQL 분석을 각각 판정한다.

## 조사에서 나온 결정

| 결정 | 근거 | 처리 |
|---|---|---|
| 새 Rust 제품으로 독립 구현 | 패키지가 JVM 플랫폼·DB·SQL·dialect 모듈로 나뉨 | Java 파일을 기계 번역하는 방식으로 계획하지 않음 |
| 세션과 실행 상태를 UI에서 분리 | connectivity 패키지의 session/transaction/cancel 후보 + 공식 세션 계약 | Session actor가 연결과 트랜잭션 소유 |
| 대용량 결과를 스트림으로 처리 | 성능 목표, 결과/LOB/다중 결과 기능 요구 | 양축 가상화, byte budget, bounded queue, spill |
| SQL 구문과 의미 모델 분리 | SQL completion/refactoring 별도 패키지 + 공식 기능 | parser만 붙이고 자동완성 완료로 표시하지 않음 |
| DB별 어댑터/기능 협상 | dialect 모듈 다수, JDBC 의존 구조 | native Rust driver 우선; unsupported 상태 명시 |
| GPUI 우선 스파이크 | Rust editor형 UI 후보; 문서상 pre-1.0 | 한글 IME·접근성·그리드 통과 후 채택 |
| 성능 목표는 측정 전 가설 | 현재 정적 분석만 수행 | 숫자 개선율 주장 금지; 기준선 먼저 측정 |

모듈 이름만으로 내부 알고리즘이나 느린 원인을 확정하지 않는다. 증거 범위는 [REA-FINDINGS.md](REA-FINDINGS.md).

## 단계와 종료 조건

### M0 — 위험 검증 / 구조 확정

작업:

1. 동일 기기에서 DataGrip 기준선 수집. 시작/입력/스크롤/쿼리/취소/스키마 탐색을 분리.
2. GPUI로 SQL editor + 가상 grid 스파이크. Iced를 대체 후보, egui를 필요 시 성능 대조군으로 사용.
3. PostgreSQL 드라이버 후보로 임의 SQL, 다중 결과, COPY, notices, cancel, temp table, transaction 실험.
4. SQLite 실행/중단/락 오류/오프라인 fixture 검증. 단일 파일 경합과 worker 전용 연결 확인.
5. SQL 문장 경계/오류 복구 파서 후보 비교. 한국어·중첩 CTE·dollar quote·미완성 SQL fixture 작성.
6. 도구체인/MSRV 확인 후 프로젝트에 Rust 버전 고정. 로컬 관찰값은 rustc/cargo 1.89.0이며 최신 UI 라이브러리와 호환 미확인.
7. 후보 dependency의 정확한 버전/commit/license 기록. GPUI와 Zed editor 재사용은 별개 결정.

통과:

- 한글 조합/undo/selection/VoiceOver, 100만 논리행 × 200열 양축 가상화 통과.
- cancel 후 terminal 상태 확인 또는 연결 폐기; 다음 쿼리를 잘못 취소하지 않음.
- temp table과 수동 transaction이 같은 세션에서 유지됨.
- 임의 SQL 실행이 parser 성공 여부에 묶이지 않음.
- [성능 측정 프로토콜](BENCHMARKS.md)과 첫 결과 저장. 실패 시 원인별 재선정.

산출물: Cargo workspace, UI/driver ADR, 재현 가능한 fixture/측정기, 좁은 spike 코드.

### M1 — PostgreSQL end-to-end 사용 가능

연결 프로필/Keychain → catalog tree → SQL 탭 → 실행·취소 → 결과 grid → history.
TLS 검증, 세션 고정, Auto/Manual transaction, 다중 result/update count/error 표현.
기본 SQL highlight/format/current-statement 실행, connection 선택/상태/재연결 UI.

통과: 로컬 fixture DB에서 이 흐름을 GUI 통합 테스트로 완료.
오류/취소/네트워크 단절 후 상태가 설명 가능하고 재실행이나 commit이 자동으로 일어나지 않음.
DataGrip 대비 성능 비교는 이 단계부터 같은 실제 작업으로 수행.

### M2 — 일상 업무 기능 완성

SQLite + MySQL/MariaDB 어댑터, 데이터 CRUD/NULL/default/LOB/JSON,
PK 기반 optimistic edit, staged/submitted/committed 상태, 충돌 표시,
CSV/JSON import/export, 필터/정렬/페이지/검색, SSH, 스키마 증분 캐시,
객체 DDL 보기/편집, 기본 explain, 프로젝트/탭 복구.

통과: PG/SQLite/MySQL 각 dialect의 타입·편집·트랜잭션·취소 계약 테스트.
긴 export 중 UI 반응 유지, RAM 한도 준수, 중단된 import의 처리 범위 표시.
실무 사용 가능한 beta 경계. 아직 최종 기능 동등성 아님.

### M3 — SQL IDE 품질

scope/alias/CTE/search_path/catalog binding, completion/ranking,
diagnostics/quick fixes, go-to/usages/docs, format/snippets,
rename/extract refactoring preview와 파일 단위 원자적 적용.
DB object rename/DDL 실행은 문서 refactoring과 별도 경계로 처리.

통과: dialect별 정답 fixture, 미완성 SQL 복구, stale snapshot 폐기,
지원하지 않는 동적 SQL/불명확 참조에 잘못된 rename을 적용하지 않음.
정확성과 지연을 함께 보고. parser 기능을 semantic 완성도로 집계하지 않음.

### M4 — 고급 DB 도구

schema/object diff와 방향 있는 migration preview,
data compare, ER diagram, explain tree/graph, charts,
대형 export/import·native dump 도구 연동, DDL offline sources,
프로젝트 Git/history integration 및 고급 검색.

통과: migration은 source/target 방향·의존성·implicit commit·부분 실패 표시.
EXPLAIN ANALYZE는 실제 실행임을 UI에 표시. 데이터 비교는 row identity와 비교 범위를 명시.

### M5 — DB 범위 확대

SQL Server/Oracle/ClickHouse, 이어 PostgreSQL 계열·warehouse/cloud,
MongoDB/Redis 및 장기 JDBC 목록 대응을 adapter 조사 결과 순으로 진행.
각 엔진은 인증/TLS/타입/metadata/DDL/explain/취소를 별도로 검증.
NoSQL은 문서/key-value 모델을 유지하며 SQL table로 강제 평탄화하지 않음.

통과: 엔진·버전·기능별 support matrix와 fixture가 함께 존재.
실제 자격 증명/검증 환경이 없는 cloud/enterprise 지원은 planned 또는 unverified.
JDBC bridge 없이는 모든 JDBC driver 동등 지원을 약속할 수 없음.

### M6 — 제품화와 잔여 기능

접근성/IME 회귀, Windows/Linux, 패키징/서명/업데이트,
복구/설정 migration, 플러그인 API, 선택형 AI/MCP,
cloud discovery 및 stored routine debugger 등 vendor별 잔여 기능.

통과: 타 플랫폼 기능/성능 회귀 통과, F01–F42와 DB matrix 리뷰,
미지원/부분 지원 목록 공개. 어떤 단계도 자동으로 전체 parity를 뜻하지 않음.

## 첫 구현 작업의 순서

| 순서 | 작업 | 의존성 | 결과 |
|---|---|---|---|
| 1 | baseline fixture·측정 규격 고정 | 현재 계획 | 재현 데이터와 결과 JSON schema |
| 2 | GPUI editor/grid spike | 1 | IME/접근성/프레임/메모리 기록 |
| 3 | PG driver contract spike | 1; 2와 병렬 가능 | session/cancel/multi-result 계약 |
| 4 | UI/driver ADR 확정 | 2, 3 | 구현 dependency 버전 고정 |
| 5 | domain/driver/session actor 구현 | 4 | UI 없는 integration tests |
| 6 | PG 수직 UI 연결 | 5 | M1 사용자 흐름 |

작업 분할은 crate/문서 소유권을 명시한다. actor 계약 변경은 메인 통합 담당이 관리.
병렬 worker는 UI, driver fixture, SQL corpus처럼 겹치지 않는 범위로 분리.
각 단계 구현 뒤 독립 읽기 전용 correctness 리뷰를 수행.

## 일정과 추정 방법

이번 단계에서 인원/가용 시간/실측 생산성은 확인하지 않았으므로 완료 날짜를 약속하지 않는다.
M0에서 실제 구현한 editor/grid/PG 세션 작업량과 재작업률을 기록한 뒤 M1–M2를 재추정한다.
M3 의미 분석과 M5 다중 DB는 독립적인 장기 작업이며 단순 기능 개수 비례 추정 금지.
빠른 데모와 DataGrip 대체 가능 상태는 구분한다.

## 남은 조사

- DataGrip 실제 사용 시 어떤 동작이 느린지는 미측정. CPU/메모리 프로파일과 런타임 비교 필요.
- JAR 바이트코드 메서드/분기 분석은 미수행. 모호한 동작이 구현에 영향을 줄 때 대상 메서드만 조사.
- 실행 취소 2단계, pending edit/commit, multi-result, DDL implicit commit은 fixture로 동작 대조.
- UI/driver crate API 및 release compatibility는 M0에서 컴파일로 검증.
- DB별 버전 목록은 지원을 선언할 때 고정. 문서나 class 이름만으로 지원 완료 판정 금지.

## 단계별 완료 정의

**핵심 DB 실무 대체 milestone:** Tier A DB에서 M1–M4 기능이 사용 가능하고,
F01–F42 각각에 구현 상태/검증 근거/제약이 있다. 이것만으로 전체 범위 완료를 선언하지 않는다.

**최종 제품 범위 완료:** M5–M6를 포함해 합의한 engine×feature acceptance를 충족한다.
planned/blocked/partial 행이 남으면 전체 완료가 아니다. 제외는 사용자와 명시적으로 합의한 항목만 가능.
DB support backlog 확정은 M0 산출물이며 미확정 엔진을 묵시적으로 완료 범위에서 빼지 않는다.

두 기준 모두 정확성·데이터 무결성 테스트를 통과한 상태에서 BENCHMARKS 목표를 평가한다.
현재 구현/검증 범위와 남은 gate는 [M0-RESULTS.md](M0-RESULTS.md)에 기록한다.
