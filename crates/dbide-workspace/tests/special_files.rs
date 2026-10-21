#![cfg(unix)]
//! A child deadline makes the no-writer FIFO regression safe even if blocking returns.
use dbide_workspace::{WorkspaceSnapshot, WorkspaceStore};
use std::{
    ffi::CString,
    fs,
    os::unix::ffi::OsStrExt,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn fifo(path: &Path) {
    let path = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: the owned NUL-terminated path remains alive for this call; mode has no pointers.
    assert_eq!(
        unsafe { libc::mkfifo(path.as_ptr(), 0o600) },
        0,
        "{}",
        std::io::Error::last_os_error()
    );
}

#[test]
fn special_file_child() {
    let Some(path) = std::env::var_os("DBIDE_WORKSPACE_SPECIAL_FILE") else {
        return;
    };
    let case = std::env::var("DBIDE_WORKSPACE_SPECIAL_FILE_CASE").unwrap();
    if case.ends_with("snapshot") {
        assert!(WorkspaceStore::open(path).is_err());
    } else {
        let store = WorkspaceStore::open(path).unwrap();
        assert!(store.save(&WorkspaceSnapshot::default()).is_err());
    }
}

#[test]
fn no_writer_fifo_and_fifo_lock_fail_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    for case in ["snapshot", "lock", "symlink_snapshot", "symlink_lock"] {
        let path = dir.path().join(format!("{case}.json"));
        let special = if case.ends_with("snapshot") {
            path.clone()
        } else {
            path.with_extension("json.lock")
        };
        if case.starts_with("symlink_") {
            let target = dir.path().join(format!("{case}-target.fifo"));
            fifo(&target);
            std::os::unix::fs::symlink(&target, &special).unwrap();
        } else {
            fifo(&special);
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "special_file_child"])
            .env("DBIDE_WORKSPACE_SPECIAL_FILE", &path)
            .env("DBIDE_WORKSPACE_SPECIAL_FILE_CASE", case)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "{case} child failed");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("{case} FIFO blocked beyond deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!fs::metadata(special).unwrap().is_file());
    }
}

#[test]
fn directories_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    assert!(WorkspaceStore::open(dir.path()).is_err());
    let path = dir.path().join("settings.json");
    fs::create_dir(path.with_extension("json.lock")).unwrap();
    let store = WorkspaceStore::open(&path).unwrap();
    assert!(store.save(&WorkspaceSnapshot::default()).is_err());
    assert!(!path.exists());
}

#[test]
fn regular_snapshot_symlinks_remain_supported() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.json");
    WorkspaceStore::open(&target)
        .unwrap()
        .save(&WorkspaceSnapshot::default())
        .unwrap();
    let link = dir.path().join("link.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    WorkspaceStore::open(link).unwrap().load().unwrap();
}
