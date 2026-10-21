# M0 실험 결정

2026-10-21. 확정 제품 구조가 아니라 검증한 후보와 남은 전제의 기록.
초기 결정 기록이다. 이후 완료된 driver wire-state/TLS/profile 작업은
[현재 상태](CURRENT-STATUS.md), 아직 통과하지 못한 종료 조건은
[M0/M1 acceptance](M0-M1-ACCEPTANCE.md)를 따른다.

## UI

초기 GPUI 0.2.2/component 0.5.1은 native 편집/가상화/Unicode/undo가 동작했지만
macOS AX tree에 editor/grid가 노출되지 않았다. 해당 버전 source에 AccessKit bridge가 없었다.
이를 해결하기 위해 `gpui-pre`/platform 0.3.9와 component/assets 0.8.0을 고정했다.
실제 native QA에서 접근성 text entry, 버튼, 표/행/셀과 값 노출을 확인했다.
현재 후보는 prerelease 의존성이라 API 안정성/VoiceOver 전체 사용성은 별도 검증 대상.
GPUI 확정 또는 모든 접근성 요구 충족이라고 선언하지 않는다.

구문 강조는 component의 syntect backend를 사용한다. parser 실험의 Tree-sitter 0.25와
새 UI의 optional Tree-sitter 0.26 native `links` 충돌을 피하며, parser 계약을 UI에 종속시키지 않는다.
Xcode Metal compiler component가 없어 공식 `runtime_shaders` 경로를 사용한다.
OS 전역 Xcode 경로나 Rust 기본 toolchain은 변경하지 않았다.

## PostgreSQL

tokio-postgres 0.7.18로 임의 simple SQL, 세션 고정, temp state, 다중 결과, cancel 경로를 검증했다.
취소한 연결은 일관되게 폐기한다. 세션 상태를 유지하는 취소 최적화는 아직 채택하지 않았다.
초기에는 protocol transaction status를 public API에서 얻지 못해 Unknown으로 두었다.
후속 [최소 driver patch](PG-STATUS-PATCH.md)로 실제 ReadyForQuery I/T/E를 노출했고,
현재 Auto/Manual·Failed·Rollback을 실서버와 native UI에서 검증했다.
이 개선이 IME/접근성 등 나머지 M0 종료 조건까지 통과했다는 뜻은 아니다.

UI 통합은 fixture 전용 별도 crate로 격리했다. 실행 직후 취소 intent와 exact target ID를 보존하고,
Drop에서 socket만 닫아 PG 작업을 남기는 문제가 live test에서 발견되어 cancel/drain을 추가했다.
생산용 endpoint/TLS/Keychain/SSH profile은 이 fixture 연결 코드로 대체하지 않는다.

## SQLite

rusqlite 0.40.2 bundled SQLite로 native Batch, 값 타입, lock timeout과 interrupt를 검증했다.
`sqlite3_changes`가 DDL/transaction에 이전 DML count를 재사용하므로 미분류 command count는 None.
문장 cap에서 다음 prepare 자체가 PRAGMA 효과를 낼 수 있어 authorizer를 cap 경계에 적용했다.
이 실험은 bounded retained report이며 생산용 progressive streaming store가 아니다.

## 다음 구현 우선순위

1. native IME composition/VoiceOver 전체 workflow와 프레임/입력 계측 완료.
2. PG ReadyForQuery 상태를 노출하는 driver 경계 확정, 실제 Auto/Manual 정책 구현.
3. 결과 스트림을 UI로 점진 전달하고 byte budget/LOB/spill 및 cancel under backpressure 검증.
4. fixture 밖 프로필·TLS·Keychain·schema explorer를 M1 UI에 연결.
5. 같은 DB fixture에서 DataGrip query/input/scroll/end-to-end 기준선 수집.

42개 기능 범위는 유지한다. 이번 결정으로 M2–M6를 제외하거나 완료 처리하지 않는다.
