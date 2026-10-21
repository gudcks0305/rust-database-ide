# Local SQL workspace recovery

`dbide-workspace` stores one bounded, versioned local recovery snapshot of SQL
tabs. The caller explicitly chooses a path; the crate performs no global profile
or settings discovery. A UI can use `profiles.path().with_file_name("workspace.json")`
for a separate application-owned sibling file. Nothing is written until `save`.

## API

```rust,ignore
use dbide_workspace::{WorkspaceStore, WorkspaceSnapshot, DocumentSnapshot};
let store = WorkspaceStore::open(explicit_workspace_path)?;
let recovered: Option<WorkspaceSnapshot> = store.load()?; // None means absent
store.save(&snapshot)?;
```

`WorkspaceStore::new(path)` is pure construction; `::open(path)` additionally
validates any existing content. Both retain the explicit path, available through
`.path()`. `load()` validates and returns `Result<Option<WorkspaceSnapshot>,
WorkspaceError>`; `save(&snapshot)` returns `Result<(), WorkspaceError>`.

`WorkspaceSnapshot` contains `schema_version: u32`, ordered
`documents: Vec<DocumentSnapshot>`, and `active_document_id: Option<u64>`.
`Default` is version 1 with no documents and no active ID. Nonempty workspaces
require an active ID that exists in the documents. IDs must be unique/nonzero.

Each document contains:

- `id: u64`, `title: String`, `revision: u64`.
- `sql: String`: the current editor recovery buffer.
- `saved_sql: String`: the last known saved baseline; `.dirty()` compares the
  exact strings. Dirty status survives restart without reading a file.
- `path: Option<PathBuf>`: metadata only, not a file to automatically read/write.
- `profile_id: Option<String>`: connection context only, allowed to reference a
  profile that was later removed. The UI must resolve or mark it unavailable.

Revision and tab order survive round trip. Recovered SQL may be incomplete or
invalid SQL; syntax validity is deliberately not a restoration requirement.
`DocumentSnapshot` Debug redacts both SQL buffers. No result rows, execution
reports, connection passwords, or Keychain data belong to this schema. Unknown
JSON fields are rejected rather than silently preserved.

## Restoration and autosave boundary

Loading only reads the specified workspace JSON file. It never runs SQL,
configures/connects a database, opens the SQL file named in document metadata,
or overwrites a referenced SQL file. Serialized SQL is a recovery buffer, not
proof of the current contents of the source file. External file changes and
stale saved baselines must be resolved explicitly by the UI before saving to
that source file. Recovery must not label a dirty recovered buffer as saved.

The store does not schedule autosave or change current UI behavior. A UI that
enables local autosave should announce that SQL buffers are retained locally,
show restoration and save failures, and preserve all in-memory dirty tabs when
snapshot validation or persistence fails. Do not silently truncate, discard,
or replace dirty tabs to satisfy limits. SQL may contain passwords, personal
information, or production data: this is plaintext local recovery data protected
by file permissions. It is separate from optional query-history persistence.

## Limits and failure behavior

Version 1 allows at most 32 documents. Current and saved SQL buffers are each
limited to 64 KiB of UTF-8 bytes, aligned with the current query/history SQL
budget. Total serialized JSON is limited to 2 MiB. Limits apply to bytes, not
characters. Maximum title size is 256 bytes, path 4096, and profile ID 128.
Titles/paths reject empty/whitespace-only values and control characters; paths
must serialize as valid UTF-8. Profile IDs reject surrounding whitespace and
control characters. Relative document paths remain metadata and are not resolved.

Individual buffers can satisfy their limits while the whole snapshot exceeds
2 MiB, particularly with escaping or both current/saved versions retained. Save
then returns an error and leaves the previous file intact. Load reads at most
2 MiB plus one byte before rejecting oversized input.

Malformed/partial JSON, invalid schema data, duplicate IDs, newer/unsupported
versions, unknown fields, and invalid UTF-8 return errors. Save rechecks existing
content while holding the writer lock and refuses to overwrite an invalid or
unsupported existing file. There is no automatic reset, deletion, rename, or
backup substitution. Preserve the original for explicit user-directed recovery;
opening an empty new store must be a deliberate choice after an error.

## Atomic replacement and locking

New parent directories use `0700`, and snapshot/lock/temporary files use `0600`
on Unix. Save takes a persistent sibling lock computed with
`path.with_extension("json.lock")` using `std::fs::File::lock`. It validates the
existing file, creates a UUID-named temporary file exclusively in the same
directory, writes and syncs it, then renames it over the snapshot. On Unix it
also syncs the parent directory. Errors before rename preserve the old snapshot
and clean up the attempted temporary file.

`WorkspaceError::Durability` is distinct: rename succeeded, but the subsequent
Unix directory sync failed. The snapshot has already been replaced; callers
must report uncertain crash durability rather than claim that nothing saved.
Filesystem/platform guarantees still apply. Windows code is compiled only when
that target is checked; current validation was on macOS.

An interrupted pre-rename write can leave an abandoned temporary file, which
load ignores. It never guesses which partial temporary file should replace the
main snapshot. A corrupt main file is surfaced as an error. The lock file stays
in place; do not unlink it while processes might use it. Locks are cooperative,
blocking, and released by dropping the file. Arbitrary external editors may
ignore them. Concurrent instances serialize complete snapshot replacements;
the latest successful full snapshot wins. There is no automatic tab merge
between separate running instances; the UI should use one owning writer per
workspace/session path.

## Verification

```sh
cargo test -p dbide-workspace
cargo clippy -p dbide-workspace --all-targets -- -D warnings
```

Tests use only temporary directories and fixture SQL. They cover ordered dirty
snapshot/revision/context round trips, missing-file behavior, duplicate/active
IDs, UTF-8 byte limits, total size preservation, corrupt/partial/future/unknown
JSON preservation, abandoned partial temporary files, redacted SQL Debug,
permissions, and unchanged referenced SQL files. They do not read user SQL files
or profiles, run a database, or perform real power-loss fault injection.
