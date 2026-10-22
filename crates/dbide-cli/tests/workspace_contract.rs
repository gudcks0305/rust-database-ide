use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dbide"))
        .arg("--config")
        .arg(dir.join("profiles.json"))
        .args(args)
        .env_remove("PGPASSWORD")
        .env_remove("DATABASE_URL")
        .env_remove("MYSQL_PWD")
        .output()
        .unwrap()
}
fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], true);
    value
}
fn workspace(dir: &Path, args: &[&str]) -> Output {
    let mut argv = vec!["workspace", "--root", dir.to_str().unwrap()];
    argv.extend_from_slice(args);
    run(dir, &argv)
}
fn init(dir: &Path) {
    success(workspace(dir, &["init", "--name", "Fixture"]));
}
fn add(dir: &Path) {
    let output = run(
        dir,
        &[
            "profiles",
            "add",
            "--id",
            "fixture",
            "--name",
            "Owned fixture",
            "--engine",
            "sqlite",
            "--sqlite-path",
            dir.join("never-created.db").to_str().unwrap(),
            "--sqlite-create",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn workspace_file_workflow_and_bounded_context_do_not_connect_or_expose_sql() {
    let d = TempDir::new().unwrap();
    init(d.path());
    success(workspace(d.path(), &["mkdir", "--path", "queries/nested"]));
    let sql = "SELECT 'PRIVATE_SQL_SENTINEL';";
    let written = success(workspace(
        d.path(),
        &["write", "--file", "queries/nested/test.sql", "--sql", sql],
    ));
    let read = success(workspace(
        d.path(),
        &["read", "--file", "queries/nested/test.sql"],
    ));
    assert_eq!(read["file"]["sql"], sql);
    assert_eq!(read["file"]["sha256"], written["file"]["sha256"]);
    let files = success(workspace(
        d.path(),
        &["files", "--directory", "queries/nested"],
    ));
    assert_eq!(files["listing"]["entries"][0]["name"], "test.sql");
    let context = success(workspace(d.path(), &["context"]));
    assert_eq!(context["target_resolution"]["status"], "unbound");
    assert!(!context.to_string().contains("PRIVATE_SQL_SENTINEL"));
    assert_eq!(context["included"]["sql_content"], false);
    assert!(!d.path().join("profiles.json").exists());
    let conflict = workspace(
        d.path(),
        &[
            "write",
            "--file",
            "queries/nested/test.sql",
            "--sql",
            "SELECT 2",
            "--expect-sha256",
            &"0".repeat(64),
        ],
    );
    assert_eq!(conflict.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&conflict.stdout).unwrap();
    assert_eq!(error["error"]["code"], "conflict");
    assert_eq!(
        fs::read_to_string(d.path().join("queries/nested/test.sql")).unwrap(),
        sql
    );
    success(workspace(
        d.path(),
        &[
            "write",
            "--file",
            "queries/nested/test.sql",
            "--sql",
            "SELECT 3",
            "--expect-sha256",
            written["file"]["sha256"].as_str().unwrap(),
        ],
    ));
}

#[test]
fn plan_requires_binding_and_its_guards_refuse_before_auth_output_or_database_creation() {
    let d = TempDir::new().unwrap();
    init(d.path());
    success(workspace(
        d.path(),
        &["write", "--file", "queries/test.sql", "--sql", "SELECT 1;"],
    ));
    add(d.path());
    let missing = workspace(d.path(), &["plan", "--file", "queries/test.sql"]);
    assert_eq!(missing.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&missing.stdout).unwrap()["error"]["code"],
        "target_required"
    );
    success(workspace(
        d.path(),
        &["bind", "--profile", "fixture", "--expect-revision", "0"],
    ));
    let plan = success(workspace(d.path(), &["plan", "--file", "queries/test.sql"]));
    assert_eq!(plan["executed"], false);
    assert_eq!(plan["intended_target"]["database"], "main");
    let argv: Vec<String> = plan["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(argv.contains(&"--read-only".into()));
    assert!(!d.path().join("never-created.db").exists());
    // Even an explicitly missing password variable must not be consulted on guard failure.
    fs::write(d.path().join("queries/test.sql"), "SELECT 2;").unwrap();
    let output = d.path().join("must-not-create.json");
    let refused = Command::new(env!("CARGO_BIN_EXE_dbide"))
        .args(&argv[1..])
        .args([
            "--password-env",
            "DBIDE_WORKSPACE_TEST_MISSING",
            "--output",
            output.to_str().unwrap(),
        ])
        .env_remove("DBIDE_WORKSPACE_TEST_MISSING")
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("SQL SHA-256 changed"));
    assert!(!output.exists());
    assert!(!d.path().join("never-created.db").exists());
    fs::write(d.path().join("queries/test.sql"), "SELECT 1;").unwrap();
    let edited = run(
        d.path(),
        &[
            "profiles",
            "edit",
            "fixture",
            "--sqlite-path",
            d.path().join("different.db").to_str().unwrap(),
        ],
    );
    assert!(edited.status.success());
    let refused = Command::new(env!("CARGO_BIN_EXE_dbide"))
        .args(&argv[1..])
        .args([
            "--password-env",
            "DBIDE_WORKSPACE_TEST_MISSING",
            "--output",
            output.to_str().unwrap(),
        ])
        .env_remove("DBIDE_WORKSPACE_TEST_MISSING")
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("target fingerprint changed"));
    assert!(!output.exists());
    assert!(!d.path().join("different.db").exists());
    let context = success(workspace(d.path(), &["context"]));
    assert_eq!(context["target_resolution"]["status"], "unresolved");
    assert_eq!(
        workspace(d.path(), &["plan", "--file", "queries/test.sql"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn offline_capabilities_and_explicit_root_contract() {
    let d = TempDir::new().unwrap();
    let value = success(run(d.path(), &["agent", "capabilities"]));
    assert_eq!(value["limits"]["sql_bytes"], 65536);
    assert_eq!(fs::read_dir(d.path()).unwrap().count(), 0);
    let missing = run(d.path(), &["workspace", "show"]);
    assert_eq!(missing.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&missing.stdout).unwrap()["error"]["code"],
        "root_required"
    );
    init(d.path());
    // Root flag after the subcommand is equally explicit and accepted.
    success(run(
        d.path(),
        &["workspace", "show", "--root", d.path().to_str().unwrap()],
    ));
}

#[test]
fn relative_sqlite_targets_are_refused_before_authentication_or_side_effects() {
    let d = TempDir::new().unwrap();
    init(d.path());
    add(d.path());
    success(workspace(
        d.path(),
        &["write", "--file", "queries/test.sql", "--sql", "SELECT 1;"],
    ));
    // GUI/imported profiles can contain a path that CLI add would normalize.
    let path = d.path().join("profiles.json");
    let mut state: dbide_profiles::WorkspaceState =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state.profiles[0].sqlite_path = Some("ambiguous.db".into());
    fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    let bound = workspace(
        d.path(),
        &["bind", "--profile", "fixture", "--expect-revision", "0"],
    );
    assert_eq!(bound.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&bound.stdout).unwrap()["error"]["code"],
        "relative_sqlite_path"
    );
    // Reproduce a preexisting/imported manifest with an otherwise matching fingerprint.
    let target =
        dbide_profiles::StoredExecutionTarget::capture(&state.profiles[0], "main").unwrap();
    dbide_project::Project::open(d.path())
        .unwrap()
        .set_default_target(Some(target.clone()), 0)
        .unwrap();
    let context = success(workspace(d.path(), &["context"]));
    assert_eq!(context["target_resolution"]["status"], "unresolved");
    assert_eq!(context["target_resolution"]["code"], "relative_sqlite_path");
    let plan = workspace(d.path(), &["plan", "--file", "queries/test.sql"]);
    assert_eq!(plan.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&plan.stdout).unwrap()["error"]["code"],
        "relative_sqlite_path"
    );
    for cwd in [d.path().join("one"), d.path().join("two")] {
        fs::create_dir(&cwd).unwrap();
        let output_path = cwd.join("must-not-create.json");
        let output = Command::new(env!("CARGO_BIN_EXE_dbide"))
            .arg("--config")
            .arg(&path)
            .args([
                "query",
                "--profile",
                "fixture",
                "--sql",
                "SELECT 1;",
                "--expect-target-fingerprint",
                &target.endpoint_fingerprint,
                "--password-env",
                "DBIDE_WORKSPACE_TEST_MISSING",
                "--output",
            ])
            .arg(&output_path)
            .current_dir(&cwd)
            .env_remove("DBIDE_WORKSPACE_TEST_MISSING")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("absolute SQLite path or :memory:")
        );
        assert!(!cwd.join("ambiguous.db").exists());
        assert!(!output_path.exists());
    }
}

#[cfg(unix)]
#[test]
fn planned_sql_replaced_with_fifo_is_refused_without_blocking_or_side_effects() {
    use std::{
        ffi::CString,
        os::unix::ffi::OsStrExt,
        process::Stdio,
        time::{Duration, Instant},
    };
    let d = TempDir::new().unwrap();
    init(d.path());
    add(d.path());
    success(workspace(
        d.path(),
        &["write", "--file", "queries/test.sql", "--sql", "SELECT 1;"],
    ));
    success(workspace(
        d.path(),
        &["bind", "--profile", "fixture", "--expect-revision", "0"],
    ));
    let plan = success(workspace(d.path(), &["plan", "--file", "queries/test.sql"]));
    let argv: Vec<&str> = plan["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let sql_path = d.path().join("queries/test.sql");
    fs::remove_file(&sql_path).unwrap();
    let fifo_path = CString::new(sql_path.as_os_str().as_bytes()).unwrap();
    // SAFETY: fifo_path is a valid NUL-terminated, owned pathname in our temp directory.
    assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
    let output_path = d.path().join("must-not-create.json");
    let mut child = Command::new(env!("CARGO_BIN_EXE_dbide"))
        .args(&argv[1..])
        .args(["--password-env", "DBIDE_WORKSPACE_TEST_MISSING", "--output"])
        .arg(&output_path)
        .env_remove("DBIDE_WORKSPACE_TEST_MISSING")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("query blocked opening a FIFO in place of planned SQL");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("SQL --file input must be a regular file")
    );
    assert!(!output_path.exists());
    assert!(!d.path().join("never-created.db").exists());
}
