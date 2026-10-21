# Declarative extensions (F42)

`dbide-extensions` supports local version-1 JSON snippets and theme palettes.
This is a bounded application API, not a universal database-driver plugin system.
No JavaScript, shell, native library, network request, installation hook or SQL
executor is provided. Imported SQL remains untrusted text, even for an enabled
extension. The caller must insert a selected snippet into the editor only after
an explicit user action; running that SQL is a separate existing execution flow.

## Manifest and compatibility

```json
{
  "schema_version": 1,
  "id": "team-snippets",
  "name": "Team SQL examples",
  "version": "1.0.0",
  "description": "Review before executing",
  "snippets": [
    { "id": "inspect-tables", "title": "SQLite tables", "dialect": "sqlite",
      "sql": "SELECT name FROM sqlite_schema WHERE type = 'table';" }
  ],
  "theme": {
    "background": "#101820", "foreground": "#F0F0F0",
    "accent": "#70B0F0", "selection": "#304860"
  }
}
```

`schema_version` must equal 1. The extension's numeric `major.minor.patch` version
is display/update metadata, not a claim of driver or application binary
compatibility. There is no executable ABI. Supported capabilities are only
`sql-snippets` and `theme-palette`; `manifest.capabilities()` reports those actually
present. Unknown JSON fields at every object level and unknown dialects are
rejected. Snippets and theme may be omitted individually, but not both.

| Field/resource | Bound |
| --- | --- |
| Input manifest file/bytes; serialized manifest | 256 KiB |
| Installed/archive record | Manifest bound + 1 KiB |
| Active registry entries | 128 |
| Extension/snippet id | 1–64 ASCII bytes, starts with a–z; lowercase a–z/0–9/hyphen; no trailing or doubled hyphen |
| Name, snippet title | Nonblank, at most 128 UTF-8 bytes |
| Description | Nonblank, at most 2,048 UTF-8 bytes |
| Snippets per manifest | 100; unique snippet ids |
| SQL per snippet | Nonblank, at most 32 KiB UTF-8 bytes |
| Version | Three numeric components, 1–9 digits each; no leading zeros except `0` |
| Palette | Exactly background/foreground/accent/selection, each `#RRGGBB` |

Text fields reject NUL. Theme validation is syntax validation, not a contrast or
accessibility guarantee. Supported dialects are exactly `postgres`, `sqlite`,
`mysql`; no cross-dialect fallback occurs. Built-in `builtin_manifest()` includes
three examples for each dialect (select, catalog, transaction). It is not
installed automatically. Transaction examples alter session state if a user
chooses to execute them; they finish with ROLLBACK and are only example text.

## Explicit lifecycle

```rust,no_run
use dbide_extensions::{Registry, Dialect, import_file};
# fn main() -> dbide_extensions::Result<()> {
let registry = Registry::open("/explicit/application-owned/extensions")?;
let preview = import_file("/explicit/chosen-extension.json")?;
// Present preview.manifest() and its capabilities before caller's install action.
let outcome = registry.install(&preview, false)?; // starts disabled
registry.set_enabled(&preview.manifest().id, true)?;
let candidates = registry.enabled_snippets(Dialect::Sqlite)?;
// On explicit selection, insert returned untrusted text into the SQL editor.
let sql_text = registry.snippet_sql("team-snippets", "inspect-tables", Dialect::Sqlite)?;
let archive = registry.uninstall("team-snippets")?;
registry.restore_archive(archive, false)?; // restored disabled
# let _ = (outcome, candidates, sql_text);
# Ok(()) }
```

`import_bytes` and `import_file` validate and return `ImportPreview` without
installation, enabling, SQL execution or any implicit search of settings/user
folders. `Registry::open` creates/canonicalizes only the explicitly supplied
root. Enabled state is stored per registry, so callers select an application
registry rather than modifying global settings. UI consumers can list enabled
snippets by dialect and enabled theme palettes.

Duplicate install fails unless `replace=true` is explicitly passed. Replacement
preserves enabled state. Every replacement or enable/disable change creates a
recoverable complete prior record under `root/archive/` before atomically
replacing the active record. Writes prepare and sync a private temporary file
on the same filesystem before committing. Unix installed and backup file modes
are 0600. A failed preparation preserves the old active bytes. Install without
replacement uses an atomic no-clobber operation.

Uninstall moves the active record to a uniquely named archive record rather than
deleting its contents. `restore_archive` accepts only a regular record within
this registry's archive, validates it again, starts it disabled, and preserves
the archive copy; restoring over an installed extension needs `replace=true`
and itself creates a backup. Archive entries are not listed as active. Archive
retention is deliberately unbounded: no automatic pruning silently destroys
recovery data. Callers may present explicit retention/cleanup controls later.

The root must be application-owned; serialize mutations to the same root across
processes. This API is not a defense against another process concurrently
rewriting that directory. Active symlink entries and symlink archive directories
are rejected. Stable validated ids are the only source of active filenames, so
absolute paths and traversal ids cannot write outside the selected root.

## Verification and integration boundary

`cargo test -p dbide-extensions` covers strict schema/version/size/count/content,
traversal and absolute ids, explicit import/install/enable, dialect matching,
failed replacement preserving active bytes, recoverable updates/uninstall,
palette validation, and Unix private permissions/symlink entry rejection.
`cargo clippy -p dbide-extensions --all-targets -- -D warnings` checks the crate.
No live DB, user profile, arbitrary plugin runtime or network is used.

The crate exposes the lifecycle for a future host UI; this document does not
claim that a native extension browser, third-party marketplace, universal driver
loader or GUI theme selector has been implemented.
