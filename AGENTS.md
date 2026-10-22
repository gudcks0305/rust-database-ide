# Rust Database IDE

This repository contains a DataGrip investigation, implementation plan, and M0
Rust experiments. Read `README.md`, `docs/PLAN.md`, `docs/M0-RESULTS.md`, and
`docs/REA-FINDINGS.md` before work. Do not treat the spikes as finished drivers.

## Scope

- Product UI and core are Rust; the default runtime must not require a JVM.
- macOS ARM64 first. Framework and driver choices remain M0 spike decisions.
- Preserve the broad feature backlog in `docs/FEATURE-MATRIX.md`.
  Early milestones are not permission to drop later features.
- Distinguish REA output, ordinary static inventory, documentation, inference,
  proposed targets, and measured runtime behavior.
- Do not claim DataGrip feature parity or a performance gain from static data.

## Files

- `docs/`: product plan and evidence-backed findings.
- `evidence/`: metadata, hashes, and provenance safe to version.
- `evidence/raw/`, `tmp/`: local-only analysis output and reference artifacts.
- Do not put reference application bytecode/assets into product source.
- Do not inspect user database profiles or connect to their databases as part
  of fixture testing. Use isolated test databases and generated data.

## Checks

```sh
uv run --python '>=3.14' scripts/verify_project.py
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/package-ui.sh
git diff --check
```

PG live integration tests are explicitly ignored in normal runs. Start the
owned loopback Docker fixture and run them explicitly as documented in README.
Never call an ignored DB test passed merely because the default test suite passed.
Keep session/cancel/transaction correctness and IME/grid performance gates.

## 기본 동작과 사용성 우선

- 고급 기능 추가보다 기본 작업의 실패·불편을 먼저 해결한다. 기능 수나 코드 존재로 완성도를 대신하지 않는다.
- 변경한 기능은 사용자가 실제로 접근하는 메뉴·버튼·단축키부터 결과까지 이어서 확인한다. 버튼이 보이는데 작동하지 않거나 다른 탭을 조작하는 상태는 미완료다.
- 관련 기본 흐름을 먼저 검증한다: 연결 선택 → 연결/실패 피드백 → DB·테이블·필드 탐색 → SQL 입력·실행 → 결과 확인·복사·페이지 이동 → 취소·닫기.
- 연결되지 않은 상태, 빈 결과, 실행 오류, 탭 전환, 재시도도 확인한다. 실패를 무반응·빈 화면·이전 결과로 숨기지 않는다. 쿼리의 자동 재실행이나 자동 commit으로 문제를 덮지 않는다.
- SQL 파일 열기·저장·드래그앤드롭, 대상 DB 유지, 수정 내용 보존 등 기존 기본 동작을 새 화면이 깨뜨리지 않게 한다.
- 화면·단축키·포커스·진행 상태는 현재 보이는 작업을 가리켜야 한다. 취소와 닫기는 실제 작업 종료·리소스 정리까지 확인한다.
- 데이터 도구 화면은 좌측 정렬·조밀한 정보 배치를 기본으로 한다. 큰 중앙 안내, 장식용 아이콘, 중복 버튼·설명, 상시 노출된 내부 해시로 작업 공간을 낭비하지 않는다. 상세 정보는 필요할 때 펼친다.
- 새 UI의 주요 흐름은 네이티브 앱에서 직접 조작하고 화면을 확인한다. 모델/단위 테스트 통과만으로 GUI 검증이나 사용 가능 상태를 선언하지 않는다.
- 검증이 막히면 정확히 무엇이 미검증인지 기록한다. 실제 동작이 확인되지 않은 기본 기능을 완료로 보고하지 않는다.
- 사용자가 지적한 기본 결함은 재발 방지 대상으로 남긴다. 새 기능을 이어가기 전에 해당 결함이 다시 생기지 않았는지 변경 범위에 맞춰 확인한다.
