# Rust Database IDE

DataGrip의 주요 작업 흐름을 Rust 데스크톱 앱으로 독립 재구현하는 프로젝트.
목표는 작은 SQL 실행기에 머무르지 않고 편집·탐색·의미 분석·마이그레이션·다중 DB까지 확장하는 것.

## 현재 상태 — 2026-10-22

- 공개 소스 저장소: [gudcks0305/rust-database-ide](https://github.com/gudcks0305/rust-database-ide). 서명된 제품 배포는 아직 없음.
- 공식 DataGrip 2026.2.6 macOS ARM64 배포본 SHA-256 확인.
- REA 6.7.0으로 1차 정적 분석, 별도 패키지 메타데이터 조사 완료.
- 기능 목록, Rust 구조, 구현 순서, 성능/정확성 통과 기준 작성.
- Rust native UI, PostgreSQL/SQLite/MySQL, 일반 연결 프로필, SQL 파일 탭, transaction 제어, 자동완성, CLI 구현.
- 탭별 전용 세션·연결 시작 DB 선택, 한국어/영어 Preferences와 CLI 언어 설정, 연결 없이 SQL·대상 복구. 실제 PG 탭 간 트랜잭션·취소 격리와 언어 재시작 유지 확인.
- 행 편집·schema tools·Codex WebSocket 연동을 통합 검증 중. [최신 기능별 상태와 한계](docs/CURRENT-STATUS.md).
- **제품 전체·M1–M6 전체 완성·DataGrip 성능 우위는 아직 달성하지 않음.**

데이터베이스 트리의 지연 조회·캐시 연동과 도구 패널 한·영 전환을 반영했다.
테이블 화면 통합 검증, 자동완성 메타데이터 갱신, 키보드 작업 흐름을 계속 보강 중이다.
[UX 조사](docs/UX-RESEARCH.md)와 [제품 backlog](docs/PRODUCT-BACKLOG.md)에
구현 상태와 검증 조건을 분리해 기록한다.

## 읽는 순서

1. [구현 계획](docs/PLAN.md) — 결정, 단계, 첫 작업, 완료 조건
2. [REA 조사 결과](docs/REA-FINDINGS.md) — 관찰/추론/미확인 분리
3. [기능 범위](docs/FEATURE-MATRIX.md) — 최종 범위와 단계별 지원 기준
4. [구조](docs/ARCHITECTURE.md) — 세션, 실행 스트림, SQL 모델, UI 선택
5. [성능 검증](docs/BENCHMARKS.md) — 수치 목표와 재현 조건
6. [출처](docs/SOURCES.md) / [증거 색인](evidence/index.json)
7. [최신 구현/검증 현황](docs/CURRENT-STATUS.md) / [초기 M0 기록](docs/M0-RESULTS.md)

## 범위 해석

제품의 UI/도메인/실행 계층은 Rust. 기본 실행에 JVM 불필요.
OS API, SQLite, 구문 분석기 등 검증된 C/C++ FFI까지 금지한다는 뜻은 아님.
macOS ARM64 우선. Windows/Linux는 후속 검증 대상.
JDBC 전용 드라이버를 그대로 로드하는 Java bridge는 기본 설계에 넣지 않음.
DB별 호환성은 기능 행렬과 실제 테스트로 선언한다.

## 재검증

```sh
uv run --python '>=3.14' scripts/verify_project.py
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

패키지 조사 재실행 명령과 REA 버전은 [조사 결과](docs/REA-FINDINGS.md)에 기록.
`tmp/`의 배포본과 `evidence/raw/`의 원본 도구 출력은 로컬 전용이며 Git에서 제외.
Rust 1.99.1을 프로젝트에 고정. 전역 기본 toolchain은 변경하지 않는다.
실제 PG 테스트는 `cargo test --workspace`에서 ignored로 표시되며 별도 명시 실행해야 한다.

## Native 앱

```sh
bash scripts/package-ui.sh
open 'outputs/Rust Database IDE.app'
```

앱에서 `⌘,` → **환경설정 / Preferences** → **언어 / Language**로 한국어·영어를
즉시 전환한다. 선택은 재시작 후에도 유지되며 SQL·DB 이름·조회 값은 번역하지 않는다.
CLI에서도 같은 설정을 바꿀 수 있다:

```sh
cargo run -p dbide-cli --bin dbide -- config language set ko
cargo run -p dbide-cli --bin dbide -- config language set en
```

별도 프로필 파일을 쓰면 GUI와 CLI에 동일한 설정 위치를 지정해야 한다.
CLI의 도움말과 원본 DB 오류, 일부 공용 UI 구성 요소는 아직 영어다.

## 격리 PostgreSQL fixture

Docker Desktop이 실행 중이어야 한다. 이 fixture는 로컬 loopback 포트만 사용한다.
아래 계정은 이 프로젝트가 생성하는 테스트용이며 실제 사용자 DB 계정이 아니다.

```sh
bash scripts/fixture-pg.sh start
docker exec -i dbide-m0-pg psql -U postgres -d dbide_fixture -v ON_ERROR_STOP=1 < fixtures/postgres/baseline.sql
export DBIDE_TEST_PG_URL='postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture'
cargo test -p dbide-pg-spike --test postgres_contract -- --ignored
cargo test -p dbide-bridge --test fixture_contract -- --ignored
# 사용 후 fixture만 중지:
bash scripts/fixture-pg.sh stop
```

현재 자동 검증은 `bash scripts/verify-current.sh`, 준비된 owned fixture의 실제 DB 검증까지 포함하려면 `bash scripts/verify-current.sh --owned-live`.
GUI/IME/VoiceOver와 DataGrip 비교 측정은 별도. `scripts/verify-m0.sh`는 초기 실험 보고서 재생성용이다.
GitHub macOS ARM64 자동 검사 초안은 [CI 문서](docs/CI.md)에 있다. 현재 OAuth의 `workflow` 권한이 없어 원격 CI는 아직 활성화되지 않았다.

## CLI

```sh
cargo run -p dbide-cli --bin dbide -- --help
```

프로필·설정·쿼리·schema·파일 export 명령은 [CLI 가이드](docs/CLI.md)에 정리했다.
GUI와 같은 설정을 쓰며 비밀번호는 명시한 환경변수/stdin/Keychain으로만 제공한다.

## 라이선스와 외부 코드

프로젝트 전체의 통일된 라이선스는 아직 지정하지 않았다. 패키지별 선언과
vendored dependency의 라이선스·출처는 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)를 참고한다.
DataGrip의 코드·JAR·아이콘·배포본은 이 저장소에 포함하지 않는다.
