//! Explicit-path, GPUI-free application settings. No user directory discovery.
pub use dbide_i18n::Locale;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub locale: Locale,
    pub editor_font_size: u16,
    pub editor_tab_width: u8,
    pub table_page_size: u16,
    pub sidebar_width: u16,
    pub editor_height: u16,
}
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            locale: Locale::Ko,
            editor_font_size: 14,
            editor_tab_width: 4,
            table_page_size: 100,
            sidebar_width: 280,
            editor_height: 300,
        }
    }
}
impl AppSettings {
    pub fn is_valid(&self) -> bool {
        (10..=24).contains(&self.editor_font_size)
            && (2..=8).contains(&self.editor_tab_width)
            && matches!(self.table_page_size, 50 | 100 | 250 | 500)
            && (180..=480).contains(&self.sidebar_width)
            && (120..=720).contains(&self.editor_height)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsChange {
    Locale(Locale),
    EditorFontSize(u16),
    EditorTabWidth(u8),
    TablePageSize(u16),
    Layout {
        sidebar_width: u16,
        editor_height: u16,
    },
}
#[derive(Serialize, Deserialize)]
struct Document {
    schema_version: u32,
    locale: Locale,
    #[serde(default = "default_font_size")]
    editor_font_size: u16,
    #[serde(default = "default_tab_width")]
    editor_tab_width: u8,
    #[serde(default = "default_page_size")]
    table_page_size: u16,
    #[serde(default = "default_sidebar_width")]
    sidebar_width: u16,
    #[serde(default = "default_editor_height")]
    editor_height: u16,
    #[serde(flatten)]
    extra: Map<String, Value>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            locale: Locale::Ko,
            editor_font_size: default_font_size(),
            editor_tab_width: default_tab_width(),
            table_page_size: default_page_size(),
            sidebar_width: default_sidebar_width(),
            editor_height: default_editor_height(),
            extra: Map::new(),
        }
    }
}
fn default_font_size() -> u16 {
    AppSettings::default().editor_font_size
}
fn default_tab_width() -> u8 {
    AppSettings::default().editor_tab_width
}
fn default_page_size() -> u16 {
    AppSettings::default().table_page_size
}
fn default_sidebar_width() -> u16 {
    AppSettings::default().sidebar_width
}
fn default_editor_height() -> u16 {
    AppSettings::default().editor_height
}
impl Document {
    fn settings(&self) -> AppSettings {
        AppSettings {
            locale: self.locale,
            editor_font_size: self.editor_font_size,
            editor_tab_width: self.editor_tab_width,
            table_page_size: self.table_page_size,
            sidebar_width: self.sidebar_width,
            editor_height: self.editor_height,
        }
    }
    fn apply(&mut self, settings: AppSettings) {
        self.locale = settings.locale;
        self.editor_font_size = settings.editor_font_size;
        self.editor_tab_width = settings.editor_tab_width;
        self.table_page_size = settings.table_page_size;
        self.sidebar_width = settings.sidebar_width;
        self.editor_height = settings.editor_height;
    }
}
#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    InvalidData,
    UnsupportedVersion(u32),
    TooLarge,
    NotRegularFile,
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self { Self::Io(e)=>write!(f,"Application settings I/O failed: {e}"), Self::InvalidData=>f.write_str("Application settings are malformed; original preserved"), Self::UnsupportedVersion(version)=>write!(f,"Unsupported application settings schema version {version}; original preserved"), Self::TooLarge=>f.write_str("Application settings exceed the 64 KiB limit; original preserved"), Self::NotRegularFile=>f.write_str("Application settings and lock paths must be regular files, not symlinks or special files") }
    }
}
impl std::error::Error for StoreError {}
impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
#[derive(Clone, Debug)]
pub struct SettingsStore {
    path: PathBuf,
}
impl SettingsStore {
    /// Pure construction: does not create directories/files or read any settings.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    /// A shared UI/CLI convention, derived only from the explicitly chosen profile path.
    /// Does not load or validate the profile file.
    pub fn for_profile_path(profile_path: impl AsRef<Path>) -> Self {
        let parent = profile_path
            .as_ref()
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        Self::new(parent.join("app-settings.json"))
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn load(&self) -> Result<AppSettings, StoreError> {
        Ok(self.read_document()?.settings())
    }
    pub fn save(&self, settings: &AppSettings) -> Result<(), StoreError> {
        self.update(|current| *current = *settings).map(|_| ())
    }
    /// Patch under the writer lock; unrelated settings and unknown JSON survive.
    pub fn update(&self, patch: impl FnOnce(&mut AppSettings)) -> Result<AppSettings, StoreError> {
        let _lock = self.lock_writer()?;
        // Re-read under the lock: never overwrite a later corrupt/future file or drop unknowns.
        let mut document = self.read_document()?;
        let mut settings = document.settings();
        patch(&mut settings);
        if !settings.is_valid() {
            return Err(StoreError::InvalidData);
        }
        document.apply(settings);
        let bytes = serde_json::to_vec(&document).map_err(|_| StoreError::InvalidData)?;
        if bytes.len() > MAX_SETTINGS_BYTES {
            return Err(StoreError::TooLarge);
        }
        self.write_atomic(&bytes)?;
        Ok(settings)
    }
    pub fn set_locale(&self, locale: Locale) -> Result<AppSettings, StoreError> {
        self.update(|settings| settings.locale = locale)
    }
    pub fn apply(&self, change: SettingsChange) -> Result<AppSettings, StoreError> {
        self.update(|settings| match change {
            SettingsChange::Locale(value) => settings.locale = value,
            SettingsChange::EditorFontSize(value) => settings.editor_font_size = value,
            SettingsChange::EditorTabWidth(value) => settings.editor_tab_width = value,
            SettingsChange::TablePageSize(value) => settings.table_page_size = value,
            SettingsChange::Layout {
                sidebar_width,
                editor_height,
            } => {
                settings.sidebar_width = sidebar_width;
                settings.editor_height = editor_height;
            }
        })
    }
    pub fn set_layout(
        &self,
        sidebar_width: u16,
        editor_height: u16,
    ) -> Result<AppSettings, StoreError> {
        self.apply(SettingsChange::Layout {
            sidebar_width,
            editor_height,
        })
    }
    pub fn set_editor_font_size(&self, value: u16) -> Result<AppSettings, StoreError> {
        self.update(|settings| settings.editor_font_size = value)
    }
    pub fn set_editor_tab_width(&self, value: u8) -> Result<AppSettings, StoreError> {
        self.update(|settings| settings.editor_tab_width = value)
    }
    pub fn set_table_page_size(&self, value: u16) -> Result<AppSettings, StoreError> {
        self.update(|settings| settings.table_page_size = value)
    }
    fn parent(&self) -> &Path {
        self.path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
    }
    fn read_document(&self) -> Result<Document, StoreError> {
        reject_non_regular_path(&self.path)?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        safe_open_flags(&mut options);
        let file = match options.open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Document::default()),
            Err(e) => return Err(e.into()),
        };
        if !file.metadata()?.is_file() {
            return Err(StoreError::NotRegularFile);
        }
        let mut bytes = Vec::new();
        file.take((MAX_SETTINGS_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_SETTINGS_BYTES {
            return Err(StoreError::TooLarge);
        }
        let document: Document =
            serde_json::from_slice(&bytes).map_err(|_| StoreError::InvalidData)?;
        if document.schema_version != SCHEMA_VERSION {
            return Err(StoreError::UnsupportedVersion(document.schema_version));
        }
        if !document.settings().is_valid() {
            return Err(StoreError::InvalidData);
        }
        Ok(document)
    }
    fn lock_writer(&self) -> Result<fs::File, StoreError> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(self.parent())?;
        let mut filename = self
            .path
            .file_name()
            .ok_or(StoreError::NotRegularFile)?
            .to_os_string();
        filename.push(".lock");
        let lock_path = self.parent().join(filename);
        reject_non_regular_path(&lock_path)?;
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        safe_open_flags(&mut options);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(lock_path)?;
        if !file.metadata()?.is_file() {
            return Err(StoreError::NotRegularFile);
        }
        file.lock()?;
        Ok(file)
    }
    fn write_atomic(&self, bytes: &[u8]) -> Result<(), StoreError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = self.parent().join(format!(
            ".app-settings-{}-{timestamp}-{}.tmp",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        let result = (|| -> Result<(), StoreError> {
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            reject_non_regular_path(&self.path)?;
            fs::rename(&temporary, &self.path)?;
            #[cfg(unix)]
            fs::File::open(self.parent())?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}
fn safe_open_flags(options: &mut fs::OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    let _ = options;
}
fn reject_non_regular_path(path: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => Err(StoreError::NotRegularFile),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
#[cfg(test)]
mod tests;
