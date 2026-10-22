# 현재 성능 측정과 다음 작업 — 2026-10-22

후속 구현/측정: [긴 셀 미리보기와 실제 GPUI frame histogram](UI-FRAME-PERFORMANCE-2026-10-22.md), [SQLite 독립 CSV 스트리밍](STREAMING-EXPORT.md). 아래 표는 최적화 전 기준선으로 유지한다.

측정 제품 기준: `4839ee83114947c0fcdbe076b0bb69fed990787c`. Apple M5 / RAM24GiB / macOS27.0 / AC 전원. release 패키지 실행 파일을 고정했고, 빌드를 마친 뒤 부하 측정들을 순차 실행했다. 다른 데스크톱 앱은 유지했으며 OS·Metal·파일 캐시는 비우지 않았다. 이번 변경은 측정 도구와 보고서이며 제품 최적화 적용 전 기준선이다.

**현재 보관 상한 안의 작업에서는 유휴 자원·검색·조회가 가벼운 편이다. 이 표본에서 긴급 성능 병목은 확인되지 않았다.** 실제 화면 표시/입력 지연과 대량 DB fetch는 아직 판정할 수 없다. DataGrip 동등 작업 비교도 수행하지 않았다.

## 시작·유휴 상태

각 구성10회 프로세스 재시작. 시작 수치는 **main 진입→첫 root-render callback**이다. 첫 paint, 복구 완료, 입력 가능 시각이 아니다. 10표본 nearest-rank p95는 최댓값이다.

| 구성 | callback 중앙값 | callback p95 | 유휴 RSS 중앙값 | 유휴 CPU |
|---|---:|---:|---:|---:|
| 빈 연결 설정 화면 | 184.0ms | 567.1ms | 126.9MiB | 0.45% |
| 파일500개·8탭을 설정한 workspace | 177.7ms | 193.4ms | 125.8MiB | 0.52% |

유휴는65초 실행 중 처음5초를 제외한 약60초, 약250ms 간격으로 main process를 샘플링했다. CPU100%는 한 코어이며 GPU/WindowServer/helper 메모리는 포함하지 않는다. RSS 끝−시작 변화는 빈 화면+0.031MiB, workspace−0.109MiB였다. 장시간 누수 부재를 증명하지 않는다. workspace 구성의 active SQL8444bytes는 복구 진행 근거지만, 전체 파일 목록/8탭 paint 완료 시각은 별도 계측하지 않았다. 두 구성은 화면 내용·순서가 다르므로 메모리/시작시간 우열 비교로 해석하지 않는다.

## SQL 조회

각 case warmup3회 후30회. 전체 wall은 새 CLI 프로세스·설정·인증/연결·실행·디코딩·JSON/stdout까지 포함한다. 어댑터 elapsed도 DB 서버만의 실행시간은 아니다. SQLite는6열, PG/MySQL은4열과 서로 다른 생성 SQL이므로 엔진 순위 비교가 아니다. 네트워크 표는 실행 파일을 고정 복사한 최종 재측정값이다.

| 엔진 / 행 | 전체 wall 중앙값 | 전체 wall p95 | 어댑터 query p95 |
|---|---:|---:|---:|
| sqlite 1 rows | 11.50ms | 13.04ms | 0.108ms |
| sqlite 1000 rows | 12.47ms | 12.91ms | 0.529ms |
| postgres 1 rows | 18.41ms | 22.40ms | 2.552ms |
| postgres 1000 rows | 21.03ms | 25.59ms | 4.064ms |
| mysql 1 rows | 19.63ms | 23.47ms | 1.840ms |
| mysql 1000 rows | 20.04ms | 24.08ms | 2.246ms |

SQLite는 생성한 임시 파일, PostgreSQL18.6/MySQL8.4.11은 owner label과 loopback 포트를 확인한 프로젝트 전용 컨테이너만 사용했다. 사용자 DB·Keychain은 사용하지 않았다. 조회 결과는 모두 성공/행수/비절단을 확인했다.

## 검색·파일 API

각 case warmup3회 후30회, 따뜻한 로컬 파일 캐시. p50/p95는 nearest rank(15/29번째)이며 UI debounce·worker scheduling·paint는 제외한다. fixture 생성, 검증 assertion, JSON 직렬화, 반환값 drop도 제외한다. CAS 저장은 실제 파일 write/fsync와 해시 갱신을 포함한다.

| 작업 | p50 | p95 | 범위 |
|---|---:|---:|---|
| 결과 마지막 셀 검색, 대소문자 구분 | 1.429ms | 2.324ms | 100,000셀 /3,000,004bytes |
| 결과 전체 무일치 검색, 대소문자 무시 | 5.208ms | 5.588ms | 100,000셀 /3,000,004bytes |
| 폴더 목록 | 0.770ms | 0.796ms | 2,000파일 중512반환; partial |
| SQL 읽기+해시 | 0.126ms | 0.135ms | 64KiB |
| SQL CAS 저장 | 4.973ms | 8.254ms | 64KiB |
| 저장 객체 찾기 | 2.368ms | 3.271ms | 8,000객체 /16shards /933,375bytes |

## 논리 그리드 관찰

네이티브 앱에서1,000,000행×200열의 마지막 셀 이동과 세로 스크롤을 확인했다. **실제 DB100만 행 fetch가 아닌 지연 생성 UI fixture**다. 30초 pilot의 sampled RSS 최대는129.75MiB, 강제 redraw 중 CPU는43.96%/core였다.

root callback interval은 마지막10,000개 ring만 남았으며 약83.33초 구간의 p50=8.332ms, p95=9.035ms다. CPU30초 표본과 정확히 같은 구간이 아니다. 이를120FPS, 실제 scroll FPS, key-to-paint 지연으로 발표하지 않는다. GPU presentation/입력 이벤트 계측이 추가로 필요하다.

## 병렬 탐색 결과와 실행 순서

성능 감사와 기능/UX 감사를 별도로 수행했고, 측정 해석도 독립 검토했다. 아래 성능 항목은 **코드상 위험 또는 미측정 구간**이지 이번 실측으로 확정한 병목이 아니다.

| 순서 | 다음 작업 | 근거 / 현재 상태 | 최소 검증·범위 |
|---|---|---|---|
| 1 | 실제 UI 지연 계측·큰 셀/wide rows 테스트 | `main.rs:147–155,278–302`는 전체 값 clone·AX 문자열 생성. 현재 logical grid는 짧은 문자열뿐 | 1KiB/64KiB/상한 근처 셀, key→paint·첫행 표시·스크롤 계측. 필요할 때만 표시 prefix 제한, 원문 copy 유지. M |
| 2 | progressive fetch·전체 streaming export | 현재 driver 보관 상한512KiB/4096events. generic query의 전체 fetch/export는 미구현 | 상한 넘는 fixture에서 중복/누락 없는 다음 page, 취소/부분 파일, memory envelope. L |
| 3 | M1 네이티브 통합 수용 테스트 | 개별 연결/탭/취소/기록 기능은 존재하지만 M1 전체 acceptance 미종료 | 소유 PG fixture에서 연결→트리→쿼리→취소/실패→다중 결과→기록. 자동 재실행/commit 없음. M–L |
| 4 | 입력 중 자동완성·파라미터 바인딩 | 명시적 자동완성은 구현. typing trigger와 F15 parameter prompt는 없음 | IME 유지 debounce·stale 취소, NULL/타입/반복 marker/주석·문자열 처리. M/L |
| 5 | 메타데이터 burst·32탭 recovery 계측 | `navigator.rs:464–548`의 UI 변환/clone, `workspace_recovery.rs:107–125` 전체 snapshot 복사 | 큰 메타데이터 burst와32×64KiB 문서에서 UI tick/복사량 측정. 필요하면 worker 변환·변경분 snapshot. M |
| 6 | 파일별 DB 대상·독립 workspace 탭 세트 | 현재 single-root/default target; 프로젝트 전환 시 기존 탭 보존 | 파일2개를 다른 DB에 묶고 복구/경로/endpoint 변경/CAS 검증. L |
| 7 | FK 관련 행 이동·DDL 편집 흐름 | FK/인덱스/뷰 metadata와 SQL draft는 존재, 관련 행 탐색·완성된 DDL 편집 workflow는 없음 | 정확한 대상/key filter, SQL preview 후 명시 실행. M–L |
| 8 | 에이전트 context UI·키보드/IME/접근성 acceptance | CLI capabilities/context/plan은 구현, 명시적 GUI AI context·physical IME 전체 검증은 남음 | 선택 root/files만 포함, 대상/내용 표시, 실행 명시적. physical Hangul/VoiceOver 별도. M |

지금은40ms polling을 event-driven 구조로 대대적으로 바꾸거나 검색 엔진을 재작성할 근거가 약하다. 유휴 CPU0.45–0.52%/core, 검색 p955.59ms로 작다. 최대2,048 manifest/64shard·lease/eviction 상한 fixture에서 재현된 뒤 filename 정렬키 사전 계산, cache admission fast path, 파일 목록 virtualizer를 선택한다. 현재512개 파일 목록은 bounded지만 비가상화다.

PG namespace/row Apply 차단 작업은 이 성능 개선 후보에 포함하지 않았고 Apply는 계속 비활성이다.

## 재현과 원시 근거

측정 중 다른 빌드·벤치마크를 동시에 실행하지 않는다. 아래 네트워크 측정은 고정된 소유 fixture만 허용한다. GUI smoke는 새 임시 설정과 생성 fixture를 사용한다.

```sh
cargo build --release -p dbide-cli
cargo build --release -p dbide-ui-spike --example measure_workflows
uv run --python ">=3.14" scripts/measure-current-performance.py
uv run --python ">=3.14" scripts/measure-owned-query-performance.py --owned-loopback
target/release/examples/measure_workflows
```

- [시작·유휴·SQLite 원시 표본](../evidence/performance-2026-10-22-runtime.json)
- [PG/MySQL 원시 표본](../evidence/performance-2026-10-22-network.json)
- [검색·파일 함수 원시 표본](../evidence/performance-2026-10-22-workflows.json)
- [논리 그리드 pilot](../evidence/performance-2026-10-22-grid.json)
- [실행 파일·도구 해시와 검증](../evidence/performance-2026-10-22-provenance.json)

이 결과는 위 기기·빌드·fixture·상한에 대한 기준선이다. 다른 DB 크기, 네트워크, physical IME, GPU frame latency, 장기 메모리 누수 또는 DataGrip 성능 우위를 증명하지 않는다.
