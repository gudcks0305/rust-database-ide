# Folder workspaces and agent workflow

Updated 2026-10-22. A workspace is an explicitly selected existing directory containing SQL files and a password-free default execution target. GUI and CLI share the `dbide-project` contract. Opening a workspace, listing files, reading context or generating a plan does not connect to a database.

## Files and persistence

Initializing a folder creates `.dbide/project.json` and, if absent, `queries/`. Existing manifests are never reinitialized. Manifest schema v1 contains `schema_version`, `name`, `revision` and optional `default_target`. The target holds profile ID, engine, explicit database and endpoint fingerprint; credentials and query results are excluded. Profile definitions remain in the separately selected profiles configuration. The fingerprint detects endpoint configuration changes; it is not authentication or proof of server identity. Database is stored separately from the endpoint fingerprint.

SQL stays in ordinary `.sql` files suitable for external editors and version control. Paths passed to workspace file commands are project-relative; parent traversal, hidden components, symlinks and special files are rejected. `.SQL` is accepted. SQL and manifest files are each limited to 64 KiB. SQL reads require UTF-8; empty drafts may be saved but cannot be planned for execution.

Each directory listing reads one level, scans at most 4,096 entries and returns at most 512 folders/SQL files. Hidden entries and other file types are omitted. `truncated` explicitly marks partial listings; this is not recursive indexing. Creation requires an existing parent and never silently replaces an entry. Cooperative project writers use a shared lock and complete same-directory atomic publication. SQL replacement compares the last loaded SHA-256; target changes compare the manifest revision. These checks do not sandbox malicious directory renames or writers that ignore the lock.

## GUI

Use New workspace to initialize an existing folder, or Open workspace for a previously initialized one. The Files sidebar offers directory navigation, Up, Refresh, new SQL files and new folders. At the root, Save As starts in `queries/`; in a browsed subdirectory it starts there. Recent locations, up to ten, live in the private configuration sidecar `project-locations.json`, outside the project. `DBIDE_WORKSPACE` can explicitly select the startup folder. Invalid or future location metadata is preserved with a diagnostic.

Opening, switching or closing the project browser preserves existing editor tabs. Opening an already open canonical file selects its tab without replacing its buffer. The project default target applies only to newly opened file tabs. Missing profiles or changed endpoints open an unbound tab with a warning; selecting the app's current connection is not a fallback. Use current console target saves the selected SQL console's target as the project default. File opening never starts a connection or runs SQL.

Saving a project file compares the on-disk content with the saved source before replacement. A conflict preserves the file and editor contents; reopen or Save As to resolve it. Each document retains its owning project root, so switching the browser does not move its save scope. Optional editor recovery schema v3 retains that root, saved source and target context; schemas v1/v2 migrate on load. Recovery does not read source files, overwrite them or connect to databases. Recovery buffers can contain SQL literals and remain separate from the shareable project manifest.

This version has one project browser. It does not provide isolated tab sets per project, multiple attached roots, per-file target overrides, Git integration or filesystem watching. Refresh updates listings; external edits do not silently replace an open buffer.

## CLI

Every `workspace` command requires explicit `--root`; there is no ancestor discovery. `--config` selects profiles separately and otherwise uses the normal platform default. `init`, `show`, `files`, `read`, `write` and `mkdir` do not need profiles. `bind`, bound-target `context` resolution and `plan` read the selected configuration without connecting or consulting credentials.

```sh
# Prepare these existing, isolated directories first. No user DB is needed.
mkdir -p ./tmp/workspace-demo ./tmp/workspace-config
dbide --config ./tmp/workspace-config/profiles.json workspace --root ./tmp/workspace-demo init --name demo
dbide --config ./tmp/workspace-config/profiles.json workspace --root ./tmp/workspace-demo show
dbide --config ./tmp/workspace-config/profiles.json workspace --root ./tmp/workspace-demo mkdir --path queries/reports
dbide --config ./tmp/workspace-config/profiles.json workspace --root ./tmp/workspace-demo write --file queries/reports/check.sql --sql 'SELECT 1;'
dbide --config ./tmp/workspace-config/profiles.json workspace --root ./tmp/workspace-demo files --directory queries/reports
dbide --config ./tmp/workspace-config/profiles.json workspace --root ./tmp/workspace-demo read --file queries/reports/check.sql
dbide --config ./tmp/workspace-config/profiles.json workspace --root ./tmp/workspace-demo context
dbide agent capabilities
```

`write` accepts exactly one of `--sql` and `--stdin`. Without `--expect-sha256` it is create-only. To replace a file, pass the exact `sha256` returned by the last `read`; a changed or missing source produces `conflict`. `bind --profile ID [--database NAME] --expect-revision N` saves a target only when revision `N` still matches. SQLite database must be `main`. Workspace targets and guarded queries require absolute SQLite file paths (`:memory:` is also accepted), so changing the process working directory cannot silently select another relative database file. Initialization starts at revision 0; read the current manifest instead of assuming a later revision.

Workspace and agent results are one JSON object with `schema_version:1` and `ok`. Runtime failures use `ok:false,error:{code,message}` and exit 2; clap argument errors remain stderr usage text with exit 2. Successful commands exit 0. Other existing CLI commands retain their own output and exit contracts documented in [CLI](CLI.md).

`agent capabilities` describes command effects, limits and exit codes without I/O. `context` returns manifest, target resolution status and at most 128 entries from the root listing. It excludes SQL content, history, database rows and passwords. Use explicit `read --file` when SQL content is wanted. An unresolved target is reported in context rather than silently rebound.

## Plan, review, execute

After creating a profile in the isolated configuration, bind its exact ID with the current manifest revision, then request a plan:

```sh
dbide --config ./tmp/workspace-config/profiles.json workspace --root ./tmp/workspace-demo bind --profile demo --expect-revision 0
dbide --config ./tmp/workspace-config/profiles.json workspace --root ./tmp/workspace-demo plan --file queries/reports/check.sql
```

`plan` validates the saved target and file, then returns `executed:false`, intended target metadata and an `argv` array for a separate `dbide query` invocation. It never authenticates, connects or submits SQL. Review the target and SQL before explicitly invoking that argv; pass the array directly to a process launcher instead of interpolating a shell command. Supply authentication separately through existing explicit CLI options when needed.

The generated argv includes absolute configuration/file paths, exact profile/database, `--expect-sql-sha256`, `--expect-target-fingerprint`, `--read-only` and `--format json`. Before authentication or dispatch, query refuses changed file bytes or a changed/missing target. SQL file input must be a regular UTF-8 file; Unix nonblocking open also refuses a planned file replaced by a FIFO without hanging. The explicit database remains pinned even if the profile's default database changes. Guards describe the planned bytes and endpoint; they do not validate SQL safety. `--read-only` is an adapter session setting, not the MCP SELECT allowlist, a SQL proof or a substitute for database permissions. SQL completion, cancellation and retained-result limits follow [CLI](CLI.md), including exit 5 for truncation and no automatic replay.

The existing [MCP server](MCP-SERVER.md) keeps its explicit single-profile, read-only scope. These workspace file-write commands do not add MCP file, shell or write tools. Verification observations belong in [current status](CURRENT-STATUS.md) and dated evidence; this document describes contracts rather than declaring native acceptance or performance results.
