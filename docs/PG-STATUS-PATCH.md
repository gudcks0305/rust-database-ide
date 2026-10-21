# PostgreSQL ReadyForQuery status patch

Date: 2026-10-21. Scope: local `tokio-postgres` 0.7.18 patch for the PostgreSQL
session experiment. This is not an upstream release or a general driver rewrite.

## Provenance and license

`vendor/tokio-postgres/` is copied from the official crates.io 0.7.18 package
already extracted in the local Cargo registry. The registry cache is unchanged.

- Package: [`tokio-postgres` 0.7.18](https://crates.io/crates/tokio-postgres/0.7.18).
- Upstream repository: <https://github.com/rust-postgres/rust-postgres>.
- Upstream VCS commit from retained `.cargo_vcs_info.json`:
  `f1cb6ec0d5766b136cbd68f3010d64142a5daa66`, path `tokio-postgres`.
- Source `.crate` SHA-256:
  `a528f7d280f6d5b9cd149635c8705b0dd049754bc67d81d31fa25169a93809d3`.
  This matches the original root Cargo.lock registry checksum.
- License remains `MIT OR Apache-2.0`. Original `LICENSE-MIT`, `LICENSE-APACHE`,
  authors, repository metadata, `Cargo.toml.orig`, and notices are retained.
- No reference application's code or assets are included in this patch.

The upstream code changes are confined to `src/simple_query.rs` and one public
re-export in `src/lib.rs`. `Cargo.toml` adds only an explicit integration-test
target; its original normalized metadata and dependencies remain intact.

## Public API and contract

```rust
pub enum TransactionStatus {
    Idle,
    InTransaction,
    InError,
}

impl SimpleQueryStream {
    pub fn transaction_status(&self) -> Option<TransactionStatus>;
}
```

The stream starts at `None`. When it consumes the query's `ReadyForQuery`
message, it records `I` as `Idle`, `T` as `InTransaction`, or `E` as `InError`
before returning the existing end-of-stream `None`. These bytes come directly
from the [PostgreSQL protocol contract](https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-READYFORQUERY).
SQL text and command tags are not used to infer transaction state.

No stream item or `SimpleQueryMessage` variant is added. Existing
`Client::simple_query` collection keeps its existing message shape. Connection
request routing, shared `Client` state, typed query APIs, and `batch_execute`
are unchanged.

A pinned consumer reads the final observation with
`stream.as_ref().get_ref().transaction_status()` after draining the stream.
After an SQL error, the consumer must preserve the error and continue draining
to the final `ReadyForQuery`. Stopping at the error, a transport failure, or
dropping the stream before that message leaves the observation unknown.
Unknown must not be converted to idle. Any status byte outside `I`, `T`, and `E`
returns a parse error and leaves the observation unknown.

This is the server state at the end of this request. It is not a continuously
updated connection-level snapshot, and the stream must not be polled again
after it returns `None`.

## Verification

The dedicated `transaction_status_wire` target uses an in-memory Tokio duplex
stream as a synthetic PostgreSQL peer. It connects to no real database and
requires no credentials. Tests exercise the actual driver startup, frame
decoder, request routing, response stream, and public accessor:

- All three valid statuses remain unknown through `CommandComplete` and become
  available only when the stream ends, without an extra stream item.
- An actual protocol `ErrorResponse` remains an error; continued draining
  obtains `InError` from the later `ReadyForQuery`.
- Disconnecting before `ReadyForQuery` returns an error and leaves state unknown.
- Every other byte value (253 cases) returns an error without fabricated state.
- `Client::simple_query` still returns the original command message vector.

Run from the project root after excluding the vendored package from workspace
membership and selecting it through `[patch.crates-io]`:

```sh
CARGO_TARGET_DIR=target/tokio-postgres-vendor cargo test --manifest-path vendor/tokio-postgres/Cargo.toml --test transaction_status_wire
```

Verified on 2026-10-21 with the pinned Rust 1.99.1 toolchain: **5 passed,
0 failed, 0 ignored**, including all 253 invalid-byte cases. The initial offline
attempt needed uncached upstream dev dependencies; the explicit online run
downloaded them and passed. No database connection was made.

`cargo fmt --manifest-path vendor/tokio-postgres/Cargo.toml -- --check` passed.
Standalone vendor Clippy with `-D warnings` reports 16 existing
`clippy::double_must_use` diagnostics from the unchanged upstream
`src/generic_client.rs` async-trait expansion on Rust 1.99.1. That strict vendor
lint run is not claimed as passed; unrelated upstream code was not rewritten.

The vendored upstream `tests/test/main.rs` is retained. Its full upstream suite
has separate server/authentication requirements and is not claimed as run by
this patch's dedicated test command. Owned PostgreSQL fixture tests in the
application crates provide separate live transaction/rollback/cancellation
evidence; default ignored tests are not evidence of a live pass.
