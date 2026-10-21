//! Explicit SQL file opening for dialogs and external drops.
//!
//! This helper only reads paths supplied by the caller. It never runs SQL or
//! opens referenced files during workspace restoration. Call it off the UI
//! thread; it performs bounded, synchronous filesystem I/O.
use std::{
    collections::HashSet,
    fmt, fs,
    io::Read,
    path::{Path, PathBuf},
};

pub const MAX_SQL_FILES_PER_BATCH: usize = 16;
pub const MAX_SQL_FILE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenedSqlFile {
    /// Canonical identity for deduplication against already open documents.
    pub canonical_path: PathBuf,
    pub title: String,
    pub content: String,
}

/// Single-file interface used by the native dialog/drop handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedSqlFile {
    pub path: PathBuf,
    pub title: String,
    pub sql: String,
}

pub fn read_sql_file(path: &Path) -> Result<LoadedSqlFile, FileOpenError> {
    let file = load_sql_files(vec![path.to_path_buf()])
        .pop()
        .expect("one input path produces one outcome")?;
    Ok(LoadedSqlFile {
        path: file.canonical_path,
        title: file.title,
        sql: file.content,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOpenErrorKind {
    UnsupportedExtension,
    BatchLimit,
    Duplicate,
    Io,
    NotRegular,
    TooLarge,
    InvalidUtf8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileOpenError {
    /// The selected/dropped path, including for errors before canonicalization.
    pub path: PathBuf,
    pub kind: FileOpenErrorKind,
    pub message: String,
}
impl fmt::Display for FileOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path.display(), self.message)
    }
}
impl std::error::Error for FileOpenError {}

/// Returns one ordered outcome per input path, including duplicates and excess
/// paths. At most sixteen paths are opened; later entries are explicit errors.
/// Symlinks to regular files are allowed. The selected filename must have a
/// case-insensitive `.sql` extension; identity/title come from its canonical path.
/// Deduplication against existing tabs is the caller's responsibility.
pub fn load_sql_files(paths: Vec<PathBuf>) -> Vec<Result<OpenedSqlFile, FileOpenError>> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .enumerate()
        .map(|(index, path)| {
            let fail = |kind, message: String| FileOpenError {
                path: path.clone(),
                kind,
                message,
            };
            if index >= MAX_SQL_FILES_PER_BATCH {
                return Err(fail(
                    FileOpenErrorKind::BatchLimit,
                    format!("Open at most {MAX_SQL_FILES_PER_BATCH} SQL files at once; this path was not opened"),
                ));
            }
            if !path.extension().and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("sql"))
            {
                return Err(fail(
                    FileOpenErrorKind::UnsupportedExtension,
                    "Only .sql files can be opened (extension is case-insensitive)".into(),
                ));
            }
            let canonical_path = fs::canonicalize(&path)
                .map_err(|error| fail(FileOpenErrorKind::Io, format!("Cannot resolve SQL file: {error}")))?;
            if !seen.insert(canonical_path.clone()) {
                return Err(fail(
                    FileOpenErrorKind::Duplicate,
                    "This canonical file is already included in the current open batch".into(),
                ));
            }
            let file = open_nonblocking(&canonical_path)
                .map_err(|error| fail(FileOpenErrorKind::Io, format!("Cannot open SQL file: {error}")))?;
            // Inspect the opened descriptor, not only a path susceptible to a
            // regular-file -> FIFO/device swap between metadata and open.
            let metadata = file.metadata()
                .map_err(|error| fail(FileOpenErrorKind::Io, format!("Cannot inspect opened SQL file: {error}")))?;
            if !metadata.is_file() {
                return Err(fail(
                    FileOpenErrorKind::NotRegular,
                    "SQL input must be a regular file; directories, FIFOs and devices are not supported".into(),
                ));
            }
            if metadata.len() > MAX_SQL_FILE_BYTES as u64 {
                return Err(fail(FileOpenErrorKind::TooLarge, size_message()));
            }
            // Recheck the actual bytes too: the file can grow after metadata.
            let mut bytes = Vec::with_capacity((metadata.len() as usize).min(MAX_SQL_FILE_BYTES));
            file.take((MAX_SQL_FILE_BYTES + 1) as u64).read_to_end(&mut bytes)
                .map_err(|error| fail(FileOpenErrorKind::Io, format!("Cannot read SQL file: {error}")))?;
            if bytes.len() > MAX_SQL_FILE_BYTES {
                return Err(fail(FileOpenErrorKind::TooLarge, size_message()));
            }
            let content = String::from_utf8(bytes).map_err(|_| fail(
                FileOpenErrorKind::InvalidUtf8,
                "SQL file is not valid UTF-8; convert its encoding before opening".into(),
            ))?;
            // UTF-8 BOM is an encoding marker, not SQL text. Line endings and
            // all subsequent characters remain unchanged; saves use plain UTF-8.
            let content = content.strip_prefix('\u{feff}').unwrap_or(&content).to_owned();
            let title = canonical_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            Ok(OpenedSqlFile { canonical_path, title, content })
        })
        .collect()
}

fn size_message() -> String {
    format!(
        "SQL file exceeds {MAX_SQL_FILE_BYTES} UTF-8 bytes; the file was not truncated or opened"
    )
}

fn open_nonblocking(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK prevents a FIFO/device replacement from blocking open;
        // the descriptor's file type is checked before any bytes are read.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn error(path: PathBuf) -> FileOpenError {
        load_sql_files(vec![path]).pop().unwrap().unwrap_err()
    }

    #[test]
    fn opens_uppercase_extension_utf8_with_canonical_identity() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("한글.SQL");
        let sql = "SELECT '안녕하세요 👋';\n";
        fs::write(&path, sql).unwrap();
        let opened = load_sql_files(vec![path.clone()]).pop().unwrap().unwrap();
        assert_eq!(opened.canonical_path, path.canonicalize().unwrap());
        assert_eq!(opened.title, "한글.SQL");
        assert_eq!(opened.content, sql);
    }

    #[test]
    fn single_file_interface_decodes_bom_and_preserves_crlf() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("bom.sql");
        fs::write(&path, "\u{feff}SELECT '한글';\r\n").unwrap();
        let opened = read_sql_file(&path).unwrap();
        assert_eq!(opened.path, path.canonicalize().unwrap());
        assert_eq!(opened.title, "bom.sql");
        assert_eq!(opened.sql, "SELECT '한글';\r\n");
    }

    #[test]
    fn rejects_non_sql_directory_missing_and_invalid_utf8() {
        let directory = TempDir::new().unwrap();
        let text = directory.path().join("query.txt");
        fs::write(&text, "SELECT 1;").unwrap();
        assert_eq!(error(text).kind, FileOpenErrorKind::UnsupportedExtension);
        let folder = directory.path().join("folder.sql");
        fs::create_dir(&folder).unwrap();
        assert_eq!(error(folder).kind, FileOpenErrorKind::NotRegular);
        assert_eq!(
            error(directory.path().join("missing.sql")).kind,
            FileOpenErrorKind::Io
        );
        let invalid = directory.path().join("invalid.sql");
        fs::write(&invalid, [0xff, 0xfe]).unwrap();
        let invalid = error(invalid);
        assert_eq!(invalid.kind, FileOpenErrorKind::InvalidUtf8);
        assert!(invalid.message.contains("UTF-8"));
    }

    #[test]
    fn exact_byte_limit_is_allowed_but_larger_file_is_never_truncated() {
        let directory = TempDir::new().unwrap();
        let exact = directory.path().join("exact.sql");
        let content = format!("{}a", "가".repeat(MAX_SQL_FILE_BYTES / 3));
        assert_eq!(content.len(), MAX_SQL_FILE_BYTES);
        fs::write(&exact, &content).unwrap();
        let opened = load_sql_files(vec![exact]).pop().unwrap().unwrap();
        assert_eq!(opened.content, content);
        let large = directory.path().join("large.sql");
        fs::write(&large, vec![b'x'; MAX_SQL_FILE_BYTES + 1]).unwrap();
        assert_eq!(error(large).kind, FileOpenErrorKind::TooLarge);
    }

    #[test]
    fn repeated_canonical_paths_are_explicit_duplicate_outcomes() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("query.sql");
        fs::write(&path, "SELECT 1;").unwrap();
        let results = load_sql_files(vec![path.clone(), path]);
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert_eq!(
            results[1].as_ref().unwrap_err().kind,
            FileOpenErrorKind::Duplicate
        );
    }

    #[test]
    fn batch_limit_reports_every_excess_path_without_opening_it() {
        let directory = TempDir::new().unwrap();
        let mut paths = Vec::new();
        for index in 0..MAX_SQL_FILES_PER_BATCH {
            let path = directory.path().join(format!("query-{index}.sql"));
            fs::write(&path, "SELECT 1;").unwrap();
            paths.push(path);
        }
        // These need not exist: the batch cap must reject them before I/O.
        paths.push(directory.path().join("excess-a.sql"));
        paths.push(directory.path().join("excess-b.sql"));
        let results = load_sql_files(paths);
        assert_eq!(results.len(), MAX_SQL_FILES_PER_BATCH + 2);
        assert!(results[..MAX_SQL_FILES_PER_BATCH].iter().all(Result::is_ok));
        assert!(
            results[MAX_SQL_FILES_PER_BATCH..].iter().all(|result| {
                result.as_ref().unwrap_err().kind == FileOpenErrorKind::BatchLimit
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn follows_regular_file_symlinks_and_deduplicates_their_targets() {
        use std::os::unix::fs::symlink;
        let directory = TempDir::new().unwrap();
        let target = directory.path().join("target.sql");
        let alias = directory.path().join("alias.SQL");
        fs::write(&target, "SELECT 'target';").unwrap();
        symlink(&target, &alias).unwrap();
        let results = load_sql_files(vec![alias, target.clone()]);
        let first = results[0].as_ref().unwrap();
        assert_eq!(first.canonical_path, target.canonicalize().unwrap());
        assert_eq!(first.title, "target.sql");
        assert_eq!(first.content, "SELECT 'target';");
        assert_eq!(
            results[1].as_ref().unwrap_err().kind,
            FileOpenErrorKind::Duplicate
        );
    }

    #[cfg(unix)]
    #[test]
    fn fifo_without_writer_is_rejected_without_blocking() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt, sync::mpsc, time::Duration};
        let directory = TempDir::new().unwrap();
        let fifo = directory.path().join("waiting.sql");
        let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // The path is a fresh test-owned directory, mode contains no flags, and
        // CString guarantees a terminated native pathname with no interior NUL.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            sender.send(error(fifo)).unwrap();
        });
        let error = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("opening a FIFO must not block waiting for a writer");
        assert_eq!(error.kind, FileOpenErrorKind::NotRegular);
    }
}
