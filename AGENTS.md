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
