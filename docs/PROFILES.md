# Connection profiles and local persistence

`dbide-profiles` stores general PostgreSQL, SQLite, and MySQL connection configuration independently
of the network worker. Creating a profile generates a stable random UUID and
starts with engine `PostgreSql`, `localhost:5432`, database/user `postgres`, a 10-second connection
timeout, and `VerifyFull`. The explicit `ConnectionProfile::fixture()` is the
project-owned `127.0.0.1:55439/dbide_fixture` test configuration with TLS disabled.
Neither constructor supplies or saves a password.

## API

- `ConnectionProfile::new()`, `::default()`, `::fixture()`, `.validate()`.
- `ConnectionRequest { profile, password }` is session-only, cloneable, has a
  redacted `Debug`, and deliberately has no serialization implementation.
- `ProfileStore::open(path)` validates an existing file without creating anything;
  `::open_default()` resolves the platform-specific application-owned path.
- `.load() -> Result<WorkspaceState, StoreError>` returns empty state when missing.
  `.save(&state)` validates and atomically replaces the file under the writer lock.
- `.update(action) -> Result<WorkspaceState, StoreError>` locks, loads the latest
  persisted state, applies the closure, validates and saves, then returns the fresh
  workspace. Use it for profile upsert/remove/selection, history, and preferences.
- `WorkspaceState::upsert(profile)`, `.remove(id)`, `.select(Option<&str>)`, and
  `.record_history(entry)` manage selection and bounded history. Updating an
  existing ID retains its identity; deleting it clears selection and its history.

Default files are platform specific:

- macOS: `~/Library/Application Support/RustDatabaseIDE/profiles.json`.
- Windows: `%APPDATA%/RustDatabaseIDE/profiles.json`; missing `APPDATA` is an error.
- Linux/other Unix: `$XDG_CONFIG_HOME/RustDatabaseIDE/profiles.json` when set to
  an absolute nonempty path, otherwise `~/.config/RustDatabaseIDE/profiles.json`.

Path resolution is separate from I/O and is tested with injected paths, without
changing environment variables. No other app's settings are read. New directories
use mode `0700`, files use `0600` on Unix.
Saving uses an exclusive same-directory temporary file, writes and syncs it,
then renames it over the target. Errors before rename preserve the old file and
remove the temporary file. A persistent sibling `profiles.json.lock` (computed
with `path.with_extension("json.lock")`) coordinates writers using the standard
library's exclusive `File::lock`; new lock files use mode `0600` on Unix.
Both `save` and `update` use the same lock. Dropping the open file releases it,
including when a callback errors or unwinds. Never unlink a live lock file: doing
so could let another writer lock a different inode. Lock acquisition is blocking;
callbacks must stay bounded and must not perform external I/O or recursively call
`save`/`update`. Readers use the atomically replaced file without taking the lock.
This is cooperative locking for application writers; arbitrary programs that
ignore the lock are not prevented from editing the JSON. Lock availability and
semantics on network filesystems depend on the filesystem/OS.

A full `.save(&old_snapshot)` remains an explicit full replacement for backward
compatibility and can overwrite newer fields. Normal GUI/CLI edits must use
`.update(...)` with the smallest intended mutation rather than copying their
stale complete workspace into the closure. The callback always starts from the
latest disk state, preserving independent profile/history/settings edits. Failed
callbacks, validation, or writes preserve the prior JSON. There is no automatic
field conflict resolution when two writers intentionally change the same field;
the later locked mutation wins. There is also no directory-fsync
crash-durability guarantee.

## Credentials, TLS, and history

Profiles never read or write passwords, and persisted profile structs have no
password field. Session-only credentials remain the default. Explicit remember,
load, and forget actions use the separate [credential store](CREDENTIALS.md);
they do not add secrets to profile JSON.
`deny_unknown_fields` rejects password-bearing or unknown JSON fields rather
than silently carrying them into a saved file. This is protection against
accidental persistence, not memory zeroization. Users should not put secrets in
profile names, database names, hostnames, or SQL; these are ordinary text fields.

`ConnectionProfile.engine` is `dbide_driver_api::DatabaseEngine` (also re-exported
by `dbide_profiles`). The variants are `PostgreSql`, `Sqlite`, and `MySql`.
Existing version-1 files lacking the new fields deserialize as PostgreSQL,
`sqlite_path: None`, `sqlite_create: false`, and `read_only: false`. A SQLite
path alone never switches the engine. Adapter selection must use `engine`
explicitly; validation is not proof that an adapter is available.

SQLite requires `sqlite_path: Some(nonempty_path)` of at most 4096 UTF-8 bytes
without control characters. Validation ignores host, port, database, username,
and TLS/CA fields for SQLite; common profile identity/name and timeout limits
still apply. Network engines retain their existing network-field validation.
`sqlite_create` is explicit permission to create an absent database; its default
is false. `read_only` requests a read-only session and defaults to false. These
are adapter-enforced connection policies; profile validation never opens or
creates a database file. Paths containing spaces are preserved exactly.

`Disable` explicitly requests plaintext transport for a network connection. Both `Require` and
`VerifyFull` require encryption and server certificate/hostname validation in
the PostgreSQL worker; they never fall back to plaintext or skip trust checks.
`root_cert_path: Option<String>` may reference an explicitly selected custom CA
file. It is a non-secret path stored with the profile, not imported into the OS
trust store. Certificate reading and trust verification belong to the backend.
`Require` currently has the same verification policy as `VerifyFull`; the name
must not imply acceptance of untrusted certificates.

`persist_history` defaults to `false`. Session history remains available in
memory, but saving excludes all SQL/history unless the user explicitly enables
history persistence in the UI. Disabling it and saving removes previously saved
history. Because `update` loads the disk state, its returned workspace contains
no earlier session-only history when persistence was disabled. UI callers should
retain their session history separately and apply the fresh profile/settings
state, rather than accidentally resetting session history on every edit.
`HistoryEntry` contains profile ID, SQL, execution Unix milliseconds,
outcome text, and optional row count. SQL can contain secrets and production
data: enabled history is plaintext local data protected only by file permissions.

## Validation and recovery

Schema version is `1`. Unsupported versions, malformed JSON, duplicate profile
IDs, invalid selections, unknown fields, oversized input, and invalid connection
fields return errors and preserve the original file; there is no automatic reset.
The UI must surface errors and let the user repair or explicitly choose another
file. Save also rechecks the current file so a corrupt file created after open
is not silently overwritten.

Limits: 100 profiles; 100 most recent history entries; 64 KiB of UTF-8 SQL per
entry; 1 MiB total JSON on disk/read. A history state can fit individual limits
but exceed the total file limit; saving then fails and preserves the previous
file. Profile IDs/names are bounded to 128 bytes, host to 253, database/user to
256, custom CA path to 4096. Required identity and network fields reject empty values, surrounding whitespace,
and control characters. Network hosts reject whitespace/URL credentials and
network port must be nonzero. SQLite paths preserve surrounding spaces when the
path is not entirely whitespace. Connect timeout is 1–300 seconds. IPv4/IPv6/hostnames are accepted;
DNS resolution is performed by the backend, not profile validation.

Tests use temporary paths only: `cargo test -p dbide-profiles`.

## Concurrent writer verification

`cargo test -p dbide-profiles` includes four concurrent child processes, each
performing eight profile/history mutations against one temporary store. The
regression checks that all 32 additions, the selected profile, and preferences
survive. Additional tests cover callback/validation rollback, lock release after
failure, and a stale GUI snapshot followed by an independent CLI-style mutation.
Tests never load real application settings or modify the process environment.
The subprocess fixture receives temporary paths only through its own environment.

The lock API is supported by the pinned Rust 1.99.1 toolchain; see
[std::fs::File::lock](https://doc.rust-lang.org/std/fs/struct.File.html#method.lock).
