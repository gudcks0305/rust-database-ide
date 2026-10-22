use dbide_profiles::{ConnectionProfile, StoredExecutionTarget};
use dbide_project::{
    EntryKind, MAX_LIST_ENTRIES, MAX_MANIFEST_BYTES, MAX_SCANNED_ENTRIES, MAX_SQL_BYTES, Project,
    ProjectError, digest_sql,
};
use std::{fs, path::Path};
use tempfile::TempDir;

fn fixture() -> (TempDir, Project) {
    let root = tempfile::tempdir().unwrap();
    let project = Project::init(root.path(), "Fixture project").unwrap();
    (root, project)
}

fn assert_code<T: std::fmt::Debug>(result: Result<T, ProjectError>, code: &str) {
    assert_eq!(result.unwrap_err().code(), code);
}

#[test]
fn init_is_explicit_preserves_files_and_refuses_reinitialization() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("queries")).unwrap();
    fs::write(root.path().join("queries/existing.sql"), "SELECT 'kept'").unwrap();
    let project = Project::init(root.path(), "한글 프로젝트").unwrap();
    assert!(project.root().is_absolute());
    assert_eq!(project.manifest().unwrap().revision, 0);
    assert_eq!(project.manifest().unwrap().name, "한글 프로젝트");
    assert_eq!(
        project
            .read_sql(Path::new("queries/existing.sql"))
            .unwrap()
            .sql,
        "SELECT 'kept'"
    );
    let original = fs::read(root.path().join(".dbide/project.json")).unwrap();
    assert_code(Project::init(root.path(), "Reset"), "already_exists");
    assert_eq!(
        fs::read(root.path().join(".dbide/project.json")).unwrap(),
        original
    );
    assert_code(
        Project::init(&root.path().join("absent"), "No"),
        "not_found",
    );
    assert!(!root.path().join("absent").exists());
}

#[test]
fn open_never_initializes_or_creates_a_lock() {
    let root = tempfile::tempdir().unwrap();
    assert_code(Project::open(root.path()), "not_found");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    fs::create_dir(root.path().join(".dbide")).unwrap();
    fs::write(
        root.path().join(".dbide/project.json"),
        r#"{"schema_version":1,"name":"Read only","revision":4,"default_target":null}"#,
    )
    .unwrap();
    Project::open(root.path()).unwrap();
    assert!(!root.path().join(".dbide/project.lock").exists());
}

#[test]
fn gui_and_cli_handles_compare_document_bytes_before_save() {
    let (root, gui) = fixture();
    let cli = Project::open(root.path()).unwrap();
    let initial = gui
        .write_sql(Path::new("queries/shared.sql"), "SELECT 1", None)
        .unwrap();
    let gui_loaded = gui.read_sql(Path::new("queries/shared.sql")).unwrap();
    let cli_written = cli
        .write_sql(
            Path::new("queries/shared.sql"),
            "SELECT 2",
            Some(&initial.sha256),
        )
        .unwrap();
    assert_code(
        gui.write_sql(
            Path::new("queries/shared.sql"),
            "SELECT 3",
            Some(&gui_loaded.sha256),
        ),
        "conflict",
    );
    assert_eq!(
        gui.read_sql(Path::new("queries/shared.sql")).unwrap(),
        cli_written
    );
    assert_code(
        cli.write_sql(Path::new("queries/shared.sql"), "overwrite", None),
        "already_exists",
    );
    assert_eq!(
        fs::read_to_string(root.path().join("queries/shared.sql")).unwrap(),
        "SELECT 2"
    );
}

#[test]
fn concurrent_cooperating_writers_publish_at_most_one_revision() {
    use std::sync::{Arc, Barrier};
    let (_root, project) = fixture();
    let first = project
        .write_sql(Path::new("queries/race.sql"), "SELECT 0", None)
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = ["SELECT 1", "SELECT 2"]
        .into_iter()
        .map(|sql| {
            let project = project.clone();
            let barrier = barrier.clone();
            let expected = first.sha256.clone();
            std::thread::spawn(move || {
                barrier.wait();
                project.write_sql(Path::new("queries/race.sql"), sql, Some(&expected))
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| matches!(error, ProjectError::Busy | ProjectError::Conflict))
    );
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(
        project.read_sql(Path::new("queries/race.sql")).unwrap(),
        winner
    );
}

#[test]
fn external_edits_and_deletions_after_load_are_conflicts() {
    let (root, project) = fixture();
    let original = project
        .write_sql(Path::new("queries/edit.sql"), "SELECT 1", None)
        .unwrap();
    let path = root.path().join("queries/edit.sql");
    fs::write(&path, "SELECT 'external'").unwrap();
    assert_code(
        project.write_sql(
            Path::new("queries/edit.sql"),
            "SELECT 2",
            Some(&original.sha256),
        ),
        "conflict",
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "SELECT 'external'");
    fs::remove_file(&path).unwrap();
    assert_code(
        project.write_sql(
            Path::new("queries/edit.sql"),
            "SELECT 2",
            Some(&original.sha256),
        ),
        "conflict",
    );
    assert!(!path.exists());
}

#[test]
fn empty_drafts_unicode_and_exact_byte_bounds_round_trip() {
    let (root, project) = fixture();
    let empty = project
        .write_sql(Path::new("queries/empty.SQL"), "", None)
        .unwrap();
    assert_eq!(empty.sha256, digest_sql(""));
    assert_eq!(empty.sql, "");
    let text = "가".repeat(MAX_SQL_BYTES / 3) + "a";
    assert_eq!(text.len(), MAX_SQL_BYTES);
    let full = project
        .write_sql(Path::new("queries/full.sql"), &text, None)
        .unwrap();
    assert_eq!(
        project.read_sql(Path::new("queries/full.sql")).unwrap(),
        full
    );
    assert_code(
        project.write_sql(
            Path::new("queries/full.sql"),
            &(text.clone() + "x"),
            Some(&full.sha256),
        ),
        "too_large",
    );
    assert_eq!(
        project.read_sql(Path::new("queries/full.sql")).unwrap(),
        full
    );
    fs::write(
        root.path().join("queries/external.sql"),
        vec![b'x'; MAX_SQL_BYTES + 1],
    )
    .unwrap();
    assert_code(
        project.read_sql(Path::new("queries/external.sql")),
        "too_large",
    );
    fs::write(root.path().join("queries/binary.sql"), [0xff, 0xfe]).unwrap();
    assert_code(
        project.read_sql(Path::new("queries/binary.sql")),
        "invalid_utf8",
    );
}

#[test]
fn traversal_hidden_metadata_and_non_sql_paths_are_refused() {
    let (_root, project) = fixture();
    for path in [
        "/tmp/out.sql",
        "../out.sql",
        "queries/../../out.sql",
        ".dbide/project.sql",
        "queries/.hidden.sql",
        "queries/data.txt",
        "queries/x\\y.sql",
        "",
    ] {
        assert_code(project.read_sql(Path::new(path)), "invalid_path");
        assert_code(
            project.write_sql(Path::new(path), "SELECT 1", None),
            "invalid_path",
        );
    }
    for path in ["..", ".dbide", "queries/..", "/tmp", ""] {
        assert_code(project.mkdir(Path::new(path)), "invalid_path");
    }
    assert_code(project.list(Path::new(".dbide")), "invalid_path");
    assert_code(project.list(Path::new("../")), "invalid_path");
    assert_code(project.mkdir(Path::new("missing/nested")), "not_found");
    assert_eq!(
        project.mkdir(Path::new("queries/nested")).unwrap(),
        Path::new("queries/nested")
    );
    assert_code(project.mkdir(Path::new("queries/nested")), "already_exists");
}

#[test]
fn listing_is_one_directory_sorted_and_does_not_read_sql_contents() {
    let (root, project) = fixture();
    fs::create_dir(root.path().join("queries/a-dir")).unwrap();
    fs::write(root.path().join("queries/a-dir/deep.sql"), "nested").unwrap();
    fs::write(
        root.path().join("queries/z.sql"),
        vec![0xff; MAX_SQL_BYTES + 100],
    )
    .unwrap();
    fs::write(root.path().join("queries/a.sql"), "").unwrap();
    fs::write(root.path().join("queries/ignored.txt"), "").unwrap();
    fs::write(root.path().join("queries/.hidden.sql"), "").unwrap();
    let listing = project.list(Path::new("queries")).unwrap();
    assert!(!listing.truncated);
    assert_eq!(
        listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["a-dir", "a.sql", "z.sql"]
    );
    assert_eq!(listing.entries[0].kind, EntryKind::Directory);
    assert_eq!(listing.entries[1].kind, EntryKind::SqlFile);
    assert_eq!(
        project
            .list(Path::new("queries/a-dir"))
            .unwrap()
            .entries
            .len(),
        1
    );
    assert_eq!(project.list(Path::new(".")).unwrap().entries.len(), 1);
}

#[test]
fn listing_reports_both_return_and_scan_limits() {
    let (root, project) = fixture();
    for index in 0..MAX_LIST_ENTRIES + 1 {
        fs::write(root.path().join(format!("queries/{index:04}.sql")), "").unwrap();
    }
    let listing = project.list(Path::new("queries")).unwrap();
    assert!(listing.truncated);
    assert_eq!(listing.entries.len(), MAX_LIST_ENTRIES);
    assert!(
        listing
            .entries
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
    );
    project.mkdir(Path::new("many-ignored")).unwrap();
    for index in 0..MAX_SCANNED_ENTRIES + 1 {
        fs::write(root.path().join(format!("many-ignored/{index}.txt")), "").unwrap();
    }
    let listing = project.list(Path::new("many-ignored")).unwrap();
    assert!(listing.truncated);
    assert!(listing.entries.is_empty());
}

#[test]
fn manifest_target_is_explicit_password_free_and_revision_checked() {
    let (root, first) = fixture();
    let second = Project::open(root.path()).unwrap();
    let target =
        StoredExecutionTarget::capture(&ConnectionProfile::fixture(), "missing_database").unwrap();
    let next = first.set_default_target(Some(target.clone()), 0).unwrap();
    assert_eq!(next.revision, 1);
    assert_eq!(next.default_target, Some(target));
    assert_code(second.set_default_target(None, 0), "conflict");
    assert_eq!(second.manifest().unwrap(), next);
    let cleared = second.set_default_target(None, 1).unwrap();
    assert_eq!(cleared.revision, 2);
    assert_eq!(cleared.default_target, None);
    let mut bad = next.default_target.unwrap();
    bad.endpoint_fingerprint = "invalid".into();
    assert_code(first.set_default_target(Some(bad), 2), "invalid_target");
    assert_eq!(first.manifest().unwrap(), cleared);
    let serialized = fs::read_to_string(root.path().join(".dbide/project.json")).unwrap();
    assert!(!serialized.contains("password"));
    assert!(!serialized.contains("host"));
}

#[test]
fn corrupt_future_and_oversized_manifests_are_preserved_even_after_open() {
    let (root, project) = fixture();
    let path = root.path().join(".dbide/project.json");
    for (bytes, expected) in [
        (br#"{"schema_version":99,"future":true}"#.to_vec(), "unsupported_version"),
        (b"{broken".to_vec(), "invalid_manifest"),
        (br#"{"schema_version":1,"name":"Secret","revision":0,"default_target":null,"password":"rejected"}"#.to_vec(), "invalid_manifest"),
        (vec![b' '; MAX_MANIFEST_BYTES + 1], "too_large"),
    ] {
        fs::write(&path, &bytes).unwrap();
        assert_code(Project::open(root.path()), expected);
        assert_code(project.set_default_target(None, 0), expected);
        assert_code(project.write_sql(Path::new("queries/new.sql"), "SELECT 1", None), expected);
        assert_code(project.mkdir(Path::new("new")), expected);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!root.path().join("queries/new.sql").exists());
    }
}

#[test]
fn held_writer_lock_fails_immediately_and_does_not_publish() {
    let (root, project) = fixture();
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path().join(".dbide/project.lock"))
        .unwrap();
    file.try_lock().unwrap();
    assert_code(project.set_default_target(None, 0), "busy");
    assert_code(
        project.write_sql(Path::new("queries/busy.sql"), "SELECT 1", None),
        "busy",
    );
    assert_code(project.mkdir(Path::new("busy")), "busy");
    assert_eq!(project.manifest().unwrap().revision, 0);
    assert!(!root.path().join("queries/busy.sql").exists());
}

#[cfg(unix)]
#[test]
fn symlink_components_and_special_files_never_escape_or_block() {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::symlink},
    };
    let (root, project) = fixture();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("outside.sql"), "external").unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();
    symlink(
        outside.path().join("outside.sql"),
        root.path().join("queries/link.sql"),
    )
    .unwrap();
    assert_code(project.list(Path::new("escape")), "invalid_path");
    assert_code(
        project.read_sql(Path::new("escape/outside.sql")),
        "invalid_path",
    );
    assert_code(
        project.write_sql(Path::new("escape/outside.sql"), "changed", None),
        "invalid_path",
    );
    assert_code(
        project.read_sql(Path::new("queries/link.sql")),
        "invalid_path",
    );
    assert_code(
        project.write_sql(
            Path::new("queries/link.sql"),
            "changed",
            Some(&digest_sql("external")),
        ),
        "invalid_path",
    );
    assert_eq!(
        fs::read_to_string(outside.path().join("outside.sql")).unwrap(),
        "external"
    );
    let fifo = root.path().join("queries/pipe.sql");
    let c_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: mkfifo receives a valid NUL-terminated temporary test path.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    assert_code(
        project.read_sql(Path::new("queries/pipe.sql")),
        "invalid_path",
    );
    assert_code(
        project.write_sql(Path::new("queries/pipe.sql"), "changed", Some("hash")),
        "invalid_path",
    );
    let socket_path = root.path().join("queries/socket.sql");
    let _socket = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    assert_code(
        project.read_sql(Path::new("queries/socket.sql")),
        "invalid_path",
    );
    assert!(
        project
            .list(Path::new("queries"))
            .unwrap()
            .entries
            .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn metadata_symlinks_and_fifo_locks_are_refused() {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::symlink},
    };
    let (root, project) = fixture();
    let manifest_path = root.path().join(".dbide/project.json");
    let outside = tempfile::tempdir().unwrap();
    let bytes = fs::read(&manifest_path).unwrap();
    fs::write(outside.path().join("manifest.json"), &bytes).unwrap();
    fs::remove_file(&manifest_path).unwrap();
    symlink(outside.path().join("manifest.json"), &manifest_path).unwrap();
    assert_code(Project::open(root.path()), "invalid_path");
    assert_code(project.set_default_target(None, 0), "invalid_path");
    assert_eq!(
        fs::read(outside.path().join("manifest.json")).unwrap(),
        bytes
    );
    let lock_path = root.path().join(".dbide/project.lock");
    fs::remove_file(&lock_path).unwrap();
    let c_path = CString::new(lock_path.as_os_str().as_bytes()).unwrap();
    // SAFETY: mkfifo receives a valid NUL-terminated temporary test path.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    assert_code(project.mkdir(Path::new("refused")), "invalid_path");
    let other = tempfile::tempdir().unwrap();
    symlink(root.path().join(".dbide"), other.path().join(".dbide")).unwrap();
    assert_code(Project::open(other.path()), "invalid_path");
    assert_code(Project::init(other.path(), "refused"), "invalid_path");
}

#[cfg(unix)]
#[test]
fn new_metadata_and_sql_files_are_private_and_failures_leave_no_temporaries() {
    use std::os::unix::fs::PermissionsExt;
    let (root, project) = fixture();
    let file = project
        .write_sql(Path::new("queries/private.sql"), "SELECT 1", None)
        .unwrap();
    project
        .write_sql(Path::new("queries/private.sql"), "", Some(&file.sha256))
        .unwrap();
    for relative in [
        ".dbide/project.json",
        ".dbide/project.lock",
        "queries/private.sql",
    ] {
        assert_eq!(
            fs::metadata(root.path().join(relative))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    fs::create_dir(root.path().join("queries/directory.sql")).unwrap();
    assert_code(
        project.write_sql(Path::new("queries/directory.sql"), "SELECT 2", Some("hash")),
        "invalid_path",
    );
    for directory in ["queries", ".dbide"] {
        assert!(
            fs::read_dir(root.path().join(directory))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".dbide-"))
        );
    }
}
