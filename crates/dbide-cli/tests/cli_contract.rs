use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};
use tempfile::TempDir;
fn command(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_dbide"));
    c.arg("--config").arg(dir.join("profiles.json")).args(args);
    // Never route tests to actual App Support or discover native credentials.
    c.env_remove("PGPASSWORD")
        .env_remove("DATABASE_URL")
        .env_remove("MYSQL_PWD");
    c
}
fn run(dir: &Path, args: &[&str]) -> Output {
    command(dir, args).output().unwrap()
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn add(dir: &Path) {
    success(run(
        dir,
        &[
            "profiles",
            "add",
            "--id",
            "fixture",
            "--name",
            "CLI fixture",
            "--engine",
            "sqlite",
            "--sqlite-path",
            dir.join("fixture.db").to_str().unwrap(),
            "--sqlite-create",
        ],
    ));
}
fn query(dir: &Path, sql: &str, format: &str) -> Output {
    run(
        dir,
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            sql,
            "--format",
            format,
        ],
    )
}
#[test]
fn help_and_missing_arguments_are_actionable() {
    let d = TempDir::new().unwrap();
    assert!(success(run(d.path(), &["--help"])).contains("profiles"));
    for args in [
        &["profiles", "edit"][..],
        &["query"][..],
        &["config", "set"][..],
        &["sql", "format"][..],
    ] {
        let o = run(d.path(), args);
        assert_eq!(o.status.code(), Some(2));
        assert!(!o.stderr.is_empty());
    }
    success(run(d.path(), &["profiles", "test", "--help"]));
    success(run(d.path(), &["sql", "format", "--help"]));
    assert!(!d.path().join("profiles.json").exists());
}
#[test]
fn profile_config_workflow_and_recoverable_removal() {
    let d = TempDir::new().unwrap();
    add(d.path());
    assert!(
        !d.path().join("fixture.db").exists(),
        "add must not connect or create a DB"
    );
    success(run(
        d.path(),
        &[
            "profiles",
            "edit",
            "fixture",
            "--name",
            "Renamed",
            "--sqlite-create=false",
        ],
    ));
    success(run(d.path(), &["profiles", "select", "fixture"]));
    let value: Value = serde_json::from_str(&success(run(d.path(), &["config", "show"]))).unwrap();
    assert_eq!(value["profiles"][0]["name"], "Renamed");
    assert_eq!(value["selected_profile_id"], "fixture");
    success(run(d.path(), &["config", "set", "persist_history", "true"]));
    let before = fs::read(d.path().join("profiles.json")).unwrap();
    success(run(d.path(), &["profiles", "remove", "fixture"]));
    let backups: Vec<_> = fs::read_dir(d.path())
        .unwrap()
        .filter_map(|p| p.ok())
        .filter(|p| p.file_name().to_string_lossy().contains(".backup-"))
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read(backups[0].path()).unwrap(), before);
    let state: Value =
        serde_json::from_slice(&fs::read(d.path().join("profiles.json")).unwrap()).unwrap();
    assert!(state["profiles"].as_array().unwrap().is_empty());
    assert!(state["selected_profile_id"].is_null());
}
#[test]
fn sqlite_script_json_csv_schema_and_selected_profile() {
    let d = TempDir::new().unwrap();
    add(d.path());
    success(query(
        d.path(),
        "CREATE TABLE demo(id INTEGER, value TEXT); INSERT INTO demo VALUES(1, NULL), (2, ''); SELECT * FROM demo; SELECT '한글' AS duplicate, 'v' AS duplicate;",
        "json",
    ));
    let csv = success(query(
        d.path(),
        "SELECT value FROM demo ORDER BY id;",
        "csv",
    ));
    assert_eq!(csv, "value\n\\N\n\"\"\n");
    let error = query(d.path(), "SELECT 1; SELECT 2;", "csv");
    assert_eq!(error.status.code(), Some(2));
    assert!(error.stdout.is_empty());
    let chosen = success(run(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            "SELECT 1 AS same; SELECT 'second' AS same, 'x' AS same;",
            "--format",
            "csv",
            "--result-index",
            "1",
        ],
    ));
    assert_eq!(chosen, "same,same\nsecond,x\n");
    success(run(d.path(), &["profiles", "select", "fixture"]));
    let output = success(run(
        d.path(),
        &[
            "query",
            "--sql",
            "SELECT '한글' AS result;",
            "--format",
            "json",
        ],
    ));
    assert!(serde_json::from_str::<Value>(&output).is_ok());
    assert!(output.contains("한글"));
    let schema: Value =
        serde_json::from_str(&success(run(d.path(), &["schema", "--profile", "fixture"]))).unwrap();
    assert!(
        schema["objects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["name"] == "demo")
    );
    success(run(d.path(), &["profiles", "test", "fixture"]));
}
#[test]
fn malformed_config_is_never_overwritten() {
    let d = TempDir::new().unwrap();
    let path = d.path().join("profiles.json");
    let original = b"{malformed-password-sentinel";
    fs::write(&path, original).unwrap();
    for args in [
        &["config", "set", "persist_history", "true"][..],
        &["profiles", "remove", "fixture"][..],
    ] {
        let output = run(d.path(), args);
        assert_eq!(output.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("sentinel"));
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}
#[test]
fn passwords_are_explicit_and_never_written_and_history_is_opt_in() {
    let d = TempDir::new().unwrap();
    add(d.path());
    let mut c = command(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            "SELECT 1;",
            "--format",
            "json",
            "--password-env",
            "DBIDE_CLI_FIXTURE_PASSWORD",
        ],
    );
    c.env("DBIDE_CLI_FIXTURE_PASSWORD", "sensitive-test-only");
    let out = c.output().unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("sensitive-test-only"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("sensitive-test-only"));
    success(out);
    let bytes = fs::read(d.path().join("profiles.json")).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("sensitive-test-only"));
    assert!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["history"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    success(run(d.path(), &["config", "set", "persist_history", "true"]));
    success(query(d.path(), "SELECT 2;", "json"));
    let v: Value =
        serde_json::from_slice(&fs::read(d.path().join("profiles.json")).unwrap()).unwrap();
    assert_eq!(v["history"][0]["sql"], "SELECT 2;");
    success(run(
        d.path(),
        &["config", "set", "persist_history", "false"],
    ));
    let v: Value =
        serde_json::from_slice(&fs::read(d.path().join("profiles.json")).unwrap()).unwrap();
    assert!(v["history"].as_array().unwrap().is_empty());
    let bad = run(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--stdin",
            "--password-stdin",
        ],
    );
    assert_eq!(bad.status.code(), Some(2));
    let bad = run(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            "SELECT 1",
            "--password-env",
            "X",
            "--password-stdin",
        ],
    );
    assert_eq!(bad.status.code(), Some(2));
}

#[test]
fn history_save_failure_keeps_database_outcome_and_output_without_replay() {
    let d = TempDir::new().unwrap();
    add(d.path());
    success(query(
        d.path(),
        "CREATE TABLE executed_once(id INTEGER);",
        "json",
    ));
    let path = d.path().join("profiles.json");
    let store = dbide_profiles::ProfileStore::open(&path).unwrap();
    let mut state = store.load().unwrap();
    state.persist_history = true;
    // A valid file below 1 MiB. One additional maximum-size history item exceeds
    // the encoded store cap only after the real database execution has finished.
    for _ in 0..15 {
        state
            .record_history(dbide_profiles::HistoryEntry {
                profile_id: "fixture".into(),
                sql: "x".repeat(dbide_profiles::MAX_SQL_BYTES),
                executed_at_unix_ms: 1,
                outcome: "Completed".into(),
                row_count: Some(0),
            })
            .unwrap();
    }
    store.save(&state).unwrap();
    let original = fs::read(&path).unwrap();
    assert!(original.len() < dbide_profiles::MAX_FILE_BYTES);
    let padded_sql = |prefix: &str| {
        let padding = dbide_profiles::MAX_SQL_BYTES - prefix.len() - 5;
        format!("{prefix} /*{}*/", "x".repeat(padding))
    };
    let sql = padded_sql(
        "INSERT INTO executed_once VALUES (1); SELECT count(*) AS total FROM executed_once;",
    );
    assert_eq!(sql.len(), dbide_profiles::MAX_SQL_BYTES);
    let input = d.path().join("large.sql");
    fs::write(&input, &sql).unwrap();
    let assert_warning = |output: &Output| {
        let warning = String::from_utf8_lossy(&output.stderr);
        assert!(warning.contains("SQL already executed"), "{warning}");
        assert!(warning.contains("history persistence failed"), "{warning}");
        assert!(warning.contains("Do not replay SQL"), "{warning}");
    };
    let row_count = |report: &Value| {
        report["events"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|e| e.get("Row"))
            .unwrap()["values"][0]
            .clone()
    };
    let stdout = run(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--file",
            input.to_str().unwrap(),
            "--format",
            "json",
        ],
    );
    assert_warning(&stdout);
    let report: Value = serde_json::from_str(&success(stdout)).unwrap();
    assert_eq!(report["outcome"], "Completed");
    assert_eq!(row_count(&report), "1");
    assert_eq!(fs::read(&path).unwrap(), original);

    let exported = d.path().join("executed.json");
    let file = run(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--file",
            input.to_str().unwrap(),
            "--format",
            "json",
            "--output",
            exported.to_str().unwrap(),
        ],
    );
    assert_warning(&file);
    assert_eq!(success(file), "");
    let report: Value = serde_json::from_slice(&fs::read(&exported).unwrap()).unwrap();
    assert_eq!(report["outcome"], "Completed");
    assert_eq!(
        row_count(&report),
        "2",
        "each invocation must execute exactly once"
    );
    assert_eq!(fs::read(&path).unwrap(), original);

    let failed = query(
        d.path(),
        &padded_sql("SELECT * FROM deliberately_missing;"),
        "json",
    );
    assert_warning(&failed);
    assert_eq!(
        failed.status.code(),
        Some(4),
        "history failure must preserve the SQL failure code"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&failed.stdout).unwrap()["outcome"],
        "Failed"
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    let report: Value = serde_json::from_str(&success(query(
        d.path(),
        "SELECT count(*) AS total FROM executed_once;",
        "json",
    )))
    .unwrap();
    assert_eq!(row_count(&report), "2");
}
#[test]
fn stdin_file_formatter_readonly_and_input_limit() {
    let d = TempDir::new().unwrap();
    add(d.path());
    success(query(d.path(), "CREATE TABLE guarded(id INTEGER);", "json"));
    let denied = run(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--read-only",
            "--sql",
            "INSERT INTO guarded VALUES(1);",
            "--format",
            "json",
        ],
    );
    assert_eq!(denied.status.code(), Some(4));
    let mut child = command(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--stdin",
            "--format",
            "json",
        ],
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"SELECT 12;")
        .unwrap();
    success(child.wait_with_output().unwrap());
    let file = d.path().join("input.sql");
    fs::write(&file, "select 3 as value;").unwrap();
    success(run(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--file",
            file.to_str().unwrap(),
            "--format",
            "json",
        ],
    ));
    let formatted = success(run(
        d.path(),
        &[
            "sql",
            "format",
            "--file",
            file.to_str().unwrap(),
            "--dialect",
            "sqlite",
        ],
    ));
    assert!(formatted.contains("SELECT"));
    fs::write(&file, "x".repeat(65537)).unwrap();
    assert_eq!(
        run(
            d.path(),
            &[
                "query",
                "--profile",
                "fixture",
                "--file",
                file.to_str().unwrap()
            ]
        )
        .status
        .code(),
        Some(2)
    );
}
#[test]
fn output_is_atomic_refuses_overwrite_and_preserves_on_sql_or_csv_failure() {
    let d = TempDir::new().unwrap();
    add(d.path());
    let path = d.path().join("output.csv");
    fs::write(&path, b"original").unwrap();
    let args = [
        "query",
        "--profile",
        "fixture",
        "--sql",
        "CREATE TABLE should_not_exist(id INTEGER);",
        "--format",
        "json",
        "--output",
        path.to_str().unwrap(),
    ];
    assert_eq!(run(d.path(), &args).status.code(), Some(2));
    let absent = query(d.path(), "SELECT * FROM should_not_exist;", "json");
    assert_eq!(absent.status.code(), Some(4));
    let missing_parent = d.path().join("missing/result.csv");
    let rejected = run(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            "CREATE TABLE missing_parent_guard(id INTEGER);",
            "--format",
            "csv",
            "--output",
            missing_parent.to_str().unwrap(),
        ],
    );
    assert_eq!(rejected.status.code(), Some(2));
    assert_eq!(
        query(d.path(), "SELECT * FROM missing_parent_guard;", "json")
            .status
            .code(),
        Some(4)
    );
    for (sql, format) in [("SELECT * FROM missing;", "json"), ("SELECT '\\N';", "csv")] {
        let o = run(
            d.path(),
            &[
                "query",
                "--profile",
                "fixture",
                "--sql",
                sql,
                "--format",
                format,
                "--output",
                path.to_str().unwrap(),
                "--force",
            ],
        );
        assert!(!o.status.success());
        assert_eq!(fs::read(&path).unwrap(), b"original");
    }
    success(run(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            "SELECT 'fresh' AS value;",
            "--format",
            "csv",
            "--output",
            path.to_str().unwrap(),
            "--force",
        ],
    ));
    assert_eq!(fs::read_to_string(path).unwrap(), "value\nfresh\n");
}

#[test]
fn concurrent_profile_mutations_keep_all_profile_ids() {
    let d = TempDir::new().unwrap();
    let mut children = vec![];
    for index in 0..8 {
        children.push(
            command(
                d.path(),
                &[
                    "profiles",
                    "add",
                    "--id",
                    &format!("parallel-{index}"),
                    "--name",
                    "Parallel profile",
                    "--engine",
                    "sqlite",
                    "--sqlite-path",
                    ":memory:",
                ],
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
        );
    }
    for child in children {
        success(child.wait_with_output().unwrap());
    }
    let state: Value = serde_json::from_str(&success(run(d.path(), &["config", "show"]))).unwrap();
    assert_eq!(state["profiles"].as_array().unwrap().len(), 8);
}
#[test]
fn timeout_cancels_exact_operation_and_truncation_is_visible() {
    let d = TempDir::new().unwrap();
    add(d.path());
    let timed = run(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n) SELECT sum(x) FROM n;",
            "--timeout-secs",
            "1",
            "--format",
            "json",
        ],
    );
    assert_eq!(timed.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&timed.stderr).contains("cancel"));
    success(query(d.path(), "SELECT 1;", "json"));
    let truncated = query(
        d.path(),
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<6000) SELECT x FROM n;",
        "json",
    );
    assert_eq!(truncated.status.code(), Some(5));
    let v: Value = serde_json::from_slice(&truncated.stdout).unwrap();
    assert_eq!(v["truncated"], true);
    let output = d.path().join("previous.json");
    fs::write(&output, "original").unwrap();
    let incomplete = run(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<6000) SELECT x FROM n;",
            "--format",
            "json",
            "--output",
            output.to_str().unwrap(),
            "--force",
        ],
    );
    assert_eq!(incomplete.status.code(), Some(5));
    assert_eq!(fs::read_to_string(output).unwrap(), "original");
}

#[cfg(unix)]
#[test]
fn ctrl_c_requests_exact_cancel_and_returns_130() {
    let d = TempDir::new().unwrap();
    add(d.path());
    let child = command(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n) SELECT sum(x) FROM n;",
            "--timeout-secs",
            "30",
            "--format",
            "json",
        ],
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        Command::new("/bin/kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(130));
    assert!(String::from_utf8_lossy(&o.stderr).contains("targeted cancel"));
    success(query(d.path(), "SELECT 1;", "json"));
}

fn live_fixture(engine: &str, port: &str, username: &str) {
    assert_eq!(
        std::env::var("DBIDE_CLI_LIVE_FIXTURE").as_deref(),
        Ok("owned-loopback"),
        "explicit fixture opt-in required"
    );
    let d = TempDir::new().unwrap();
    success(run(
        d.path(),
        &[
            "profiles",
            "add",
            "--id",
            "fixture",
            "--name",
            "Owned loopback fixture",
            "--engine",
            engine,
            "--host",
            "127.0.0.1",
            "--port",
            port,
            "--database",
            "dbide_fixture",
            "--username",
            username,
            "--tls",
            "disable",
            "--connect-timeout-secs",
            "3",
        ],
    ));
    let mut c = command(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            "SELECT 1 AS first; SELECT NULL AS absent, '' AS empty_value;",
            "--format",
            "json",
            "--password-env",
            "DBIDE_CLI_OWNED_PASSWORD",
        ],
    );
    c.env("DBIDE_CLI_OWNED_PASSWORD", "dbide_fixture_only");
    let v: Value = serde_json::from_str(&success(c.output().unwrap())).unwrap();
    assert_eq!(v["outcome"], "Completed");
    let events = v["events"].as_array().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.get("ResultSetStarted").is_some())
            .count(),
        2
    );
    let rows: Vec<_> = events.iter().filter_map(|e| e.get("Row")).collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["values"], serde_json::json!(["1"]));
    assert_eq!(rows[1]["values"], serde_json::json!([null, ""]));
    let mut csv = command(
        d.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            "SELECT 1 AS first; SELECT NULL AS absent, '' AS empty_value;",
            "--format",
            "csv",
            "--result-index",
            "1",
            "--password-env",
            "DBIDE_CLI_OWNED_PASSWORD",
        ],
    );
    csv.env("DBIDE_CLI_OWNED_PASSWORD", "dbide_fixture_only");
    assert_eq!(success(csv.output().unwrap()), "absent,empty_value\n\\N,\n");
}
#[test]
#[ignore = "requires explicitly opted-in owned PostgreSQL loopback fixture on 55439"]
fn postgres_owned_fixture() {
    live_fixture("postgres", "55439", "postgres");
}
#[test]
#[ignore = "requires explicitly opted-in owned MySQL loopback fixture on 55441"]
fn mysql_owned_fixture() {
    live_fixture("mysql", "55441", "root");
}
