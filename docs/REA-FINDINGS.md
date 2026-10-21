# DataGrip 1차 REA 조사

조사일: 2026-10-21 KST. 대상: 공식 DataGrip 2026.2.6 / build `262.10968.148` / macOS ARM64.

## 결론과 범위

계획 수립에 필요한 **패키지 식별, launcher, DB/SQL 모듈 경계의 1차 정적 조사**를 수행했다.
REA CLI 실제 호출 5건과 별도 파일/ZIP 메타데이터 조사를 구별한다.
DataGrip 전체 기능의 내부 구현을 복원한 상태는 아니며 Java 메서드 디컴파일/런타임 관찰/성능 측정은 미수행.

기본 설치 경로 `/Applications`, 사용자 Applications, JetBrains Toolbox apps, Downloads의 제한된 탐색과
bundle ID Spotlight 조회에서 로컬 DataGrip을 찾지 못했다. 다른 모든 경로에 설치본이 없다는 증거는 아님.
분석 대상은 새로 받은 공식 배포본으로 고정했다. 앱 실행/사용자 DB 접속은 하지 않았다.

## 대상과 도구

| 항목 | 관찰 |
|---|---|
| 공식 release API | version 2026.2.6, build 262.10968.148, date 2026-09-29 |
| DMG 크기 | 1,027,873,598 bytes |
| DMG SHA-256 | `cf9f762e3b1bc802c0f096215d30cacfe3878fa48e12233cbdaf699588c7dbb2` |
| 검증 | 다운로드 파일 SHA-256와 JetBrains checksum 일치 |
| REA | npm `rea-agents@6.7.0`; Node 26.7.0 |
| 설치 skill | v44; release CLI expected skill v42. 실제 CLI schema/help 기준 실행 |
| MCP | active REA tools 없음; doctor에서 Codex 등록 missing. CLI로 분석 수행 |
| 분석 엔진 | Hopper/Ghidra/IDA/JADX/Kuna 미설정. 이번 메타데이터/ZIP/Mach-O 조사에는 불필요 |
| host | macOS 27.0, arm64; Rust 1.89.0 |

doctor의 skill identity missing은 setup-owned 등록 형식의 판정이다.
실제 `~/.codex/skills/reverse-engineer-anything/SKILL.md` v44는 존재하며 이를 삭제/덮어쓰지 않았다.
Codex 설정·다른 client 설정·분석 엔진 설치를 변경하지 않았다.

## 실제 REA Evidence

전체 ID/subject digest/파일 hash는 [index.json](../evidence/index.json). 원본은 로컬 `evidence/raw/`.

| 라벨 | operation / target | Evidence ID | 관찰 범위 |
|---|---|---|---|
| E01 | inspect_plist / DataGrip.app | `ev_8112d8de793151656d7f0bf5d7ab15dac7e2e73b71b590b8289fd57cf34953a8` | bundle ID `com.jetbrains.datagrip`, version/build, launcher 이름 |
| E02 | inspect_macho / Contents/MacOS/datagrip | `ev_4315962c7034038d2ab04a5592f52a24bf2ff4175edd86b3bed4ac77d0d61bde` | ARM64 Mach-O, load dependencies, entrypoint metadata |
| E03 | inspect_artifact / database-plugin.jar | `ev_504e85465b7b4a8fb272f62ee5750730897595135c477477e4c1033a3245efe4` | ZIP 중앙 디렉터리 inventory |
| E04 | inspect_artifact / intellij.database.connectivity.jar | `ev_095a4b05259004295403ad824434d6c3b07bad95d828eb1682c1a9ff17936127` | ZIP 중앙 디렉터리 inventory |
| E05 | inspect_artifact / intellij.database.sql.impl.jar | `ev_b2281ce65535603f9372a2035079cc15de233fd8eaa503ef604ce61c998d6464` | ZIP 중앙 디렉터리 inventory |

E03–E05의 `complete-within-substeps`는 수행된 ZIP inventory 범위의 완료일 뿐
Java 의미 분석/전체 애플리케이션 조사 완료를 뜻하지 않는다.
E02 import/export에는 stripping/toolchain 누락 가능성이 있다.
E01 subject의 digest는 launcher에 연결된 값이므로 plist 파일 전체 digest로 재해석하지 않는다.

## 별도 정적 조사 (REA Evidence 아님)

`scripts/inventory_datagrip.py`가 원본 앱 디렉터리의 product-info/ZIP entry/descriptor만 읽었다.
[package-inventory.json](../evidence/package-inventory.json)에 file hash와 재현 가능한 선택 규칙을 보존.

- 전체 `.jar` 파일 **973개**, 합계 **2,033,497,118 bytes**. 메모리 상주 크기와 다름.
- `product-info.json` layout entry **1,030개**. alias와 classPath 없는 entry 포함.
- `database|datagrip|datagrid|^intellij\.sql` 이름 선택: layout entry **118개**, unique JAR **118개**.
- 선택 JAR의 `.class` entry 합계 **28,999개**. 내부/익명 클래스·외부 라이브러리 포함; 고유 기능 개수가 아님.
- 초기 더 좁은 `database|sql|datagrid` 필터 결과는 115개. `product-summary.json`에는 이 초기 필터 결과를 보존;
  최종 보고 수치 118은 위 확장 선택 규칙으로 산출. 서로 같은 분모가 아님.
- `minRequiredJavaVersion: 25`, `mainClass: com.intellij.idea.Main`, bundled JBR java 경로 선언.
- `database-plugin.jar`의 `META-INF/plugin.xml`: plugin ID `com.intellij.database`, 이름 `Database Tools and SQL`.

## 관찰 → 설계상 의미 → 아직 모르는 것

| 관찰/근거 | 설계 추론 | 미확인 |
|---|---|---|
| product-info에 JBR/main class와 플랫폼 모듈 선언 | native launcher만 분석/번역해서 DB IDE를 재현할 수 없음 | 런타임의 실제 module loading/메모리 비중 |
| connectivity archive에 `DatabaseSessionManager`, `SeparateIntrospectionSessionRouter`, `JdbcEngine` 관련 class 이름 | 세션·metadata·실행을 분리하는 요구가 존재할 가능성; 공식 D03로 동작 계약 보강 | actor/lock/queue 실제 구현과 race 처리 |
| `JdbcEngine$OperationBase$CommitOrRollback`, `RollbackToSavepoint` 등 | 수동 transaction/savepoint가 별도 상태로 필요 | 실패 시 원래 코드가 어떤 순서로 복구하는지 |
| `CancellingDelegate`, `CancelProgressAuditor`, `DataRequest$Cancel` 후보 | 서버 취소와 UI/연결 종료를 구별해야 함; D06로 보강 | 메서드 본문·엔진별 cancel 세부 구현 |
| SQL core/impl의 completion/resolve/refactor class와 별도 dialect 모듈 | SQL parser만으로 IDE 기능 전체 충족 불가 | 각 dialect grammar/semantic 정확도와 비용 |
| `dialects.postgres/mysql/oracle/mssql/mongo/redis/...` 선언 | DB별 처리와 NoSQL별 모델 필요 | 전체 driver coverage/서버 버전 호환 |
| `impl.jcef`, diagrams/flameGraphs, SSH, AI/MCP module 선언 | UI/시각화/접속/확장 기능 backlog 유지 | 실제 활성 여부·어떤 화면이 JCEF를 쓰는지 |

클래스명 검색은 후보 추출이다. 예컨대 외부 지리 라이브러리의 `Grid`도 걸리므로
검색 count를 DataGrid 구현량으로 주장하지 않는다. 정확한 이름/descriptor/공식 동작을 조합해 경계만 추론.

## 다음 REA / 행동 대조 작업

| 우선순위 | 질문 | 다음 증거 |
|---|---|---|
| M0 | 실제 느린 구간은 시작/입력/그리드/DB/스키마 중 어디인가 | 같은 fixture runtime timings + CPU/메모리 프로파일 |
| M0 | 취소·다중 결과·세션 보존의 사용자 관찰 계약 | 격리된 PG에서 query/cancel/temp table 시나리오 |
| M2 | grid submit/rollback/concurrent update 동작 | fixture의 DB 변화와 화면 상태 대조 |
| M3 | alias/CTE/search_path/refactor 경계 | dialect별 정답 SQL corpus, preview/적용 결과 |
| 필요 시 | 문서/런타임으로 설명되지 않는 구현 분기 | 선택 JAR·클래스·메서드의 로컬 JVM bytecode 도구 분석 |

REA release의 `inspect-managed-artifact`는 PE/CLI(.NET)용이며 JVM JAR 디컴파일로 사용하지 않는다.
native 분석 엔진을 설치해 Java 도메인 로직을 자동 복원할 수 있다고 가정하지 않는다.

## 재현 명령

작업 디렉터리는 프로젝트 root. `tmp/`와 `evidence/raw/`는 Git 제외.

```sh
# 공식 파일 취득 후 checksum 확인
curl -fL 'https://download.jetbrains.com/datagrip/datagrip-2026.2.6-aarch64.dmg' -o tmp/datagrip-2026.2.6-aarch64.dmg
shasum -a 256 tmp/datagrip-2026.2.6-aarch64.dmg
cat evidence/datagrip.sha256

# 읽기 전용 mount; 실행/설치하지 않음
hdiutil attach -readonly -nobrowse -mountpoint "$PWD/tmp/datagrip-volume" tmp/datagrip-2026.2.6-aarch64.dmg

npx -y rea-agents@6.7.0 inspect-plist "$PWD/tmp/datagrip-volume/DataGrip.app" --format json
npx -y rea-agents@6.7.0 inspect-macho "$PWD/tmp/datagrip-volume/DataGrip.app/Contents/MacOS/datagrip" --format json
npx -y rea-agents@6.7.0 inspect-artifact "$PWD/tmp/datagrip-volume/DataGrip.app/Contents/plugins/DatabaseTools/lib/database-plugin.jar" --format json
npx -y rea-agents@6.7.0 inspect-artifact "$PWD/tmp/datagrip-volume/DataGrip.app/Contents/plugins/DatabaseTools/lib/modules/intellij.database.connectivity.jar" --format json
npx -y rea-agents@6.7.0 inspect-artifact "$PWD/tmp/datagrip-volume/DataGrip.app/Contents/plugins/DatabaseTools/lib/modules/intellij.database.sql.impl.jar" --format json
uv run --python '>=3.14' scripts/inventory_datagrip.py tmp/datagrip-volume/DataGrip.app evidence/package-inventory.json

hdiutil detach "$PWD/tmp/datagrip-volume"
```

실제 조사는 REA 전체 출력을 각각 `evidence/raw/rea-*.json`으로 redirect해 보존했다.
macOS 27은 hdiutil attach deprecation 안내를 출력했으나 읽기 전용 mount는 성공했다.
