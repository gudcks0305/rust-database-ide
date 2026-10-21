#!/usr/bin/env bash
set -euo pipefail
umask 077

# Only project-owned loopback fixtures. No container/volume/database deletion.
fixture_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
fixture_tls="$fixture_root/tmp/tls-fixture"
pg_image='postgres:18-alpine@sha256:77f585114c32fbca283dc835b0596f4e52b51b4c6662d7810b2f4084f60a1873'
mysql_image='mysql:8.4@sha256:0f51e63e537a1f176423fa03ef559aabe6fa4372a53160a44bc7e75da7d15364'
fixture_lock=''
fail() { echo "Fixture error: $*" >&2; exit 1; }
usage() {
  echo 'Usage: fixture-engines.sh start|status|stop [all|pg|pg-tls|mysql]'
  echo '       fixture-engines.sh seed pg|pg-tls|mysql baseline|rich|fidelity|consoles'
  echo '       fixture-engines.sh renew-certs'
}
container_for() { case "$1" in pg) echo dbide-m0-pg;; pg-tls) echo dbide-tls-pg;; mysql) echo dbide-mysql;; *) fail 'Unknown fixture target';; esac; }
owner_for() { case "$1" in pg) echo rust-database-ide-m0;; pg-tls) echo rust-database-ide-tls;; mysql) echo rust-database-ide-mysql;; esac; }
port_for() { case "$1" in pg) echo 55439;; pg-tls) echo 55440;; mysql) echo 55441;; esac; }
image_for() { case "$1" in pg|pg-tls) echo "$pg_image";; mysql) echo "$mysql_image";; esac; }
exists() { docker container inspect "$(container_for "$1")" >/dev/null 2>&1; }
running() { [[ $(docker inspect --format '{{.State.Running}}' "$(container_for "$1")") == true ]]; }
assert_owned() {
  local target=$1 container owner bindings expected actual_image expected_image private_port
  container=$(container_for "$target")
  owner=$(docker inspect --format '{{index .Config.Labels "owner"}}' "$container")
  [[ "$owner" == "$(owner_for "$target")" ]] || fail "$container belongs to another owner; refusing action"
  private_port=5432; [[ "$target" != mysql ]] || private_port=3306
  bindings=$(docker inspect --format '{{range $port, $bindings := .HostConfig.PortBindings}}{{range $bindings}}{{printf "%s=%s:%s\n" $port .HostIp .HostPort}}{{end}}{{end}}' "$container")
  expected="$private_port/tcp=127.0.0.1:$(port_for "$target")"
  [[ "$bindings" == "$expected" ]] || fail "$container has unexpected published ports; refusing action"
  actual_image=$(docker inspect --format '{{.Image}}' "$container")
  expected_image=$(docker image inspect "$(image_for "$target")" --format '{{.Id}}') || fail 'Pinned image is not available locally for ownership verification'
  [[ "$actual_image" == "$expected_image" ]] || fail "$container has an unexpected image; refusing action"
  if [[ "$target" == pg-tls ]]; then
    local mount
    mount=$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/tls"}}{{printf "%s|%s|%t" .Type .Source .RW}}{{end}}{{end}}' "$container")
    [[ "$mount" == "bind|$fixture_tls|false" ]] || fail "$container has an unexpected TLS mount; refusing action"
  fi
}
local_docker() {
  command -v docker >/dev/null || fail 'Docker CLI is required'
  local endpoint
  endpoint=${DOCKER_HOST:-$(docker context inspect --format '{{.Endpoints.docker.Host}}')}
  [[ "$endpoint" == unix://* ]] || fail 'Use a local Unix-socket Docker context for these local fixtures'
  docker info --format '{{.OSType}}' >/dev/null || fail 'Docker daemon is unavailable'
}
validate_certs() {
  command -v openssl >/dev/null || fail 'OpenSSL is required for TLS fixtures'
  [[ -f "$fixture_tls/ca.pem" && -f "$fixture_tls/server.pem" && -f "$fixture_tls/server.key" ]] || fail 'Incomplete TLS files; repair or explicitly renew while TLS fixtures are stopped'
  openssl x509 -in "$fixture_tls/ca.pem" -noout -checkend 0 >/dev/null || fail 'Fixture CA expired; stop TLS fixtures and run renew-certs'
  openssl x509 -in "$fixture_tls/server.pem" -noout -checkend 0 >/dev/null || fail 'Fixture server certificate expired; stop TLS fixtures and run renew-certs'
  openssl verify -CAfile "$fixture_tls/ca.pem" -verify_hostname localhost "$fixture_tls/server.pem" >/dev/null || fail 'Fixture certificate chain/localhost identity is invalid'
  openssl verify -CAfile "$fixture_tls/ca.pem" -verify_ip 127.0.0.1 "$fixture_tls/server.pem" >/dev/null || fail 'Fixture certificate lacks the loopback IP identity'
  local certificate_key private_key
  certificate_key=$(openssl x509 -in "$fixture_tls/server.pem" -pubkey -noout | openssl dgst -sha256)
  private_key=$(openssl pkey -in "$fixture_tls/server.key" -pubout 2>/dev/null | openssl dgst -sha256)
  [[ "$certificate_key" == "$private_key" ]] || fail 'Fixture certificate/private key do not match'
}
release_lock() { if [[ -n "$fixture_lock" ]]; then rmdir "$fixture_lock"; fixture_lock=''; fi; }
generate_certs() {
  local renew=${1:-false} stage backup file target
  command -v openssl >/dev/null || fail 'OpenSSL is required for certificate generation'
  mkdir -p "$fixture_root/tmp"
  fixture_lock="$fixture_root/tmp/.fixture-certs.lock"
  mkdir "$fixture_lock" 2>/dev/null || { fixture_lock=''; fail 'Another certificate operation holds the fixture certificate lock'; }
  trap release_lock EXIT
  for target in pg-tls mysql; do
    if exists "$target"; then assert_owned "$target"; ! running "$target" || fail 'Stop pg-tls and mysql explicitly before generating/replacing their certificates'; fi
  done
  if [[ "$renew" != true ]]; then
    for file in ca.pem ca.key server.pem server.key; do [[ ! -e "$fixture_tls/$file" ]] || fail 'Refusing automatic replacement of an existing/partial TLS set'; done
  fi
  stage=$(mktemp -d "$fixture_root/tmp/tls-generation.XXXXXX")
  cat > "$stage/ca.cnf" <<'EOF'
[req]
prompt=no
distinguished_name=dn
x509_extensions=v3_ca
[dn]
CN=DBIDE Local Test CA
[v3_ca]
basicConstraints=critical,CA:TRUE
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
EOF
  cat > "$stage/extensions.cnf" <<'EOF'
basicConstraints=CA:FALSE
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:localhost,IP:127.0.0.1
EOF
  openssl req -new -x509 -newkey rsa:2048 -nodes -sha256 -days 3650 -config "$stage/ca.cnf" -keyout "$stage/ca.key" -out "$stage/ca.pem" >"$stage/generate.log" 2>&1
  openssl req -new -newkey rsa:2048 -nodes -subj '/CN=localhost' -keyout "$stage/server.key" -out "$stage/server.csr" >>"$stage/generate.log" 2>&1
  openssl x509 -req -in "$stage/server.csr" -CA "$stage/ca.pem" -CAkey "$stage/ca.key" -CAcreateserial -days 365 -sha256 -extfile "$stage/extensions.cnf" -out "$stage/server.pem" >>"$stage/generate.log" 2>&1
  chmod 600 "$stage/ca.key" "$stage/server.key"
  openssl verify -CAfile "$stage/ca.pem" -verify_hostname localhost "$stage/server.pem" >/dev/null
  mkdir -p "$fixture_tls"
  if [[ "$renew" == true ]]; then
    backup=$(mktemp -d "$fixture_root/tmp/tls-backup.XXXXXX")
    for file in ca.pem ca.key ca.srl server.pem server.key server.csr extensions.cnf generate.log; do
      if [[ -e "$fixture_tls/$file" ]]; then cp -p "$fixture_tls/$file" "$backup/$file"; fi
    done
    echo "Previous TLS material preserved in $backup"
  fi
  for file in ca.pem ca.key server.pem server.key; do cp "$stage/$file" "$fixture_tls/$file"; done
  chmod 600 "$fixture_tls/ca.key" "$fixture_tls/server.key"
  validate_certs
  release_lock
  echo 'Generated fixture-only CA and localhost/127.0.0.1 certificate; no system trust-store changes'
}
ensure_certs() {
  [[ ! -d "$fixture_root/tmp/.fixture-certs.lock" ]] || fail 'Certificate operation in progress; retry fixture start afterward'
  if [[ -f "$fixture_tls/ca.pem" && -f "$fixture_tls/server.pem" && -f "$fixture_tls/server.key" ]]; then validate_certs; else generate_certs false; fi
}
mysql_sql() {
  assert_owned mysql
  docker exec -i -e MYSQL_PWD=dbide_fixture_only dbide-mysql mysql -uroot --default-character-set=utf8mb4 --batch --skip-column-names --database=dbide_fixture "$@"
}
health() {
  local target=$1 container
  assert_owned "$target"; container=$(container_for "$target")
  running "$target" || return 1
  if [[ "$target" != pg ]]; then validate_certs; fi
  if [[ "$target" == mysql ]]; then
    local host_certificate loaded_certificate
    host_certificate=$(openssl x509 -in "$fixture_tls/server.pem" -noout -fingerprint -sha256)
    loaded_certificate=$(docker exec dbide-mysql openssl x509 -in /var/lib/mysql/server-cert.pem -noout -fingerprint -sha256)
    [[ "$host_certificate" == "$loaded_certificate" ]] || fail 'MySQL stored certificate differs from host fixture material; stop then start explicitly to install it'
  fi
  case "$target" in
    pg) docker exec "$container" pg_isready -U postgres -d dbide_fixture >/dev/null && docker exec "$container" psql -U postgres -d dbide_fixture -Atc 'SELECT version();';;
    pg-tls) docker exec -e PGPASSWORD=dbide_fixture_only "$container" psql 'host=localhost port=5432 dbname=dbide_fixture user=postgres sslmode=verify-full sslrootcert=/tls/ca.pem' -Atc "SELECT version(); SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid();";;
    mysql) mysql_sql --host=127.0.0.1 --ssl-mode=VERIFY_IDENTITY --ssl-ca=/var/lib/mysql/ca.pem --connect-timeout=2 -e "SELECT VERSION(); SHOW SESSION STATUS LIKE 'Ssl_cipher';";;
  esac
}
status_one() {
  local target=$1
  if ! exists "$target"; then echo "$target: absent"; return; fi
  assert_owned "$target"
  if ! running "$target"; then echo "$target: stopped (data retained)"; return; fi
  echo "$target: running on 127.0.0.1:$(port_for "$target")"
  health "$target"
}
start_one() {
  local target=$1 container attempt ready=false
  container=$(container_for "$target")
  if exists "$target"; then
    assert_owned "$target"
    if running "$target"; then echo "$target: already running; no restart or certificate reload"; health "$target"; return; fi
  fi
  if [[ "$target" == pg ]]; then bash "$fixture_root/scripts/fixture-pg.sh" start; assert_owned pg; return; fi
  ensure_certs
  if exists "$target"; then assert_owned "$target"; docker start "$container" >/dev/null
  elif [[ "$target" == pg-tls ]]; then
    docker run -d --name "$container" --label owner=rust-database-ide-tls \
      -e POSTGRES_PASSWORD=dbide_fixture_only -e POSTGRES_DB=dbide_fixture \
      -p 127.0.0.1:55440:5432 --mount "type=bind,source=$fixture_tls,target=/tls,readonly" \
      --entrypoint sh "$pg_image" -ec 'cp /tls/server.pem /tmp/server.pem
cp /tls/server.key /tmp/server.key
chown postgres:postgres /tmp/server.pem /tmp/server.key
chmod 600 /tmp/server.pem /tmp/server.key
exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/tmp/server.pem -c ssl_key_file=/tmp/server.key' >/dev/null
  else
    docker run -d --name "$container" --label owner=rust-database-ide-mysql \
      -e MYSQL_ROOT_PASSWORD=dbide_fixture_only -e MYSQL_DATABASE=dbide_fixture -e MYSQL_ROOT_HOST=% \
      -p 127.0.0.1:55441:3306 "$mysql_image" >/dev/null
  fi
  assert_owned "$target"
  for ((attempt=0; attempt<90; attempt++)); do
    if [[ "$target" == pg-tls ]]; then
      if docker exec "$container" pg_isready -U postgres -d dbide_fixture >/dev/null 2>&1; then ready=true; break; fi
    elif mysql_sql --connect-timeout=2 -e 'SELECT 1' >/dev/null 2>&1; then ready=true; break
    fi
    sleep 1
  done
  [[ "$ready" == true ]] || fail "$target did not become ready; container/data retained for diagnosis"
  if [[ "$target" == mysql ]]; then
    assert_owned mysql
    docker cp "$fixture_tls/ca.pem" dbide-mysql:/var/lib/mysql/ca.pem
    docker cp "$fixture_tls/server.pem" dbide-mysql:/var/lib/mysql/server-cert.pem
    docker cp "$fixture_tls/server.key" dbide-mysql:/var/lib/mysql/server-key.pem
    assert_owned mysql
    docker exec dbide-mysql sh -ec 'chown mysql:mysql /var/lib/mysql/ca.pem /var/lib/mysql/server-cert.pem /var/lib/mysql/server-key.pem; chmod 644 /var/lib/mysql/ca.pem /var/lib/mysql/server-cert.pem; chmod 600 /var/lib/mysql/server-key.pem'
    mysql_sql -e 'ALTER INSTANCE RELOAD TLS'
  fi
  health "$target"
}
stop_one() {
  local target=$1
  if ! exists "$target"; then echo "$target: absent"; return; fi
  assert_owned "$target"
  if ! running "$target"; then echo "$target: already stopped (data retained)"; return; fi
  if [[ "$target" == pg ]]; then bash "$fixture_root/scripts/fixture-pg.sh" stop; else docker stop "$(container_for "$target")" >/dev/null; fi
  echo "$target: stopped; container and volumes retained"
}
seed_mysql() {
  local kind=$1
  if [[ "$kind" == consoles ]]; then
    mysql_sql -e 'CREATE DATABASE IF NOT EXISTS dbide_console_b CHARACTER SET utf8mb4'
  elif [[ "$kind" == baseline ]]; then
    mysql_sql <<'SQL'
CREATE TABLE IF NOT EXISTS dbide_benchmark_customers(id bigint PRIMARY KEY,name varchar(128) NOT NULL,email varchar(256),status varchar(16) NOT NULL,balance decimal(18,4),display_name varchar(128),created_at datetime(6),active boolean,metadata json,optional_text text);
INSERT IGNORE INTO dbide_benchmark_customers
WITH RECURSIVE numbers(n) AS(SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<1000)
SELECT n,CONCAT('Customer ',n),CONCAT('customer-',n,'@example.invalid'),IF(MOD(n,3)=0,'inactive','active'),MOD(n,100000)/100,CONCAT('사용자 ',n),TIMESTAMPADD(SECOND,n,'2026-01-01 00:00:00'),MOD(n,2)=0,JSON_OBJECT('segment',MOD(n,7)),IF(MOD(n,5)=0,NULL,'') FROM numbers;
SQL
  else
    mysql_sql <<'SQL'
CREATE TABLE IF NOT EXISTS dbide_demo_customers(id integer PRIMARY KEY,name varchar(128) NOT NULL,email varchar(256) UNIQUE,balance decimal(18,4),metadata json);
CREATE TABLE IF NOT EXISTS dbide_demo_orders(id integer PRIMARY KEY,customer_id integer,total decimal(18,4),created_at datetime(6) NOT NULL,CONSTRAINT dbide_demo_orders_customer_fk FOREIGN KEY(customer_id) REFERENCES dbide_demo_customers(id));
CREATE TABLE IF NOT EXISTS dbide_demo_notes(id integer PRIMARY KEY,customer_id integer,body text,optional_text text,CONSTRAINT dbide_demo_notes_customer_fk FOREIGN KEY(customer_id) REFERENCES dbide_demo_customers(id));
START TRANSACTION;
INSERT IGNORE INTO dbide_demo_customers VALUES(1,'한국어 고객','customer1@example.invalid',123.4500,'{"tier":"gold"}'),(2,'NULL example',NULL,0.0000,'{}'),(3,'Emoji🙂','customer3@example.invalid',-12.3400,'{"active":false}');
INSERT IGNORE INTO dbide_demo_orders VALUES(1,1,20.0000,'2026-01-01 00:00:00'),(2,1,30.0000,'2026-01-02 00:00:00'),(3,3,0.0100,'2026-01-03 00:00:00');
INSERT IGNORE INTO dbide_demo_notes VALUES(1,1,CONCAT('한글·emoji🙂·comma, quote" and newline',CHAR(10),'second line'),NULL),(2,2,'',''),(3,3,'SQL-looking text: SELECT 1; --',NULL);
COMMIT;
CREATE OR REPLACE VIEW dbide_demo_customer_totals AS SELECT c.id,c.name,count(o.id) AS order_count,coalesce(sum(o.total),0) AS total FROM dbide_demo_customers c LEFT JOIN dbide_demo_orders o ON o.customer_id=c.id GROUP BY c.id,c.name;
SQL
  fi
}
seed_one() {
  local target=$1 kind=$2 source
  [[ "$kind" == baseline || "$kind" == rich || "$kind" == fidelity || "$kind" == consoles ]] || fail 'Seed kind must be baseline, rich, fidelity, or consoles'
  [[ "$target" != mysql || "$kind" != fidelity ]] || fail 'The fidelity fixture is PostgreSQL-only'
  assert_owned "$target"; running "$target" || fail 'Start the selected fixture before explicit seeding'
  if [[ "$target" == mysql ]]; then seed_mysql "$kind"
  elif [[ "$kind" == consoles ]]; then
    docker exec -i "$(container_for "$target")" psql -U postgres -d dbide_fixture -v ON_ERROR_STOP=1 <<'SQL'
SELECT 'CREATE DATABASE dbide_console_b'
WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = 'dbide_console_b')
\gexec
SQL
  else
    case "$kind" in
      baseline) source="$fixture_root/fixtures/postgres/baseline.sql";;
      rich) source="$fixture_root/crates/dbide-workflows/fixtures/catalog_features.sql";;
      fidelity) source="$fixture_root/crates/dbide-workflows/fixtures/catalog_fidelity.sql";;
    esac
    [[ -f "$source" ]] || fail 'Versioned PostgreSQL fixture source is missing'
    docker exec -i "$(container_for "$target")" psql -U postgres -d dbide_fixture -v ON_ERROR_STOP=1 < "$source"
  fi
  echo "$target: explicit $kind seed completed"
}

case "${1:-status}" in
  -h|--help|help) usage; exit 0;;
  start|status|stop)
    action=${1:-status}; target=${2:-all}
    case "$target" in all|pg|pg-tls|mysql) ;; *) usage >&2; exit 2;; esac
    [[ $# -le 2 ]] || { usage >&2; exit 2; }
    local_docker
    if [[ "$target" == all ]]; then for target in pg pg-tls mysql; do "${action}_one" "$target"; done
    else "${action}_one" "$target"
    fi
    ;;
  seed)
    [[ $# == 3 && "$2" != all ]] || { usage >&2; exit 2; }
    container_for "$2" >/dev/null; local_docker; seed_one "$2" "$3"
    ;;
  renew-certs)
    [[ $# == 1 ]] || { usage >&2; exit 2; }
    local_docker; generate_certs true
    ;;
  *) usage >&2; exit 2;;
esac
