# Native CLI

Folder workspaces: `dbide workspace init/show/files/read/write/mkdir/bind/context/plan --root PATH`.
Agent discovery: `dbide agent capabilities`. These return versioned JSON and do not connect.
See [WORKSPACES.md](WORKSPACES.md) for exact create-only/CAS write and planned-query contracts.
`query` also accepts `--database`, `--expect-sql-sha256` and `--expect-target-fingerprint` to reject changed inputs before authentication/dispatch.

`dbide-cli`는 GUI와 같은 `dbide-profiles` 저장소, `DatabaseBridge`, PostgreSQL /
SQLite / MySQL native adapter를 사용한다. JVM·GUI 실행 없이 동작한다. 속도 우위는
아직 측정하지 않았다. 현재는 bounded 결과를 반환하는 CLI이며 무제한 streaming dump는 아니다.

## 바로 실행

```sh
cargo build -p dbide-cli --release
./target/release/dbide --help

# 명시한 새 설정 파일만 사용. add는 연결하거나 SQLite 파일을 만들지 않는다.
./target/release/dbide --config ./tmp/cli-demo/profiles.json profiles add \
  --id demo --name 'SQLite demo' --engine sqlite \
  --sqlite-path ./tmp/cli-demo/demo.db --sqlite-create
./target/release/dbide --config ./tmp/cli-demo/profiles.json profiles select demo
./target/release/dbide --config ./tmp/cli-demo/profiles.json query \
  --profile demo --sql "SELECT 1 AS id, NULL AS absent, '' AS empty;" --format json
./target/release/dbide --config ./tmp/cli-demo/profiles.json query \
  --profile demo --sql 'CREATE TABLE example(id INTEGER PRIMARY KEY, name TEXT);'
./target/release/dbide --config ./tmp/cli-demo/profiles.json schema --profile demo
```

`--config PATH`는 모든 위치에서 사용할 수 있는 global option. 생략하면 GUI와
동일한 platform 기본 파일을 사용한다. macOS는
`~/Library/Application Support/RustDatabaseIDE/profiles.json`, Windows는
`%APPDATA%/RustDatabaseIDE/profiles.json`, Unix는
`$XDG_CONFIG_HOME/RustDatabaseIDE/profiles.json` 또는
`~/.config/RustDatabaseIDE/profiles.json`이다. 테스트는 항상 임시 `--config`를 지정하며
사용자의 기본 설정·DB·Keychain을 읽지 않는다.

## 프로필과 설정

```sh
# ID는 불변. add의 --id 생략 시 UUID 생성 후 stdout 출력.
dbide --config ./profiles.json profiles list
dbide --config ./profiles.json profiles add --id local-pg --name 'Local PostgreSQL' \
  --engine postgres --host localhost --port 5432 --database postgres \
  --username postgres --tls verify-full --connect-timeout-secs 10
# --tls disable / require / verify-full. 기본 verify-full.
# custom CA: --root-cert-path /absolute/path/ca.pem
# MySQL: --engine mysql (새 프로필의 기본 port=3306, database=mysql, username=root)
dbide --config ./profiles.json profiles edit local-pg --name 'Development' --port 5433
dbide --config ./profiles.json profiles select local-pg
dbide --config ./profiles.json profiles test local-pg --password-env MY_DB_PASSWORD
# 삭제 직전 원본 파일 전체를 같은 디렉터리의 profiles.json.backup-*.json에 보존.
dbide --config ./profiles.json profiles remove local-pg

dbide --config ./profiles.json config show
dbide --config ./profiles.json config set persist_history true
dbide --config ./profiles.json config set persist_history false

# GUI와 공유하는 언어 설정. get/set 모두 ko 또는 en 한 줄 출력.
dbide --config ./profiles.json config language get
dbide --config ./profiles.json config language set ko
dbide --config ./profiles.json config language set en
```

`profiles list`와 `config show`는 JSON. `config show`는 실제 shared schema 전체를 반환한다.
현재 `config set` 대상은 실제 존재하는 `persist_history` 하나다. 연결 기본값은
프로필의 host/port/database/username/TLS/timeout 필드로 편집한다.
`config language`는 공통 `dbide-settings` store로 선택한 profiles 파일의 부모 디렉터리
`app-settings.json`을 읽고 쓴다. `--config`를 생략하면 기본 profiles 경로의 같은 부모를 쓴다.
설정 파일이 없을 때 `get`은 파일 생성 없이 `ko`를 반환한다. `set`은 `ko`/`en`만 허용하며
다른 설정 필드를 보존하고 atomic 저장한다. malformed/future 설정은 원본을 보존하고 exit 2.
언어 명령은 profiles JSON을 읽거나 DB에 연결하지 않으며 `persist_history` 값을 바꾸지 않는다.
설정한 locale은 GUI와 공유한다. 현재 CLI의 help/diagnostic/SQL 데이터는 영어/원문을 유지한다.

add/edit는 공통 validator를 사용하며 duplicate ID·잘못된 port·engine·TLS·timeout을 거절한다.
edit의 boolean은 `--read-only=false`, `--sqlite-create=false`로 끌 수 있다.
`--root-cert-path ''`로 custom CA를 지운다. 새 SQLite 상대 경로는 저장 시 절대 경로로
해석한다 (`:memory:` 제외). 존재하지 않는 SQLite 파일 생성은 `--sqlite-create`가 필요하며,
생성 권한과 별개로 부모 디렉터리가 존재해야 한다.

`profiles test`는 명시적 연결 검사이며 결과 세션을 닫는다. SQLite profile에 create가
켜져 있으면 검사가 빈 DB 파일을 만들 수 있다. `schema`도 명시한 catalog 조회를 실행한다.

저장은 공통 atomic store를 사용한다. malformed/unsupported 설정은 에러로 종료하고
원본을 덮어쓰지 않는다. GUI와 CLI는 공통 `ProfileStore::update`와 sibling lock file로
최신 설정의 read/modify/write를 직렬화한다. 같은 profile ID를 동시에 편집하면 마지막
writer가 해당 profile의 내용을 결정하며, 서로 다른 profile이나 설정은 유지한다.
remove는 프로필·선택·해당 프로필 history를 제거하지만 Keychain entry는 삭제하지 않는다.
복구하려면 backup JSON을 원래 설정 경로로 명시적으로 복사한다.

## 자격 증명

query / schema / profiles test에서 다음 중 하나를 명시한다.

- `--password-env NAME`: 정확히 그 environment variable만 읽는다. 값이 없거나 UTF-8이 아니면 실패.
- `--password-stdin`: stdin 전체를 최대 64 KiB로 읽고 마지막 LF 또는 CRLF 하나만 제거.
- `--keychain`: 앱의 `com.local.rust-database-ide.credentials` 서비스와 해당 profile ID의 정확한 entry만 읽음. macOS 전용.

선택하지 않으면 빈 password로 연결한다. 일반 `--password VALUE`, credential URL,
자동 environment 탐색, 다른 앱 credential discovery는 제공하지 않는다.
세 option은 서로 배타적이며 SQL `--stdin`과 password `--password-stdin`도 함께 사용할 수 없다.
password를 profile JSON/history/로그에 저장하지 않는다. 이 CLI는 Keychain을 쓰거나 삭제하지 않는다.
SQL 문장 자체에 적힌 민감 정보는 SQL 결과나 사용자가 opt-in한 history에 포함될 수 있다.
SQL 완료 후 history 저장만 실패하면 결과와 원래 execution exit code를 유지하고 stderr에
`SQL already executed; history persistence failed` 경고를 낸다. history를 복구하려고 SQL을
다시 실행하지 않는다.
history의 `execution_target`은 실제 제출한 profile ID·engine·database와 endpoint fingerprint를
보존한다. SQLite database는 `main`, 서버 엔진은 제출 당시 프로필의 database다.
`--read-only`를 포함한 실제 session 설정으로 capture하며, 이후 프로필 선택·endpoint 편집이
과거 실행 대상을 바꾸지 않는다. SQL script 안의 `USE` 등으로 바뀐 실행 중 상태를 추론하지 않는다.

## SQL 실행과 결과

```sh
dbide --config ./profiles.json query --profile dev --file ./query.sql --format json
printf 'SELECT 1;' | dbide --config ./profiles.json query --profile dev --stdin --format csv
# 명시적으로 선택한 프로필이 있으면 --profile 생략 가능.
dbide --config ./profiles.json query --sql 'SELECT 1;' --format table --max-rows 50
# 새로운 파일만 허용. 기존 파일 교체는 --force를 명시해야 함.
dbide --config ./profiles.json query --profile dev --sql 'SELECT 1;' \
  --format csv --output ./result.csv
# 여러 result set에서 두 번째만 CSV로 선택 (zero-based).
dbide --config ./profiles.json query --profile dev --sql 'SELECT 1; SELECT 2;' \
  --format csv --result-index 1
# 이 invocation의 adapter session read-only 설정. 저장된 프로필은 수정하지 않음.
dbide --config ./profiles.json query --profile dev --sql 'SELECT 1;' --read-only --timeout-secs 15
```

SQL source는 `--sql`, `--file`, `--stdin` 중 정확히 하나. SQL은 비어 있으면 안 되며
UTF-8 65,536 bytes 이하만 허용한다. 초과 입력은 잘라 실행하지 않고 dispatch 전에 거절한다.
공통 bridge의 configure → 필요 시 자동 연결 → SQL **한 번 제출** 순서다.
실패·unknown outcome 이후 자동 재실행하지 않는다. 실행 SQL의 parser 성공을 요구하지 않는다.

진행 메시지·notice·경고는 stderr. stdout은 선택한 형식만 출력한다.

- JSON: bridge `ExecutionReport` 전체. 모든 retained result set, command count, error,
  notice, outcome, elapsed, transaction state, truncation을 포함한다. row 값은 driver의
  text/null 계약이며 숫자 타입을 추정하지 않는다. 중복 column 이름도 positional 배열로 보존.
- CSV: 한 result set만 표현한다. 여러 set이면 `--result-index` 필수. command count/error
  전체가 필요하면 JSON 사용. 이름과 값은 순서를 유지하며 formula escape/type coercion을
  하지 않는다. NULL 기본 marker는 `\N`; empty text는 empty CSV field다.
  실제 text가 marker와 충돌하면 거절하고 `--null-marker VALUE`로 바꿀 수 있다.
- table: set별 기본 100행, `--max-rows 1..4096`. column 앞에 위치 index를 표시한다.
  text는 JSON quote로 표시해 NULL·empty string·tab/newline을 구별한다.
  cell display는 256 Unicode 문자 후 명시적으로 생략 표시; 원문 export는 JSON/CSV 사용.
  table display row cap은 export/DB 실행 row limit이 아니다.

현재 공통 driver retention cap은 **512 KiB / 4096 events**, SQL cap은 **64 KiB**다.
CLI에서 retention cap을 무한으로 늘리는 option은 제공하지 않는다. driver truncation은
stderr 경고·JSON `truncated:true`·exit 5로 드러난다. schema는 공통 catalog query의
retention cap과 최대 2048 metadata rows 제한을 따른다. table에서만 숨긴 retained row는
안내 문구를 출력하고 정상 종료한다.

`--output`은 같은 디렉터리의 private temporary file에 완전히 encoding·sync한 뒤 atomic
publish한다. 부모 디렉터리는 이미 존재해야 하며 쓰기 가능 여부도 SQL dispatch 전에
검사한다. 기본 no-clobber; `--force`일 때만 교체한다. SQL/encoding/truncation 실패 시
기존 output을 보존하고 새 output을 publish하지 않는다. stdout은 실패한 SQL의 partial
retained JSON/table report를 출력할 수 있으므로 exit code를 함께 확인해야 한다.
CSV의 multiple-set/marker 충돌은 실행 결과를 알아야 판정된다. 해당 export 실패가 SQL의
이미 발생한 변경을 되돌리지는 않는다.

`--timeout-secs 1..600` (기본 30) 또는 Ctrl+C 시 현재 operation ID만 취소한다.
이후 disconnect event를 최대 9초 기다리고 worker shutdown을 요청한다.
서버에 도달한 SQL은 commit되었을 수 있으므로 timeout을 자동 retry 근거로 사용하지 않는다.
`--read-only`는 adapter session 설정이며 DB 계정 권한을 대체하지 않는다.

## 오프라인 SQL format

```sh
dbide sql format --file ./query.sql --dialect postgres
printf 'select 1 as value;' | dbide sql format --stdin --dialect sqlite
# stdout 대신 새 파일: --output ./formatted.sql (교체 시 --force)
```

`postgres`, `sqlite`, `mysql` dialect. 공통 `dbide-sql::format_sql`을 사용한다.
설정 파일·DB·Keychain에 접근하지 않는다. parser가 지원하지 않는 SQL은 오류로 보고한다.

## 저장된 catalog 일부 조회

```sh
dbide --config ./profiles.json catalog list
# 목록의 shard_id를 그대로 복사해 SHARD_ID에 지정한 뒤:
dbide --config ./profiles.json catalog show "$SHARD_ID"
```

이 명령은 공통 `dbide-catalog-store`를 사용해 선택한 profiles 파일 부모의
`catalog-cache/`만 읽는다. `--config` 생략 시 기본 profiles 경로의 같은 부모다.
profiles JSON 자체·DB·인증 정보·Keychain에 접근하지 않고, refresh나 SQL을 실행하지 않는다.
cache가 없으면 `list`는 `[]`를 반환하며 디렉터리나 파일을 생성하지 않는다.

- `list`: 최대 2 MiB / 2048 entries의 bounded identity manifest만 읽는다. 각 항목은
  `shard_id`, `key`, `bytes`, `captured_unix_ms`. 개별 shard payload를 읽거나 전체 디렉터리를
  스캔하지 않으므로 관련 없는 shard가 손상되어도 목록과 다른 shard 조회는 가능하다.
- `show SHARD_ID`: manifest에 있는 정확한 소문자 64자리 hex + `.json` ID 하나를 선택해
  최대 1 MiB의 shard 하나만 읽는다. JSON의 `key`, `snapshot`, `historical:true`로 반환한다.
  scope는 database 목록, schemas, 특정 schema의 objects, 특정 relation의 columns 중 하나다.
  `snapshot.captured_unix_ms`, `truncated`, `fields_partial`을 그대로 유지한다.

결과는 저장 당시의 metadata이며 현재 서버 상태나 freshness를 보증하지 않는다. query row나
SQL history를 읽는 명령도 아니다. 경로·상대경로·변형 ID·없는 shard는 exit 2로 거절한다.
손상·미지원 manifest/shard, 비공개 권한 조건 위반, symlink 경로도 진단 후 원본을 보존한다.
Store 정책상 cache 디렉터리는 Unix 0700, 파일은 0600이며 symlink ancestor를 허용하지 않는다.
GUI가 저장하는 disk cache 총량 cap은 128 MiB다. 이 CLI 명령에는 쓰기·삭제 기능이 없다.

## Exit codes와 검증

| code | 의미 |
|---|---|
| 0 | 완료 |
| 2 | 인자/입력/설정/encoding/파일 저장 오류 |
| 3 | 연결 실패 |
| 4 | SQL/bridge 실패, unknown outcome, operation timeout |
| 5 | 쿼리/schema 완료했으나 retained 결과가 잘림 |
| 130 | Ctrl+C 또는 adapter cancellation |

```sh
cargo test -p dbide-cli
cargo clippy -p dbide-cli --all-targets -- -D warnings
# 아래는 프로젝트 소유 loopback fixture를 별도로 시작한 뒤에만 실행.
DBIDE_CLI_LIVE_FIXTURE=owned-loopback cargo test -p dbide-cli postgres_owned_fixture -- --ignored
DBIDE_CLI_LIVE_FIXTURE=owned-loopback cargo test -p dbide-cli mysql_owned_fixture -- --ignored
```

기본 subprocess 계약 테스트는 임시 config와 임시 SQLite 파일만 사용한다. PG/MySQL
ignored 테스트는 127.0.0.1:55439 / :55441의 프로젝트 fixture와 고정 fixture 계정만 사용한다.
기본 suite 통과는 live PG/MySQL 테스트를 실행했다는 뜻이 아니다.

이번 구현 검증: 기본 suite **18 passed / 2 ignored**, 별도 명시 실행한 PostgreSQL fixture
**1 passed**, MySQL fixture **1 passed**. 두 native 엔진 모두 JSON의 두 result set과
positional NULL/empty string, `--result-index 1` CSV 원문을 검사했다. 기본 suite에는
8개 CLI child process의 동시 profile 추가 보존, timeout/Ctrl+C와 export 실패 전후의
기존 파일 보존 검사도 포함한다. 실제 SQL 완료 뒤 history 파일 크기 제한 저장 실패가
발생해도 stdout/file report·원래 SQL exit·단일 실행을 유지하는 회귀도 검사한다.
Clippy `--all-targets -- -D warnings` 통과.
기본 suite의 offline catalog subprocess 계약 4개는 missing cache 무생성, private Store roundtrip,
manifest-only listing, 단일 shard 부분 읽기, traversal 거절, 손상 원본 보존을 검사한다.
