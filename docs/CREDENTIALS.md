# Credential storage boundary

`dbide-credentials` provides opt-in, exact-profile credential operations. The
native implementation uses macOS Security Framework generic-password APIs via
`security-framework 3.7.0` / `security-framework-sys 2.17.0`. Its service is fixed:

```text
com.local.rust-database-ide.credentials
```

Each account attribute is the exact connection profile ID. Load, save, and
remove always supply both service and account; there is no enumeration, wildcard
lookup, other-service access, subprocess, or plaintext file fallback. Profile
renaming retains the same item because identity is the stable ID. The service
name scopes application queries; it is not an authorization boundary against
other local applications. Normal macOS Keychain access controls still apply.
The operating system may request Keychain access authorization.

## API and integration

```rust,ignore
use dbide_credentials::{CredentialStore, MacKeychainStore, Secret};

let store = MacKeychainStore; // construction performs no Keychain I/O
let password = Secret::new("an-explicitly-entered-password");
// Only after the user explicitly enables "remember password":
store.save(profile_id, &password)?;
let remembered: Option<Secret> = store.load(profile_id)?;
store.remove(profile_id)?; // missing item is already successfully removed
```

`CredentialStore` has `load(&self, id: &str) -> Result<Option<Secret>, CredentialError>`,
`save(&self, id: &str, secret: &Secret) -> Result<(), CredentialError>`, and
`remove(&self, id: &str) -> Result<(), CredentialError>`.
`load` returns `None` only for the native item-not-found status. Access denial,
cancellation, or locked/unavailable Keychain errors remain errors. Save creates
or updates that exact service/account item. Remove is idempotent.

`Secret::new(impl Into<Vec<u8>>)` moves credential bytes into `Zeroizing<Vec<u8>>`.
`expose_secret()` explicitly borrows bytes; `expose_str()` validates UTF-8 without
a lossy copy. Binary bytes and empty passwords are supported. Secret has no
`Clone`, `Serialize`, or `Display`; `Debug` is always `Secret([REDACTED])`.
`zeroize` (resolved 1.9.1) wipes its owned bytes on drop. This does not wipe
separate UI strings, driver copies, Keychain framework allocations, swap, or
historical allocations. Callers should minimize independent password copies.

`CredentialError` exposes only a safe category or operation plus numeric OSStatus,
never native error descriptions, profile IDs, passwords, or credential lengths.
Profile IDs must be 1–128 UTF-8 bytes with no surrounding whitespace or control
characters. Credential size is limited to 64 KiB. Validation happens before
native access. An oversized save leaves the previous item unchanged.

On other operating systems, the same `MacKeychainStore` methods explicitly return
`UnsupportedPlatform` after validating input; credentials are never redirected
to files or automatically cached elsewhere. `InMemoryCredentialStore` is an
explicit session/test implementation, with zeroizing values and redacted Debug.
Its load returns one deliberate owned copy; overwrite/removal drop and wipe the
stored value. It is not an automatic error recovery fallback.

This crate adds no UI behavior. Until explicit remember-password controls are
integrated, the existing connection form retains its session-memory behavior.
Profile JSON still has no password fields. A future UI should surface load/save/
remove errors and must not claim a credential was remembered after a failure.
Do not silently save merely because a connection succeeds. Disabling remember
or deleting a profile should explicitly remove its scoped item, with failures
reported rather than hidden.

## Verification and native fixture

Default tests use only `InMemoryCredentialStore`: ID byte limits, missing items,
exact-ID isolation, overwrite, idempotent removal, binary/empty values, safe
Debug/errors, and oversized-save preservation. Constructing the native store
has no side effects. Native code is compiled by ordinary macOS crate tests and
clippy, but default tests do not call native credential functions.

```sh
cargo test -p dbide-credentials
cargo clippy -p dbide-credentials --all-targets -- -D warnings
```

One explicitly ignored native integration test exists. It refuses to run without
`DBIDE_TEST_KEYCHAIN=1`, creates a unique `dbide-keychain-test-<UUID>` account under
this app's fixed service, writes only fake fixture passwords, checks overwrite,
and deletes exactly that generated account. A drop guard also attempts cleanup
on failure. Abrupt process termination can prevent cleanup; the test never scans
for or deletes other entries. Running it can trigger macOS access prompts and
must be an explicit test choice:

```sh
DBIDE_TEST_KEYCHAIN=1 cargo test -p dbide-credentials -- --ignored --exact tests::native_keychain_fixture_round_trip
```

The implementation worker did not run this native test or access the user's
Keychain. A successful default test run means native compilation plus mocked
behavior, not a verified native Keychain round trip.

## Primary API references

Verified 2026-10-21 against published 3.7.0 documentation and the resolved crate
source. The documented query constructor identifies items by service/account;
set creates or updates, and delete reports item-not-found for missing items:

- [PasswordOptions::new_generic_password](https://docs.rs/security-framework/3.7.0/security_framework/passwords/struct.PasswordOptions.html#method.new_generic_password)
- [generic_password](https://docs.rs/security-framework/3.7.0/security_framework/passwords/fn.generic_password.html)
- [set_generic_password](https://docs.rs/security-framework/3.7.0/security_framework/passwords/fn.set_generic_password.html)
- [delete_generic_password](https://docs.rs/security-framework/3.7.0/security_framework/passwords/fn.delete_generic_password.html)
- [zeroize::Zeroizing](https://docs.rs/zeroize/1.9.1/zeroize/struct.Zeroizing.html)
