#!/usr/bin/env bash
set -euo pipefail

# Disposable loopback-only fixture. Never reads a user's database configuration.
container=dbide-m0-pg
image='postgres:18-alpine@sha256:77f585114c32fbca283dc835b0596f4e52b51b4c6662d7810b2f4084f60a1873'
case "${1:-start}" in
  start)
    if docker container inspect "$container" >/dev/null 2>&1; then
      owner=$(docker inspect --format '{{index .Config.Labels "owner"}}' "$container")
      [[ "$owner" == rust-database-ide-m0 ]] || { echo 'Container name belongs to another owner' >&2; exit 1; }
      docker start "$container" >/dev/null
    else
      docker run -d --name "$container" --label owner=rust-database-ide-m0 \
        -e POSTGRES_PASSWORD=dbide_fixture_only -e POSTGRES_DB=dbide_fixture \
        -p 127.0.0.1:55439:5432 "$image" >/dev/null
    fi
    for _attempt in $(seq 1 30); do
      if docker exec "$container" pg_isready -U postgres -d dbide_fixture >/dev/null; then
        echo 'Fixture ready on 127.0.0.1:55439 (dbide_fixture)'
        exit 0
      fi
      sleep 1
    done
    echo 'Fixture did not become ready' >&2
    exit 1
    ;;
  stop)
    owner=$(docker inspect --format '{{index .Config.Labels "owner"}}' "$container")
    [[ "$owner" == rust-database-ide-m0 ]] || { echo 'Refusing to stop unrelated container' >&2; exit 1; }
    docker stop "$container" >/dev/null
    ;;
  *) echo 'Usage: bash scripts/fixture-pg.sh start|stop' >&2; exit 2 ;;
esac
