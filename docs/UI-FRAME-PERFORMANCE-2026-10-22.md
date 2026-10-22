# Native UI frame observations — 2026-10-22

긴 셀의 화면·접근성 미리보기를 256 escaped UTF-8 bytes로 제한했다. 생략 표시와 원본 byte 길이를 제공하며, 실제 결과·복사·파일 export는 원문을 유지한다. 열 이름 표시도128bytes로 제한한다. NULL, 빈 문자열, 문자열 `NULL`은 접근성 설명으로 구분한다. codepoint 경계는 유지하지만 grapheme 단위 절단은 아니다.

**이번 표본에서 입력 지연은 소폭 낮았으나 draw p95는 개선되지 않았다. 전체 앱이 빨라졌다는 근거로 쓰지 않는다.** 확인된 효과는 원문 크기에 비례하던 표시/AX 문자열 생성을 제한한 것이다.

## 측정 범위

Apple M5 /24GiB /macOS27.0 /AC, release `ui-profiler` 빌드. GPUI0.3.9의 native profiler histogram을 사용했다. 같은 실행 파일에서 `DBIDE_UI_RAW_GRID=1`로 기존 원문 표시 경로, 환경변수 없이 제한 미리보기 경로를 실행했다. 기본 제품 빌드는 profiler와 원문 비교 경로를 활성화하지 않는다.

각 구성에서 SQLite 한 행을 실제 조회한 뒤 SQL 주석에 native scripted `x` 입력30회, 매 입력 후 AX 관찰. reset/report 키 입력도 포함되어 histogram 표본은31–32개다. 각 구성1회, 기존 경로3구성→제한 경로3구성 순서이며 순서를 교차하거나 통계적 반복을 수행하지 않았다. 다른 빌드/벤치마크는 입력 측정 중 멈췄고, 기존 데스크톱 앱·OS 캐시는 유지했다.

| 구성 | 입력→플랫폼 제출 p95 기존 / 제한 | draw p95 기존 / 제한 | 입력 프레임 기존 / 제한 |
|---|---:|---:|---:|
| 1KiB 셀, 2열 | 8.70 /8.12ms | 2.27 /2.43ms |31 /31|
| 128KiB 셀, 2열 | 9.87 /8.63ms | 2.55 /2.66ms |31 /32|
| 64열, 각2KiB | 10.61 /10.21ms | 3.00 /3.41ms |31 /31|

입력 수치는 GPUI가 관측한 입력부터 `platform_window.draw` 반환까지다. GPU 완료·물리 화면 표시·키보드 하드웨어·한글 IME 지연이 아니다. coalesced 입력과 mid-draw 제외가 존재한다. draw histogram에는 커서 등 다른 redraw도 포함되며64열은 가상화된 표시 범위만 그린다. 화면 주사율/FPS로 해석하지 않는다.

각 query의 `grid_install_to_root_paint_ms`도 별도 원시 기록했다. 이는 결과 설치→root canvas paint이며 query 시작·wire 첫 행·실제 픽셀 표시가 아니다. 구성별1표본이고 연결/스케줄링 조건이 달라 비교 결론을 내리지 않는다. 200KiB SQLite 셀은 기존 문자열 비용3배 계산 때문에512KiB 보관 상한을 초과하여0행으로 잘렸으므로 비교에서 제외했다.

## 보존 검증

- preview unit6개: Unicode 경계, 제어문자, NULL/빈 문자열, 정확한 상한,1.2MB 원문에 대한 제한된 출력.
- 네이티브128KiB 셀에서 AX 생략·원본 크기 표시 확인.
- 값 검사기의 원문 복사→새 TextEdit scratch에 붙여넣기 후131,072개의 `x` 전부 보존 확인. 검사기 화면 자체의 기존64KiB 미리보기 한계는 유지한다.
- 독립 읽기 전용 검토: 원본 ResultPage를 사용하는 복사/export/검사기 경로와 GPUI 계측 단위 확인.

## 재현

```sh
cargo build --release -p dbide-ui-spike --features ui-profiler
# 격리된 생성 profile 파일을 지정한 상태에서 UI 실행.
DBIDE_PROFILE_STORE=/absolute/owned/profiles.json target/release/dbide-ui-spike
# 동일 binary의 비교 경로:
DBIDE_PROFILE_STORE=/absolute/owned/profiles.json DBIDE_UI_RAW_GRID=1 target/release/dbide-ui-spike
```

편집기를 선택하고 query 실행 후 `⌘⌥⇧M`으로 histogram 초기화, 입력 시퀀스 수행, `⌘⇧M`으로 stdout JSON 기록. `ui_profiler` 아래 histogram은 count/p50/p95/max와 원시 bucket을 포함한다. 정상 제품 재패키징은 `bash scripts/package-ui.sh`로 profiler 없이 수행한다.

[원시 histogram·SQL·실행 파일 SHA-256·측정 순서](../evidence/performance-2026-10-22-ui-frames.json). 측정 binary는 기본commit `3a57364`에 미커밋 UI 변경을 더한 빌드다. 이후 tooltip 문구/format 정리와 SQLite export 검토 수정이 있으므로 최종 제품 바이너리와 hash를 혼동하지 않는다.

다음 성능 게이트: 반복·순서 교차 입력 측정, native scroll/physical IME, metadata burst·32탭 복구. PostgreSQL/MySQL progressive fetch, GUI 전체 export는 이번 SQLite CLI 스트리밍과 별개 미완료다.
