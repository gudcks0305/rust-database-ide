# Proposed continuous checks

`ci/rust-checks.yml` is a reviewable, inactive workflow draft. GitHub rejected
publishing it under `.github/workflows/ci.yml` because the current OAuth
credential lacks the `workflow` scope. No permissions were changed. The local
`work/ci-workflow-permission` branch preserves that unpublished attempt.

Once authorized workflow publication is available, the proposed job runs on
pushes to `main`, pull requests and explicit workflow dispatch. It uses the standard `macos-15` ARM64 runner, matching the
initial desktop target. Runner labels and public-repository availability were
checked against the [official runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
on 2026-10-22. Standard hosted runners are free for public repositories; this
workflow does not request a larger runner or deployment environment.

The job installs the Rust version/components from `rust-toolchain.toml`, checks
formatting, runs workspace tests, Clippy, the existing vendored protocol tests,
documentation/evidence metadata checks and whitespace checks. Dependencies in
the workspace use `--locked`. Action revisions are pinned to full commit SHAs;
checkout has read-only contents permissions and does not retain credentials.
Build jobs are limited to two, incremental compilation and development/test
debug info are disabled to reduce runner disk/memory pressure. A newer push to
the same ref cancels an obsolete run.

This is an offline test job. Ignored PostgreSQL/MySQL integration tests remain
ignored here, and CI must not be reported as proof that those tests ran. The
owned local fixture verification is separate:

```sh
bash scripts/verify-current.sh --owned-live
```

GUI interaction, Keychain opt-in, physical Korean IME, VoiceOver, cross-app file
drag, sampled app performance and comparative DataGrip measurements are also
separate acceptance gates. CI does not build a signed release or deploy an app.

Raw REA evidence intentionally stays outside Git. `verify_project.py` reports
these files as missing on a fresh checkout while still validating the versioned
metadata, links and feature IDs; it does not claim remote verification of the
missing raw evidence hashes.

No remote CI run has started. `actionlint` validates the draft locally, and a
fresh exported public checkout passes the documentation checker. Local tests
remain the current evidence; the draft is not a passing or enabled CI pipeline.
