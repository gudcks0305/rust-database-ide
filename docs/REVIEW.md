# 계획 검토 기록

2026-10-21. 독립 읽기 전용 architecture review 수행. 제품 구현 리뷰가 아님.

## 발견 및 반영

1. CommitPolicy와 실제 서버 transaction 상태가 혼합되어 직접 BEGIN/COMMIT과 implicit commit을 제대로 설명하지 못함.
   ARCHITECTURE에서 별도 타입/상태로 분리. BENCHMARKS에 직접 transaction SQL 검증 추가.
2. 초기 완료 정의가 M5–M6가 planned여도 전체 완료로 읽힐 수 있음.
   PLAN에서 핵심 DB 실무 대체 milestone과 최종 전체 범위 완료를 분리.

## 확인한 경계

- REA 5개 operation의 metadata/ZIP/Mach-O 범위와 보조 Python 정적 조사 구분.
- 메서드 디컴파일/런타임/성능 실측 미수행 표시.
- 넓은 기능 backlog 유지, UI/driver 확정 전 spike.
- 세션 고정, cancel race, 결과 불명 상태, bounded stream, SQL syntax/semantic 분리.

## 한계

독립 검토는 문서와 증거 색인 대상. 제품 컴파일/DB/성능 검증은 수행하지 않음.
원본 Evidence 5개 파일 hash, 문서 상대 링크, 기능 ID와 inventory 합계는 메인 작업의 검증 스크립트로 별도 확인.

## M0 구현 후 독립 검토

같은 날 별도 읽기 전용 코드 리뷰 후 다음을 수정하고 회귀 검증했다.

- SQLite: 이전 DML changes count가 DDL/transaction에 재사용됨 → 분류되지 않은 affected rows None.
- SQLite: 초과 statement prepare 단계 PRAGMA 부작용 → cap 경계 authorizer, trailing comment/다음 실행 검증.
- SQLite: shutdown 뒤 queued 장기 query가 다시 시작됨 → admission lock/stopping flag/native interrupt.
- Bridge: target 없는 cancel이 다음 실행으로 재바인딩됨 → exact ExecutionKey 또는 OperationId 필수.
- Bridge: actor 시작 전 cancel/disconnect가 취소를 놓침 → Running 또는 terminal 대기, 결과 보존.
- Bridge: socket close만으로 PG query가 남음 → worker cancel/drain과 backend disappearance 검사.
- UI: 첫 40ms 동안 cancel key 없음 → immutable operation target fallback.
- UI: grid stripe filler가 가짜 AX Row 노출 → 실제 row만 의미 부여.
- SQL: shape-only 비교 → 모든 node byte/point range와 구조 검증, 회귀 추가.

Reviewer가 기존 DB/bridge P1/P2 해결을 source 재검토로 확인했다.
메인 통합 검증은 `scripts/verify-m0.sh` 전체 통과와 실제 native UI 동작 확인으로 수행.
