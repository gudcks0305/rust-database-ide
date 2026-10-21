//! Declarative, local-only extensions. SQL returned here is untrusted editor text.
//! This crate has no SQL executor, network client, script loader, or driver loader.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
pub const MAX_SNIPPETS: usize = 100;
pub const MAX_EXTENSIONS: usize = 128;
pub const MAX_SQL_BYTES: usize = 32 * 1024;
pub const CAPABILITIES: &[&str] = &["sql-snippets", "theme-palette"];

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Json(serde_json::Error),
    Invalid(String),
    AlreadyInstalled,
    NotInstalled,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Json(e) => e.fmt(f),
            Self::Invalid(e) => f.write_str(e),
            Self::AlreadyInstalled => {
                f.write_str("extension exists; explicit replacement required")
            }
            Self::NotInstalled => f.write_str("extension is not installed"),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    Postgres,
    Sqlite,
    Mysql,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snippet {
    pub id: String,
    pub title: String,
    pub dialect: Dialect,
    pub sql: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemePalette {
    pub background: String,
    pub foreground: String,
    pub accent: String,
    pub selection: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
    #[serde(default)]
    pub theme: Option<ThemePalette>,
}
fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
fn slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.as_bytes().last() != Some(&b'-')
        && !value.contains("--")
}
fn check_id(id: &str) -> Result<()> {
    if slug(id) {
        Ok(())
    } else {
        Err(invalid("id must be a lowercase ASCII slug (1..64 bytes)"))
    }
}
fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.contains('\0')
}
impl ExtensionManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(invalid("unsupported extension schema_version"));
        }
        check_id(&self.id)?;
        if !text(&self.name, 128) || !text(&self.description, 2048) {
            return Err(invalid("invalid name/description length or content"));
        }
        let parts: Vec<_> = self.version.split('.').collect();
        if parts.len() != 3
            || parts.iter().any(|p| {
                p.is_empty()
                    || p.len() > 9
                    || !p.bytes().all(|c| c.is_ascii_digit())
                    || (p.len() > 1 && p.starts_with('0'))
            })
        {
            return Err(invalid("version must be numeric major.minor.patch"));
        }
        if self.snippets.len() > MAX_SNIPPETS || (self.snippets.is_empty() && self.theme.is_none())
        {
            return Err(invalid(
                "extension needs snippets or a theme; snippet limit is 100",
            ));
        }
        let mut ids = HashSet::new();
        for snippet in &self.snippets {
            check_id(&snippet.id)?;
            if !ids.insert(&snippet.id)
                || !text(&snippet.title, 128)
                || !text(&snippet.sql, MAX_SQL_BYTES)
            {
                return Err(invalid("duplicate snippet id or invalid title/SQL"));
            }
        }
        if let Some(t) = &self.theme {
            for c in [&t.background, &t.foreground, &t.accent, &t.selection] {
                if c.len() != 7
                    || !c.starts_with('#')
                    || !c.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
                {
                    return Err(invalid("theme colors must be #RRGGBB"));
                }
            }
        }
        if serde_json::to_vec(self)?.len() > MAX_MANIFEST_BYTES {
            return Err(invalid("manifest exceeds byte limit"));
        }
        Ok(())
    }
    pub fn capabilities(&self) -> Vec<&'static str> {
        let mut result = Vec::new();
        if !self.snippets.is_empty() {
            result.push(CAPABILITIES[0]);
        }
        if self.theme.is_some() {
            result.push(CAPABILITIES[1]);
        }
        result
    }
}
/// A preview performs validation only; importing does not install or enable it.
#[derive(Clone, Debug)]
pub struct ImportPreview {
    manifest: ExtensionManifest,
}
impl ImportPreview {
    pub fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }
}
pub fn import_bytes(bytes: &[u8]) -> Result<ImportPreview> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("manifest exceeds byte limit"));
    }
    let manifest: ExtensionManifest = serde_json::from_slice(bytes)?;
    manifest.validate()?;
    Ok(ImportPreview { manifest })
}
pub fn import_file(path: impl AsRef<Path>) -> Result<ImportPreview> {
    let file = fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("import must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    import_bytes(&bytes)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledExtension {
    pub enabled: bool,
    pub manifest: ExtensionManifest,
}
#[derive(Debug)]
pub struct InstallOutcome {
    pub path: PathBuf,
    pub backup: Option<PathBuf>,
}
/// One explicit application-owned root. No global/user directory discovery.
/// Callers must serialize mutations to a root across processes.
pub struct Registry {
    root: PathBuf,
}
impl Registry {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: fs::canonicalize(root)?,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        check_id(id)?;
        Ok(self.root.join(format!("{id}.json")))
    }
    fn read(&self, id: &str) -> Result<InstalledExtension> {
        let path = self.path(id)?;
        let meta = fs::symlink_metadata(&path).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                Error::NotInstalled
            } else {
                e.into()
            }
        })?;
        if !meta.file_type().is_file() || meta.len() > (MAX_MANIFEST_BYTES + 1024) as u64 {
            return Err(invalid("invalid registry entry"));
        }
        let entry: InstalledExtension = serde_json::from_reader(fs::File::open(path)?)?;
        entry.manifest.validate()?;
        if entry.manifest.id != id {
            return Err(invalid("entry id differs from filename"));
        }
        Ok(entry)
    }
    pub fn list(&self) -> Result<Vec<InstalledExtension>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| invalid("invalid filename"))?;
                if entries.len() >= MAX_EXTENSIONS {
                    return Err(invalid("registry exceeds extension count limit"));
                }
                entries.push(self.read(id)?);
            }
        }
        entries.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        Ok(entries)
    }
    fn archive_directory(&self) -> Result<PathBuf> {
        let path = self.root.join("archive");
        match fs::create_dir(&path) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
        if !fs::symlink_metadata(&path)?.file_type().is_dir() {
            return Err(invalid("archive must be a real directory"));
        }
        Ok(path)
    }
    fn backup(&self, source: &Path) -> Result<PathBuf> {
        let directory = self.archive_directory()?;
        let mut file = tempfile::Builder::new()
            .prefix("backup-")
            .suffix(".json")
            .tempfile_in(directory)?;
        io::copy(&mut fs::File::open(source)?, &mut file)?;
        file.as_file().sync_all()?;
        let (_, path) = file.keep().map_err(|e| Error::Io(e.error))?;
        fs::File::open(path.parent().expect("archive file parent"))?.sync_all()?;
        Ok(path)
    }
    fn write(&self, entry: &InstalledExtension, replace: bool) -> Result<InstallOutcome> {
        entry.manifest.validate()?;
        let path = self.path(&entry.manifest.id)?;
        let exists = match fs::symlink_metadata(&path) {
            Ok(_) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        if exists && !replace {
            return Err(Error::AlreadyInstalled);
        }
        if !exists && self.list()?.len() >= MAX_EXTENSIONS {
            return Err(invalid("registry extension count limit reached"));
        }
        if exists {
            self.read(&entry.manifest.id)?;
        }
        // NamedTempFile creates mode 0600 on Unix. Prepare fully before touching old entry.
        let mut temp = tempfile::NamedTempFile::new_in(&self.root)?;
        temp.write_all(&serde_json::to_vec(entry)?)?;
        temp.as_file().sync_all()?;
        let backup = if exists {
            Some(self.backup(&path)?)
        } else {
            None
        };
        if exists {
            temp.persist(&path).map_err(|e| Error::Io(e.error))?;
        } else {
            temp.persist_noclobber(&path).map_err(|e| {
                if e.error.kind() == io::ErrorKind::AlreadyExists {
                    Error::AlreadyInstalled
                } else {
                    Error::Io(e.error)
                }
            })?;
        }
        fs::File::open(&self.root)?.sync_all()?;
        Ok(InstallOutcome { path, backup })
    }
    /// Explicit installation. New installs start disabled; replacement preserves state.
    pub fn install(&self, preview: &ImportPreview, replace: bool) -> Result<InstallOutcome> {
        let enabled = match self.read(&preview.manifest.id) {
            Ok(entry) => entry.enabled,
            Err(Error::NotInstalled) => false,
            Err(e) => return Err(e),
        };
        self.write(
            &InstalledExtension {
                enabled,
                manifest: preview.manifest.clone(),
            },
            replace,
        )
    }
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<InstallOutcome> {
        let mut entry = self.read(id)?;
        entry.enabled = enabled;
        self.write(&entry, true)
    }
    /// Moves the complete installed record to archive; use restore_archive to undo.
    pub fn uninstall(&self, id: &str) -> Result<PathBuf> {
        self.read(id)?;
        let source = self.path(id)?;
        let destination = self.backup(&source)?;
        // Rename over our private archive copy: the active path disappears atomically.
        fs::rename(&source, &destination)?;
        fs::File::open(&self.root)?.sync_all()?;
        Ok(destination)
    }
    /// Only an archive record under this registry can be restored. Starts disabled.
    pub fn restore_archive(&self, path: impl AsRef<Path>, replace: bool) -> Result<InstallOutcome> {
        let path = path.as_ref();
        let directory = self.archive_directory()?;
        if path.parent() != Some(directory.as_path())
            || !fs::symlink_metadata(path)?.file_type().is_file()
        {
            return Err(invalid(
                "restore requires a regular file in this registry archive",
            ));
        }
        let file = fs::File::open(path)?;
        if file.metadata()?.len() > (MAX_MANIFEST_BYTES + 1024) as u64 {
            return Err(invalid("archive exceeds byte limit"));
        }
        let mut entry: InstalledExtension = serde_json::from_reader(file)?;
        entry.enabled = false;
        self.write(&entry, replace)
    }
    pub fn enabled_snippets(&self, dialect: Dialect) -> Result<Vec<(String, Snippet)>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|e| e.enabled)
            .flat_map(|e| {
                let id = e.manifest.id;
                e.manifest
                    .snippets
                    .into_iter()
                    .filter(move |s| s.dialect == dialect)
                    .map(move |s| (id.clone(), s))
            })
            .collect())
    }
    /// Return text for an explicit editor insertion action; never execute it.
    pub fn snippet_sql(&self, extension: &str, snippet: &str, dialect: Dialect) -> Result<String> {
        let entry = self.read(extension)?;
        if !entry.enabled {
            return Err(invalid("extension is disabled"));
        }
        entry
            .manifest
            .snippets
            .into_iter()
            .find(|s| s.id == snippet && s.dialect == dialect)
            .map(|s| s.sql)
            .ok_or_else(|| invalid("snippet not found for selected dialect"))
    }
    pub fn enabled_themes(&self) -> Result<Vec<(String, ThemePalette)>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|e| e.enabled)
            .filter_map(|e| e.manifest.theme.map(|theme| (e.manifest.id, theme)))
            .collect())
    }
}

pub fn builtin_manifest() -> ExtensionManifest {
    let examples = [
        (
            "pg-select",
            "PostgreSQL select",
            Dialect::Postgres,
            "SELECT 1 AS example;",
        ),
        (
            "pg-catalog",
            "PostgreSQL tables",
            Dialect::Postgres,
            "SELECT table_schema, table_name FROM information_schema.tables ORDER BY 1, 2;",
        ),
        (
            "pg-transaction",
            "PostgreSQL read-only transaction",
            Dialect::Postgres,
            "BEGIN READ ONLY;\nSELECT 1;\nROLLBACK;",
        ),
        (
            "sqlite-select",
            "SQLite select",
            Dialect::Sqlite,
            "SELECT 1 AS example;",
        ),
        (
            "sqlite-catalog",
            "SQLite tables",
            Dialect::Sqlite,
            "SELECT name, type FROM sqlite_schema WHERE type IN ('table', 'view') ORDER BY name;",
        ),
        (
            "sqlite-transaction",
            "SQLite transaction example",
            Dialect::Sqlite,
            "BEGIN;\nSELECT 1;\nROLLBACK;",
        ),
        (
            "mysql-select",
            "MySQL select",
            Dialect::Mysql,
            "SELECT 1 AS example;",
        ),
        (
            "mysql-catalog",
            "MySQL tables",
            Dialect::Mysql,
            "SELECT table_schema, table_name FROM information_schema.tables ORDER BY 1, 2;",
        ),
        (
            "mysql-transaction",
            "MySQL read-only transaction",
            Dialect::Mysql,
            "START TRANSACTION READ ONLY;\nSELECT 1;\nROLLBACK;",
        ),
    ];
    ExtensionManifest { schema_version: 1, id: "builtin-sql".into(), name: "Built-in SQL examples".into(), version: "1.0.0".into(), description: "Explicit editor insertion examples; review before running. Transaction examples change session state.".into(), snippets: examples.into_iter().map(|(id,title,dialect,sql)| Snippet { id: id.into(), title: title.into(), dialect, sql: sql.into() }).collect(), theme: None }
}

#[cfg(test)]
mod tests;
