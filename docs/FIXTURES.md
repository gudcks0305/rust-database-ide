# 격리 DB fixture 재현

2026-10-21 기준. 사용자 프로필·실제 DB를 읽지 않는 프로젝트 전용 Docker
fixture다. `scripts/fixture-engines.sh`가 고정 이름, 정확한 `owner` label,
고정 image ID, loopback publish 설정을 검증한다. TLS PostgreSQL은 읽기 전용
인증서 bind mount도 검증한다. 다른 owner나 예상 밖 설정이면 동작을 거부한다.
원격 Docker context는 사용하지 않고 로컬 Unix socket context만 허용한다.

| Target | Container / owner | Host endpoint | Image |
|---|---|---|---|
| `pg` | `dbide-m0-pg` / `rust-database-ide-m0` | `127.0.0.1:55439` | PostgreSQL 18.6 Alpine |
| `pg-tls` | `dbide-tls-pg` / `rust-database-ide-tls` | `127.0.0.1:55440` | PostgreSQL 18.6 Alpine |
| `mysql` | `dbide-mysql` / `rust-database-ide-mysql` | `127.0.0.1:55441` | MySQL 8.4.11 |

모두 database는 `dbide_fixture`, 테스트 전용 password는
`dbide_fixture_only`. PostgreSQL user는 `postgres`, MySQL user는 `root`.
이 계정은 격리된 생성 데이터용이다. 앱의 일반 PostgreSQL 연결은 기본적으로
VerifyFull을 사용하며, 평문 55439 fixture는 명시적으로 TLS Disable을 선택한다.

고정 registry digest:

```text
postgres:18-alpine@sha256:77f585114c32fbca283dc835b0596f4e52b51b4c6662d7810b2f4084f60a1873
mysql:8.4@sha256:0f51e63e537a1f176423fa03ef559aabe6fa4372a53160a44bc7e75da7d15364
```

Docker의 `.Image`는 해당 플랫폼의 config digest이므로 registry manifest
digest와 문자열이 다르다. 스크립트는 고정 registry 참조를 로컬 image ID로
해결한 뒤 실제 container image ID와 비교한다.

## 시작·상태·중지

프로젝트 root에서 실행한다. Docker Desktop과 OpenSSL 필요.

```sh
bash scripts/fixture-engines.sh status
bash scripts/fixture-engines.sh start all
bash scripts/fixture-engines.sh status pg-tls
bash scripts/fixture-engines.sh status mysql
# 테스트 종료 후 선택적으로 중지:
bash scripts/fixture-engines.sh stop all
```

기본 동작은 `status`; start/status/stop의 기본 target은 `all`이다.
`pg`, `pg-tls`, `mysql`을 개별 지정할 수 있다. 실행 중인 fixture의 start는
health check만 수행한다. 재시작·TLS reload·seed를 수행하지 않는다.
`pg`의 실제 시작·중지는 기존 `scripts/fixture-pg.sh`에 위임한다.

stop은 container만 중지하고 data volume을 유지한다. 이후 start는 같은
container와 data를 재사용한다. container/volume 삭제, prune, 데이터 초기화
옵션은 없다. 현재 container 설정이 검증과 다르면 자동 교체하지 않는다.
저장소를 다른 경로로 옮기면 기존 TLS bind mount가 달라질 수 있으며, 이
경우도 검증 실패로 중단한다.

status는 read-only 검사다. PostgreSQL version, TLS PG의 실제 세션 `ssl=t`,
MySQL version/TLS cipher를 확인한다. TLS PG는 `sslmode=verify-full`, MySQL은
`VERIFY_IDENTITY`로 localhost/127.0.0.1 identity를 검증한다. MySQL의 인증서가
host fixture 인증서와 같은지도 확인한다.

TLS PostgreSQL은 host `tmp/tls-fixture`를 `/tls:ro`로 mount한다. 시작 시
`server.pem`과 `server.key`를 container `/tmp`로 복사하고 postgres 소유·0600으로
설정한 뒤 SSL을 켠다. MySQL은 새로 시작한 owned container의 data directory에
CA/server certificate/key를 설치하고 mysql 소유·private key 0600을 설정한다.
`ALTER INSTANCE RELOAD TLS` 후 검증된 TLS health check가 성공해야 완료된다.
기존 실행 중인 container의 인증서는 start 명령으로 바꾸지 않는다.

## 인증서 수명과 명시적 갱신

현재 canonical 파일명:

```text
tmp/tls-fixture/ca.pem
tmp/tls-fixture/ca.key
tmp/tls-fixture/server.pem
tmp/tls-fixture/server.key
```

SAN은 `DNS:localhost`, `IP:127.0.0.1`. 앱/테스트의 root CA 경로는
`tmp/tls-fixture/ca.pem`이다. CA private key는 container로 복사하지 않는다.
OS trust store도 변경하지 않는다.

인증서 세트가 전혀 없을 때만 최초 start가 새로 생성한다. 일부 파일만
남아 있거나 인증서가 만료·불일치하면 자동 덮어쓰기를 거부한다. 생성 시
새 CA는 3,650일, server certificate는 365일 유효하다. 기존 인증서는 기간이
짧아도 보존한다. 2026-10-21에 확인한 기존 server certificate는
2026-10-23 13:20 UTC 만료이며, 이 스크립트를 추가하면서 교체하지 않았다.

갱신은 테스트가 끝난 뒤 명시적으로 수행한다.

```sh
bash scripts/fixture-engines.sh stop pg-tls
bash scripts/fixture-engines.sh stop mysql
bash scripts/fixture-engines.sh renew-certs
bash scripts/fixture-engines.sh start pg-tls
bash scripts/fixture-engines.sh start mysql
```

pg-tls/mysql이 실행 중이면 갱신을 거부한다. 이전 인증서·키는 권한 제한된
`tmp/tls-backup.*`에 복사해 보존하고, 새 생성 원본·로그도 `tmp/tls-generation.*`에
보존한다. 이 디렉터리에는 private key가 있으므로 일반 산출물로 공유하지 않는다.
이 fixture 파일은 프로젝트 `tmp/` 아래에 있고 Git에서 제외된다.

## 명시적 seed

start에는 seed가 포함되지 않는다. live contract test는 보통 자체 임시·고유
객체를 생성하므로 benchmark/demo seed 없이 실행할 수 있다.

```sh
# 기존 PostgreSQL baseline: 생성 customers100만 행
bash scripts/fixture-engines.sh seed pg baseline
# 같은 baseline을 별도 TLS PostgreSQL DB에 명시적으로 적재:
bash scripts/fixture-engines.sh seed pg-tls baseline
# MySQL의 작은 생성 baseline: customers1,000행
bash scripts/fixture-engines.sh seed mysql baseline
# Composite FK/generated/index metadata와 MySQL의 작은 demo 데이터:
bash scripts/fixture-engines.sh seed pg rich
bash scripts/fixture-engines.sh seed pg-tls rich
bash scripts/fixture-engines.sh seed mysql rich
# 선택적 PostgreSQL metadata fidelity: comments/collation/deferred/unvalidated
bash scripts/fixture-engines.sh seed pg fidelity
# 탭별 database 선택/세션 격리를 확인할 작은 두 번째 DB (데이터 적재 없음):
bash scripts/fixture-engines.sh seed pg consoles
bash scripts/fixture-engines.sh seed mysql consoles
```

seed는 target 하나와 `baseline|rich|fidelity|consoles`를 모두 지정해야 한다. `seed all`은 거부한다.
사용자 URL이나 DB명을 인자로 받지 않는다. 일반 seed는 고정 `dbide_fixture`에서
실행한다. `consoles`는 같은 owned container 안에 `dbide_console_b`가 없을 때만
생성한다. 기존 DB/테이블/row를 수정하거나 삭제하지 않는다.

| Seed | PostgreSQL namespace | MySQL names in `dbide_fixture` |
|---|---|---|
| baseline | `dbide_benchmark.customers` / 기존 versioned baseline.sql | `dbide_benchmark_customers` |
| rich | `dbide_workflow_features`: versioned catalog_features.sql | `dbide_demo_customers`, `dbide_demo_orders`, `dbide_demo_notes`, `dbide_demo_customer_totals` view |
| fidelity | `dbide_workflow_features`: versioned catalog_fidelity.sql | 지원하지 않음 |
| consoles | 두 번째 DB `dbide_console_b`, 데이터 적재 없음 | 두 번째 DB `dbide_console_b`, 데이터 적재 없음 |

PG rich는 composite PK/FK, deferred FK, identity/generated column, CHECK,
expression/partial/descending/INCLUDE index와 view를 추가한다. 별도 fidelity는
column comment, deterministic/custom/ICU nondeterministic collation,
deferrable PK/UNIQUE, unvalidated CHECK를 추가하며 ICU가 필요하다.
MySQL rich는 테이블별 3개 생성 row, FK, nullable/empty text, 한글·emoji·줄바꿈,
정확한 decimal, JSON, 집계 view를 포함한다. 기존 key는 PostgreSQL baseline의
ON CONFLICT DO NOTHING / MySQL INSERT IGNORE로 보존한다. DROP/TRUNCATE나
기존 row 수정은 하지 않는다. owned fixture view 정의는 재생성한다. MySQL DDL은
implicit commit이 있으므로 seed 전체를 원자적인 migration으로 취급하지 않는다.
두 engine baseline 규모가 다르므로 이 seed 결과로 성능 우위를 비교하지 않는다.

## 검증 범위

스크립트 추가 시 `bash -n`, shellcheck, help/잘못된 seed 인자 거부,
현재 owned container 3개의 status/health를 확인했다.
Mock Docker로 owner/port/image/TLS mount 불일치 12개 거부 경로도 확인했고,
어느 경우에도 mutation 명령을 호출하지 않았다. PostgreSQL18.6,
TLS PG의 ssl=t, MySQL8.4.11/TLS_AES_128_GCM_SHA256 확인.
실행 중인 테스트를 방해하지 않기 위해 restart/stop/renew와 baseline/rich seed는
실행하지 않았다. 별도 명시적 `seed pg fidelity`는 실행해 기존 객체 보존·constraint
guard·새 empty-edit fixture 생성을 확인했다.
이들의 실제 lifecycle 검증은 테스트 종료 후 명시적으로 수행할 작업이다.
