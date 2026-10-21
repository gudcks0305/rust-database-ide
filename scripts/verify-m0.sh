#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p tmp evidence
bash scripts/fixture-pg.sh start
export DBIDE_TEST_PG_URL='postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture'
cargo fmt --all -- --check
cargo test --workspace
cargo test -p dbide-pg-spike --test postgres_contract -- --ignored
cargo test -p dbide-bridge --test fixture_contract -- --ignored
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release -p dbide-pg-spike > evidence/pg-spike.json
cargo run --release -p dbide-sqlite-spike > evidence/sqlite-spike.json
cargo run --release -p dbide-sql-spike -- --large > evidence/sql-spike.json
uv run --python '>=3.14' scripts/verify_project.py
git diff --check
printf '%s\n' 'M0 automated checks passed. Native UI, IME, accessibility, and comparative performance gates remain separate.'
