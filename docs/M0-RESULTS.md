# 초기 M0 구현 및 측정 기록

2026-10-21. M0 실험과 초기 UI/driver 통합. 전체 제품 완료 또는 모든 M0 gate 통과를 의미하지 않는다.
이 파일의 driver Unknown/fixture-only 설명과 테스트 수는 당시 버전 기준이다.
이후 PG wire 상태·일반 프로필·TLS·CLI 등이 추가됐다. 최신 완료 판정은
[M0/M1 acceptance](M0-M1-ACCEPTANCE.md)와 [현재 상태](CURRENT-STATUS.md)를 따른다.

## 현재 구현

| 영역 | 구현 | 근거 |
|---|---|---|
| Rust workspace | 1.99.1 pin, Cargo.lock, native packaging | root Cargo.toml/rust-toolchain.toml/scripts/package-ui.sh |
| PostgreSQL | 고정 세션, 독립 취소/connection retirement, 결과 cap, 오류/unknown 상태 | dbide-pg-spike + evidence/pg-spike.json |
| SQLite | worker-owned connection, native script boundaries, scope cancellation, shutdown, 타입/락 검증 | dbide-sqlite-spike + evidence/sqlite-spike.json |
| SQL 분석 | sqlparser/Tree-sitter corpus, UTF-8 incremental edit, node byte/point 일치 검증 | dbide-sql-spike + evidence/sql-spike.json |
| UI 1차 | SQL 입력, 한글 Unicode 표시, undo, 1M×200 lazy grid, far-corner 이동 | native runtime 관찰 + evidence/ui-smoke.json |
| UI↔PG bridge | 독립 worker runtime, nonblocking 명령·이벤트, 실제 연결/실행/취소 | dbide-bridge + evidence/ui-native-qa.json |

## 실제 확인한 1차 UI 동작

GPUI 0.2.2 + component 0.5.1의 release native 창을 직접 실행했다.
Last cell 동작으로 행 1,000,000·열 200을 표시했고, id 고정 열도 유지되었다.
SQL 창에 `SELECT '한글 입력 😊' AS 이름;`을 붙여넣어 표시한 뒤 Cmd+Z로 원문 복원을 확인했다.
이것은 native IME 조합/후보창/한영 전환 검증을 대신하지 않는다.

동일 창의 접근성 tree는 window/title만 노출하고 editor/grid 노드가 없었다.
해당 pinned GPUI에는 AccessKit bridge 자체가 없어 framework API가 있는 후속 버전으로 시험을 확장했다.
기존 버전에서 VoiceOver 통과 또는 UI framework 확정이라고 주장하지 않는다.

## 업그레이드 후 실제 native QA

GPUI-pre/platform 0.3.9 + component/assets 0.8.0 빌드 성공. [ADR](ADR-M0.md)에 선택 이유 기록.
native 창을 Raise한 후 AX에 SQL editor/value, toolbar 버튼, table/row/cell 값 노출 확인.
작은 1행 결과에서 stripe filler가 가짜 접근성 행으로 나타나지 않도록 수정했다.

직접 UI에서 다음을 확인했다 (`evidence/ui-native-qa.json`):

- Connect → 기본 SELECT 실행 → 실제 PostgreSQL 1,000행·6열 표시.
- 두 SELECT 실행 → Previous/Next로 결과 전환. NULL과 빈 text 구분, 한글·이모지 결과 보존.
- `SELECT pg_sleep(30)` 실행 직후 Cancel → SQLSTATE 57014, Cancelled, 재연결 필요 표시.
- 재연결 뒤 `SELECT 1; SELECT 1/0; SELECT 3` → 앞 결과 1 보존, SQLSTATE 22012, 뒤 결과 미실행.

`evidence/ui-pg-smoke.jsonl`에도 실제 자동 연결/쿼리 완료 결과를 저장했다.
UI는 terminal report 수신 뒤 결과를 표시한다. 아직 progressive fetching UI가 아니다.
native AX 노출을 전체 VoiceOver·한국어 IME 조합 통과로 확대 해석하지 않는다.

동일 자동 `pressKey(g,k,s)` 입력은 GPUI editor와 별도 macOS `NSTextView` 대조군 모두에서
`ㅎㅏㄴ`으로 나타났다. 이 입력 경로로 실제 물리 키보드의 연속 marked-text 조합을 검증할 수 없으므로
GPUI만의 조합 결함으로 분류하지 않는다. 대조군은 프로젝트 `tmp/`의 임시 앱이며 검사 후 종료.

## 초기 계측

- `evidence/ui-smoke.json`: 0.2.2 release binary로 process 재시작 5회.
  root render callback까지 97.3–284.3 ms, sampled main-process RSS peak 약 140.7–141.3 MiB.
  root callback은 first paint가 아니며 OS/Metal cache를 비우지 않았음. 독립 GPU memory/process-tree 측정 아님.
  해당 binary/source hash 보존. 정확한 옛 소스/바이너리는 로컬 `tmp/ui-stable-snapshot/`.
- `evidence/sql-spike.json`: 10 KiB, 1 MiB, 10 MiB 반복 SQL buffer의 incremental edit 5회.
  실제 표본을 참고하고 p95/p99로 확대하지 않는다. 10 MiB에서는 입력 스레드 예산을 넘을 수 있어
  background parsing, revision cancellation, 크기별 분석 정책이 필요하다.
- `evidence/datagrip-runtime.json`: 격리 config/system/plugins/log에서 DataGrip 첫 실행 welcome 화면 관찰.
  설정 import는 건너뛰고 선택형 AI 가입 안내는 닫았다. 자동 생성 default project에 DB 연결은 하지 않았다.
  주 프로세스 idle RSS만 관찰했으며 동일 DB workload/시작 시간 대조는 미완료.
- `evidence/ui-smoke-current.json`: 최종 GPUI-pre 0.3.9 release 5회 process 재시작.
  root render callback 약 145–171 ms, sampled main-process RSS peak 약 130.0–130.4 MiB.
  first paint/IME/DB throughput/전체 메모리 측정은 아니며 같은 계측 한계를 유지.

가상 grid와 DataGrip 전체 welcome workspace의 기능/상태가 다르므로 메모리 감소율·속도 향상 배수를 계산하지 않는다.

## 계약 검증

- PostgreSQL 18.6 ARM64 Docker, tokio-postgres 0.7.18: live scenario 8개 통과.
- SQLite 3.53.2/rusqlite 0.40.2: scenario 6개, 회귀 포함 unit tests 7개 통과.
- SQL: statement boundary/미완성 입력/UTF-8 edit/동일 shape의 다른 range 검출 unit tests 4개 통과.
- 독립 reviewer가 발견한 SQLite stale DML row-count 문제를 `Command(None)`으로 수정.
  zero budget 선처리와 shutdown 중 대기 query 실행 방지 회귀도 추가.
  추가 발견한 budget 초과 문장의 prepare-time PRAGMA 부작용은 native authorizer로 차단하고 다음 실행의 정상 복구까지 검증.
- PG terminal_error는 결과 cap 이후에도 보존. 실제 server transaction status는 여전히 Unknown.
  Manual policy는 metadata이며 명시 BEGIN이 필요하다. 생산용 adapter 완료로 집계하지 않음.
- Bridge는 target ID 없는 취소와 actor 시작 전 취소 경쟁을 수정. 즉시 실행→취소/연결 종료,
  이전 key 취소가 다음 실행을 건드리지 않는 것, 서버 backend 9개 소멸을 live test로 확인.
- 전체 자동 검증: unit tests 16개 + 명시 실행한 live integration tests 2개 통과.
  `cargo fmt --check`, workspace clippy `-D warnings`, shellcheck, 문서/증거 검증 통과.
  기본 workspace tests에서 live tests 2개는 ignored이며, 별도 실행 결과로만 통과 집계했다.

## 남은 gate

1. 업그레이드 UI의 AX 노출은 확인. keyboard/VoiceOver selection 의미의 전체 검증은 남음.
2. native 한국어 IME 조합/후보창·VoiceOver 조작 전체 검증.
3. 같은 DB/schema/쿼리의 DataGrip 대비 end-to-end baseline.
4. PG protocol transaction state, typed values, COPY/TLS, streaming backpressure와 LOB.
5. 42개 기능군을 완성할 M1–M6 구현. 현재 실험은 기능 동등성 완료가 아님.
