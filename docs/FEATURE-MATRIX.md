# 기능 범위 / 구현 추적

상태: 2026-10-21. **42개 기능군의 전체 완료 항목은 아직 없음.** 현재 구현·실연동·UI 검증과 남은 조건은 [CURRENT-STATUS.md](CURRENT-STATUS.md)에 구분한다. 아래 기능군 상태는 전체 완료 판정이 아니다.
단계는 목표 순서이며 출시 약속이 아님. 완료 시 각 행에 테스트/버전/제약 링크 추가.
참조 번호는 [SOURCES.md](SOURCES.md). 표는 사용자 요구를 구현 가능한 계약으로 분해한 계획이며 DataGrip 내부 코드의 복원본이 아니다.

| ID | 기능군 | 최초 단계 | 완료 기준 | 근거 | 현재 상태 |
|---|---|---|---|---|---|
| F01 | DB 연결 프로필·URL·옵션·테스트 | M1 | 프로필 분리, 비밀 저장소 참조, 오류 분류 | D01 | partial — 일반 프로필/Keychain/3 engines; URL/options 확대 남음 |
| F02 | TLS·SSH tunnel·jump host | M1 TLS / M2 SSH | 인증서/host key 검증, auth 실패·재연결 테스트 | D02 | partial — TLS 실검증, SSH/jump host 미구현 |
| F03 | 연결 lifecycle·keepalive·timeout·startup script | M1–M2 | 연결 generation 갱신, 활성 transaction 무단 재실행 없음 | D01,D03 | partial — reconnect/cancel/timeout; startup script/keepalive 남음 |
| F04 | console별 session·공유/전용·metadata session | M1 | temp table/session variable 격리와 의도한 공유 검증 | D03 | experimental |
| F05 | Auto/Manual transaction·isolation·savepoint·read-only | M1–M2 | submit/commit/rollback 구별, 실패한 transaction 복구 | D04 | partial — 실제 state/Auto/Manual/rollback; isolation UI 남음 |
| F06 | schema/catalog introspection·범위·증분 refresh | M1–M2 | 전체/선택/변경분 조회와 취소, cache generation 검증 | D05 | partial — PG bounded catalog, 3-engine basic metadata; 증분/lazy 남음 |
| F07 | Explorer·객체 검색·필터·DDL/source | M1–M2 | lazy tree, stale/미조회 상태, object identity 보존 | D05 | planned |
| F08 | SQL 탭·console·scratch·datasource/schema context | M1 | 저장/재시작 복구, 잘못된 DB 실행 방지하는 명확한 context | D06 | partial — SQL 파일 탭·dirty 보호; restart 복원 통합 중 |
| F09 | dialect syntax·highlight·statement boundary | M1–M3 | quotes/comments/procedure/dollar quote/미완성 구문 corpus | D06,D09 | experimental |
| F10 | semantic completion·alias/CTE/scope·type resolution | M3 | catalog/context별 정답 fixture, stale 결과 폐기 | D09,D10 | partial — catalog/alias/CTE 명시 completion; type resolution 제한 |
| F11 | inspections·diagnostics·quick fixes | M3 | syntax/semantic/server 오류 분리, fix preview | D09 | partial — syntax/catalog diagnostics; quick fixes 제한 |
| F12 | declaration/usages/docs·객체 navigation | M3 | 식별자 binding 기반 이동, 미해결 참조 표시 | D09,D10 | planned |
| F13 | rename/extract/refactor | M3 | 참조 검증, preview/undo, 동적 SQL unknown 유지 | D10 | partial — alias/CTE rename preview; extract 미구현 |
| F14 | format·code style·snippets·multi-cursor | M1 basic / M3 | 의미 보존, 선택 범위·dialect·IME 회귀 | D09 | partial — format/snippet backend; style/multi-cursor 남음 |
| F15 | parameter prompt/binding·named/positional | M1–M2 | NULL/타입/반복 marker/escaping, raw SQL과 구분 | D06 | planned |
| F16 | current/selected/script execution·multi-result | M1 | 실행 순서, update count, notices, partial failure 보존 | D06 | experimental |
| F17 | 취소·강제 연결 종료·실행 상태 | M1 | cancel≠future drop, next-query race 없음 | D06 | experimental |
| F18 | history·검색·재실행·즐겨찾기 | M1–M2 | context 보존, 민감값 처리, 자동 재실행 없음 | D06 | partial — context 기록/재열기; 검색/즐겨찾기 남음 |
| F19 | 결과 grid·paging/limit·가상화·결과 탭 | M1 | 양축 가상화, row/byte limit, multi-result별 상태 | D07 | experimental |
| F20 | JSON/tree/text/transpose·LOB/value editor | M2 | 타입 구분, lazy 큰 값, 표시 제한 | D07 | planned |
| F21 | grid CRUD·paste·NULL/default·clone | M2 | key 기반 edit, optimistic conflict, generated column | D07 | partial — PG typed CRUD/batch 실검증; UI 통합 중 |
| F22 | filter/sort/search·서버/로컬 구분 | M2 | fetched subset 정렬과 전체 DB 정렬 구별 | D07 | planned |
| F23 | staged/submitted/committed·revert | M2 | Auto/Manual commit에 맞는 실제 DB 상태 반영 | D04,D07 | partial — PG transaction/savepoint batch; staged UI 통합 중 |
| F24 | CSV/TSV/JSON 및 후속 XML/Markdown/XLSX export | M2–M4 | streaming·escaping·encoding·취소·partial 파일 표시 | D08 | partial — CSV/TSV/JSON snapshot; 전체 streaming export 남음 |
| F25 | import·column/type mapping·preview·batch | M2–M4 | 타입 손실/중단 처리 범위·transaction 정책 검증 | D08 | partial — preview/mapping core; DB import 적용 남음 |
| F26 | table/query-to-table copy·native dump/restore | M4 | 대상/방향 표시, 도구 버전 기록, 부분 실패 보존 | D08 | planned |
| F27 | object create/modify·DDL generator | M2–M4 | DB별 option/quoting/default/dependency fixture | D11 | partial — index/view SQL drafts; 편집/실행 acceptance 남음 |
| F28 | schema diff·migration preview | M4 | source→target 방향, dependency order, implicit commit | D11 | partial — constrained schema diff/preview; apply 남음 |
| F29 | data diff/compare | M4 | row key/order/NULL/decimal 규칙, 비교 범위 명시 | D12 | partial — keyed comparison; 대형 데이터 검증 남음 |
| F30 | ER diagram·관계 탐색·export | M4 | FK/수동 관계 구분, 대형 schema layout 취소 | D13 | partial — FK SVG+export; 대형 layout 검증 남음 |
| F31 | EXPLAIN/ANALYZE·tree/graph | M2 basic / M4 | plan-only와 실제 실행 구분, 엔진별 의미 보존 | D14 | partial — PG plan-only; ANALYZE/기타 엔진 남음 |
| F32 | charts·aggregate view | M4 | 타입/표본 범위 명시, 큰 결과 UI 중단 없음 | D07 | planned |
| F33 | DDL files/offline datasource | M4 | 오프라인 catalog 생성, 실제 DB와 구분 | D15 | partial — constrained DDL offline parser; dialect 확대 남음 |
| F34 | Git/local history·프로젝트·검색/replace | M2 basics / M4 | 파일 변경/복구, credential과 버전관리 분리 | D09 | planned |
| F35 | MongoDB document/collection query | M5 | BSON/JSON 타입과 namespace, 문서 기반 capability | D16 | planned |
| F36 | Redis key browser/command/cluster | M5 | key type/TTL/cluster 차이, table 가정 없음 | D16 | planned |
| F37 | warehouse/cloud·enterprise engine 확장 | M5 | engine/version/auth/type/metadata별 검증 | D01 | planned |
| F38 | cloud resource discovery | M5–M6 | 계정/region/context 선택, 비용/권한 실패 상태 | P01 module evidence | planned |
| F39 | 키맵·theme·dock·접근성·한국어 IME | M0–M6 | keyboard-only/VoiceOver/조합/undo, OS별 회귀 | 제품 요구 + U01–U03 | experimental |
| F40 | Oracle PL/SQL debugger | M6 | breakpoint/step/watch/session 제약 검증; DataGrip 문서상 Oracle만 지원 | D17 | planned |
| F41 | 선택형 AI assistance·MCP/tool integration | M6 | 명시적 연결, query/credential 전송 범위·실행 정책 | P01 module evidence | partial — WS/MCP backend; live AI/UI 통합 중 |
| F42 | 플러그인/driver 확장·설정 이전·업데이트 | M6 | versioned API, capability 설명, migration/recovery | 제품 요구 + D01 | partial — declarative snippets/themes; settings/release/update 남음 |

## DB 지원 확대

| Tier | 엔진 | 목표 | 확정되지 않은 부분 |
|---|---|---|---|
| A | PostgreSQL, SQLite, MySQL/MariaDB | M1–M4 핵심 기능과 correctness suite | driver 선택, 실제 지원 version은 M0에서 고정 |
| B | SQL Server, Oracle, ClickHouse | M5 native adapter + 엔진별 기능 | SDK/auth/driver completeness/배포 요구 |
| C | CockroachDB, Redshift, Greenplum, Snowflake, BigQuery 등 | 계열 공통 기능 + 독립 capability 검증 | wire 호환이 DDL/type/introspection 동등성을 뜻하지 않음 |
| D | MongoDB, Redis, DynamoDB, Cassandra, Couchbase 등 | 문서/key-value/엔진 전용 모델 | SQL grid 계약의 범용화 불가 |
| E | Db2, Sybase, Vertica, H2/HSQLDB, Derby, Hive/Spark, 기타 JDBC-only 대상 | 장기 adapter 조사/구현 추적 | JVM bridge 없이 지원 곤란한 대상은 blocked/unsupported로 유지 |

이 표는 모든 DB가 이미 지원된다는 뜻이 아니다. D01의 DataGrip complete/basic support 구분도 그대로 동등하게 달성한 상태가 아님.
M0에서 공식 지원 목록을 고정한 engine별 backlog를 생성하고 빠진 엔진도 누락 대신 명시 상태로 남긴다.
현재 generic JDBC 파일 직접 로딩은 설계 제외. 필요 시 JVM-free 요구와 충돌하는 별도 ADR로 결정.

## 상태 규칙

- `planned`: 해당 기능군 구현 예정.
- `experimental`: 일부 계약/동작을 실험 코드로 검증했으나 기능군 전체 정합성/복구/성능 조건 미통과.
- `partial`: 검증된 부분과 빠진 조건을 함께 표기.
- `supported`: 지정 DB/version/platform의 acceptance 통과와 재현 증거 존재.
- `blocked`: driver/라이선스/실제 검증 환경 등 구체적 외부 제약.
- `out-of-scope`: 사용자와 합의한 제외만 사용. 단계가 늦다는 이유로 임의 제외 금지.

기능군 개수로 단일 "호환률 %"를 계산하지 않는다. engine×feature acceptance coverage를 보고한다.

초기 실험 범위는 [M0-RESULTS.md](M0-RESULTS.md)의 당시 binary 기준이다. 이후 일반 프로필·3종 native engine·Auto/Manual 정책·실제 transaction 상태·CLI 등이 추가됐다. 최신 범위와 미완료 조건은 [CURRENT-STATUS.md](CURRENT-STATUS.md)를 따른다.
