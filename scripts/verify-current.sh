#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Default checks never discover credentials or connect to network databases.
# --owned-live explicitly opts into this repository's disposable fixtures.
mode=${1:-offline}
case "$mode" in offline|--owned-live) ;; *) echo 'Usage: bash scripts/verify-current.sh [offline|--owned-live]' >&2; exit 2 ;; esac

cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
CARGO_TARGET_DIR="$PWD/target/tokio-postgres-vendor" cargo test \
  --manifest-path vendor/tokio-postgres/Cargo.toml --test transaction_status_wire
uv run --python '>=3.14' scripts/verify_project.py
git diff --check

if [[ "$mode" == --owned-live ]]; then
  # Refuse an unrelated container that happens to use the expected name.
  for pair in dbide-m0-pg:rust-database-ide-m0 dbide-tls-pg:rust-database-ide-tls dbide-mysql:rust-database-ide-mysql; do
    name=${pair%%:*}
    expected=${pair#*:}
    owner=$(docker inspect --format '{{index .Config.Labels "owner"}}' "$name")
    running=$(docker inspect --format '{{.State.Running}}' "$name")
    [[ "$owner" == "$expected" && "$running" == true ]] || { echo "Owned fixture unavailable: $name" >&2; exit 1; }
  done
  export DBIDE_TEST_PG_URL='postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture'
  export DBIDE_WORKFLOW_TEST_URL="$DBIDE_TEST_PG_URL"
  export DBIDE_TEST_TLS_CA="$PWD/tmp/tls-fixture/ca.pem"
  export DBIDE_MYSQL_FIXTURE_CA="$DBIDE_TEST_TLS_CA"
  [[ -f "$DBIDE_TEST_TLS_CA" ]] || { echo 'Owned TLS fixture CA missing' >&2; exit 1; }
  export DBIDE_MYSQL_FIXTURE=owned-loopback
  export DBIDE_BRIDGE_FIXTURES=owned-loopback
  export DBIDE_MCP_FIXTURE=owned-loopback
  export DBIDE_CLI_LIVE_FIXTURE=owned-loopback
  export DBIDE_DATA_FIXTURE=owned-loopback
  # Serialize live tests: several deliberately create and clean fixed fixture tables.
  for package in dbide-pg-spike dbide-bridge dbide-mysql dbide-workflows dbide-mcp dbide-cli dbide-data; do
    cargo test -p "$package" --tests -- --ignored --test-threads=1
  done
  # Public console/navigator/table-reader state machines; no native window or
  # user profile access. Navigator and table-reader probes are read-only.
  # Requires explicit `fixture-engines.sh seed pg consoles` setup beforehand.
  for test_target in console_sessions navigator table_reader; do
    cargo test -p dbide-ui-spike --test "$test_target" -- --ignored --test-threads=1
  done
fi
printf '%s\n' "Current checks passed ($mode). Native UI, Keychain opt-in, physical IME, and comparative performance remain separate gates."
