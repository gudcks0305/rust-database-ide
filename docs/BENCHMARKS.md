# 성능과 정확성 검증

**아래 수치는 제안 목표이며 달성 결과/예측 개선율이 아니다.** 초기 smoke/parser/DB 계약 관찰은
[M0-RESULTS.md](M0-RESULTS.md)와 `evidence/*spike.json`, `evidence/ui-smoke.json`에 별도 기록.
동등 기능의 DataGrip 대조 benchmark는 아직 완료하지 않았다.
정적 JAR 수/크기나 Rust 사용 여부만으로 성능 결론을 내리지 않는다.

2026-10-21 phase3 release의 빈 격리 workspace 5회 smoke는
[`ui-phase3-smoke.json`](../evidence/ui-phase3-smoke.json)에 기록했다.
3초 구간의 process RSS 표본 최댓값은 121.3–122.1 MiB,
첫 root-render callback은 318–461 ms였다. 동시 개발/build 부하가 있었고
OS/Metal cache를 비우지 않았다. 60초 idle, 첫 paint, 입력 지연 측정이나
기존 M0 대비 성능 회귀/개선 판정으로 해석하지 않는다.

## 비교 조건

- 동일 macOS ARM64 기기, OS/build, 전원/열 상태, 디스플레이 주사율, 해상도 기록.
- DataGrip build, runtime, 플러그인, heap 설정, 결과 page/limit, introspection 범위 기록.
- Rust commit, compiler, dependency lock, `--release`, logging 설정 기록.
- DB version, schema, seed, row/column 수, 타입, 인덱스, DB process 설정과 네트워크 경로 고정.
- 동일 query/결과 row limit/동시 작업/표시 옵션. 기능을 꺼서 빨라진 경우 그 차이 공개.
- 서버 실행시간, 전송/디코딩, 최초 row 도착, 최초 paint, 전체 fetch를 분리.
- 앱 RSS만이 아니라 helper/driver 프로세스를 포함한 process tree RSS와 메모리 pressure 기록.

## fixture

| Fixture | 데이터 | 주로 검증할 것 |
|---|---|---|
| Grid logical | 1,000,000행 × 200열, 필요한 viewport만 lazy 생성 | 양축 가상화, select/scroll/paste/copy, 메모리 상한 |
| DB narrow | 1,000,000행 × 10열 mixed scalar | 실제 fetch/디코딩/streaming/export |
| DB wide | 100,000행 × 200열, nullable/decimal/timestamp | 수평 스크롤/타입 정확성/포맷팅 비용 |
| Large value | JSON/text 1 KiB·1 MiB·10 MiB, binary LOB | lazy load, truncation, size quota, 취소 |
| Catalog | 100·10,000·50,000 objects | lazy tree, 증분 introspection, completion cache |
| SQL buffer | 10 KiB·1 MiB·10 MiB, 오류·CTE·한글 포함 | 입력/undo/parse/resolve/stale result |
| Failure | slow query, disconnect, cancel race, lock timeout | outcome/transaction correctness |

논리 grid fixture는 UI만의 시험이다. DB end-to-end 성능으로 발표하지 않는다.
대형 catalog/DB 생성은 격리된 로컬 test DB에서 수행. 실제 사용자 DB는 사용하지 않는다.

## 제안 목표

| 측정 | 초기 목표 | 측정 정의 |
|---|---|---|
| UI 입력 반응 | p95 ≤50 ms | key event→반영된 frame, IME commit과 조합 별도 |
| 스크롤 | 60 Hz에서 frame p95 ≤16.7 ms | 고정 viewport/overscan, 양축 스크롤 |
| client 추가 첫-row 지연 | p95 ≤100 ms | row batch 수신→첫 paint; 서버시간 제외 |
| warm start | ≤1 s 목표 | process spawn→저장 workspace가 입력 수용, DB 미연결 |
| process-cold start | ≤2 s 목표 | 앱 process 미실행→입력 가능; OS page cache 상태 별도 |
| idle process-tree RSS | ≤250 MiB 초기 예산 | 빈 workspace, 안정화 후 60초 샘플 |
| 활성 row cache | 기본 128 MiB byte budget | row 수가 아닌 실제 추적 batch bytes |
| UI cancel feedback | ≤100 ms | 클릭→CancelRequested 표시; 서버 완료와 분리 |
| 서버 취소 확인 | 엔진별 baseline과 보고 | CancelRequested→confirmed/drained/connection closed |
| warm semantic completion | p95 ≤100 ms | cached catalog와 지정 SQL fixture, stale 결과 제외 |
| export/introspection | baseline 비교, 절대값 미정 | rows/s·bytes/s 또는 objects/s + peak RSS |

M0에서 하드웨어/화면/fixture를 고정한 뒤 현실성 검토. 목표 변경은 이유와 측정값을 ADR로 남김.
warm start/idle memory가 좋아도 같은 기능에서 query/data correctness가 나빠지면 합격 아님.

## 반복과 기록

- 시작 시간: process-cold 10회, warm 30회, 순서 교차. 이 표본에서 median/p95와 전체 표본 제시.
- steady UI: 각 시나리오 30초 × 5회, event/frame 수와 p50/p95/p99 기록.
- query/export: warm-up 3회 후 10회 이상; DB cache 상태 별도. 이 표본으로 p99 주장 금지.
- OS file cache를 비웠다고 가정하지 않는다. 재부팅/캐시 상태를 따로 기록하고 보안/전역 설정을 변경하지 않음.
- 실행 중 CPU profiler와 일반 측정은 분리; instrumentation overhead 표시.
- 결과 파일 필드: timestamp, host, build, dataset digest, parameters, raw samples, outcomes, memory, timings, limitations.

## 필수 정확성 시나리오

1. session A temp table과 session B 격리, explicit shared session의 일관성.
2. 수동 transaction edit→submit→다른 session 비가시→commit/rollback.
3. INSERT/COMMIT 전송 직후 연결 단절: outcome unknown, 자동 재실행 없음.
4. query N 완료 직후 cancel 도착: query N+1에 취소가 잘못 전달되지 않음.
5. 여러 result/update count/notice/중간 오류가 원래 순서대로 전달됨.
6. integer/decimal/NULL/empty/Unicode/timezone/byte/array/JSON 값 왕복 무손실.
7. PK 없는/join/duplicate row는 임의 업데이트하지 않음. 동시 수정 충돌 탐지.
8. quote/comment/procedure body의 `;`로 script를 잘못 분할하지 않음.
9. 조회중 refresh/role/search_path 변경 시 오래된 completion/rename 결과 적용 안 됨.
10. memory/spill/disk quota, 느린 UI consumer, export 취소, 프로세스 crash 후 복구.
11. schema migration 방향과 DDL implicit commit/부분 실패 기록.
12. 한국어 조합·후보창·cursor·undo·VoiceOver가 grid/editor 모두에서 동작.
13. 직접 `BEGIN/COMMIT/ROLLBACK` 실행 및 DDL implicit commit 후 CommitPolicy와 실제 ServerTransactionState를 각각 검증.
