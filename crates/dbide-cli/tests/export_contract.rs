use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dbide"));
    command
        .arg("--config")
        .arg(dir.join("profiles.json"))
        .args(args)
        .env_remove("PGPASSWORD")
        .env_remove("DATABASE_URL")
        .env_remove("MYSQL_PWD");
    command
}
fn run(dir: &Path, args: &[&str]) -> Output {
    command(dir, args).output().unwrap()
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    success(run(
        dir.path(),
        &[
            "profiles",
            "add",
            "--id",
            "fixture",
            "--name",
            "Owned export fixture",
            "--engine",
            "sqlite",
            "--sqlite-path",
            dir.path().join("source.db").to_str().unwrap(),
            "--sqlite-create",
        ],
    ));
    success(run(
        dir.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            "CREATE TABLE original(v); INSERT INTO original VALUES(7)",
            "--format",
            "json",
        ],
    ));
    dir
}
fn export(dir: &Path, sql: &str, name: &str, options: &[&str]) -> Output {
    let output = dir.join(name);
    let mut args = vec![
        "export",
        "--profile",
        "fixture",
        "--sql",
        sql,
        "--output",
        output.to_str().unwrap(),
    ];
    args.extend_from_slice(options);
    run(dir, &args)
}
fn not_published(dir: &Path, sql: &str, name: &str, options: &[&str]) -> Output {
    let output = export(dir, sql, name, options);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not published"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!dir.join(name).exists());
    output
}

#[test]
fn sqlite_export_exceeds_retained_caps_includes_last_row_and_captured_identity() {
    let dir = fixture();
    let before = fs::read(dir.path().join("source.db")).unwrap();
    let sql = "WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<6000) SELECT v AS id, printf('%0120d',v) AS payload FROM n";
    let statistics: Value = serde_json::from_str(&success(export(
        dir.path(),
        sql,
        "full.csv",
        &["--format", "csv"],
    )))
    .unwrap();
    let bytes = fs::read(dir.path().join("full.csv")).unwrap();
    assert!(bytes.len() > 512 * 1024);
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(text.lines().count(), 6001);
    assert!(text.lines().last().unwrap().starts_with("6000,"));
    assert_eq!(statistics["rows"], 6000);
    assert_eq!(statistics["encoded_bytes"], text.len());
    assert_eq!(statistics["sql_sha256"], dbide_project::digest_sql(sql));
    assert_eq!(statistics["execution_target"]["profile_id"], "fixture");
    assert_eq!(statistics["execution_target"]["database"], "main");
    assert_eq!(statistics["read_only"], true);
    assert_eq!(statistics["independent_connection"], true);
    assert_eq!(fs::read(dir.path().join("source.db")).unwrap(), before);
    // Existing query remains bounded and reports truncation for the same SQL.
    let query = run(
        dir.path(),
        &[
            "query",
            "--profile",
            "fixture",
            "--sql",
            sql,
            "--format",
            "json",
        ],
    );
    assert_eq!(query.status.code(), Some(5));
    assert_eq!(
        serde_json::from_slice::<Value>(&query.stdout).unwrap()["truncated"],
        true
    );
}

#[test]
fn csv_preserves_null_empty_unicode_labels_and_explicit_formula_mode() {
    let dir = fixture();
    let sql = "SELECT NULL AS \"\", '' AS duplicate, '한글,\"quote\"' || char(10) || 'line' AS duplicate, '=1+2' AS formula";
    success(export(dir.path(), sql, "values.csv", &[]));
    assert_eq!(
        fs::read_to_string(dir.path().join("values.csv")).unwrap(),
        ",duplicate,duplicate,formula\n\\N,,\"한글,\"\"quote\"\"\nline\",=1+2\n"
    );
    success(export(
        dir.path(),
        "SELECT '=1+2' AS v",
        "escaped.csv",
        &["--escape-formulas"],
    ));
    assert_eq!(
        fs::read_to_string(dir.path().join("escaped.csv")).unwrap(),
        "v\n'=1+2\n"
    );
    success(export(
        dir.path(),
        "SELECT 1 AS v WHERE 0; -- comment-only tail",
        "empty.csv",
        &[],
    ));
    assert_eq!(
        fs::read_to_string(dir.path().join("empty.csv")).unwrap(),
        "v\n"
    );
    not_published(dir.path(), "SELECT '\\N' AS v", "collision.csv", &[]);
    success(export(
        dir.path(),
        "SELECT '\\N' AS v",
        "marker.csv",
        &["--null-marker", "<NULL>"],
    ));
}

#[test]
fn partial_source_encoding_and_budget_failures_never_publish_or_mutate_source() {
    let dir = fixture();
    let before = fs::read(dir.path().join("source.db")).unwrap();
    for (index, sql) in [
        "SELECT 'first' AS v UNION ALL SELECT abs(-9223372036854775808)",
        "SELECT 'first' AS v UNION ALL SELECT '\\N'",
        "SELECT CAST(x'80' AS TEXT)",
        "SELECT x'ff'",
        "SELECT 1e999",
        "INSERT INTO original VALUES(9)",
        "CREATE TEMP TABLE bypass(v)",
        "PRAGMA query_only=OFF",
        "PRAGMA user_version=42",
        "ATTACH ':memory:' AS bypass",
        "SELECT 1; SELECT 2",
        "SELECT 1; PRAGMA user_version=42",
    ]
    .into_iter()
    .enumerate()
    {
        not_published(dir.path(), sql, &format!("refused-{index}.csv"), &[]);
    }
    for (index, options) in [
        vec!["--max-rows", "1"],
        vec!["--max-row-bytes", "20"],
        vec!["--max-bytes", "32"],
    ]
    .into_iter()
    .enumerate()
    {
        not_published(
            dir.path(),
            "SELECT '123456789012345678901234567890' AS v UNION ALL SELECT 'tail'",
            &format!("budget-{index}.csv"),
            &options,
        );
    }
    // Encoded quote expansion exceeds the output cap while logical bytes fit.
    not_published(
        dir.path(),
        "SELECT '\"\"\"\"\"\"\"\"\"\"' AS v",
        "encoded-cap.csv",
        &["--max-bytes", "16"],
    );
    assert_eq!(fs::read(dir.path().join("source.db")).unwrap(), before);
    assert!(!fs::read_dir(dir.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp")
    }));
}

#[test]
fn output_conflicts_reserved_paths_and_input_guards_refuse_without_replacement() {
    let dir = fixture();
    fs::write(dir.path().join("existing.csv"), "original").unwrap();
    let error = export(dir.path(), "SELECT 1", "existing.csv", &[]);
    assert_eq!(error.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(dir.path().join("existing.csv")).unwrap(),
        "original"
    );
    let before = fs::read(dir.path().join("source.db")).unwrap();
    assert!(
        !export(dir.path(), "SELECT 1", "source.db", &[])
            .status
            .success()
    );
    for name in [
        "source.db-wal",
        "source.db-shm",
        "source.db-journal",
        "SOURCE.DB-WAL",
        "SOURCE.DB-JOURNAL",
    ] {
        not_published(dir.path(), "SELECT 1", name, &[]);
    }
    not_published(
        dir.path(),
        "SELECT 1",
        "hash.csv",
        &["--expect-sql-sha256", "changed"],
    );
    not_published(
        dir.path(),
        "SELECT 1",
        "target.csv",
        &["--expect-target-fingerprint", "changed"],
    );
    not_published(dir.path(), "SELECT 1", "missing/out.csv", &[]);
    assert_eq!(fs::read(dir.path().join("source.db")).unwrap(), before);
    let state = dbide_profiles::ProfileStore::open(dir.path().join("profiles.json"))
        .unwrap()
        .load()
        .unwrap();
    let target =
        dbide_profiles::StoredExecutionTarget::capture(&state.profiles[0], "main").unwrap();
    success(export(
        dir.path(),
        "SELECT 1",
        "guarded.csv",
        &[
            "--expect-sql-sha256",
            &dbide_project::digest_sql("SELECT 1"),
            "--expect-target-fingerprint",
            &target.endpoint_fingerprint,
        ],
    ));
}

#[test]
fn relative_memory_and_non_sqlite_profiles_are_explicitly_unsupported() {
    let dir = fixture();
    for (index, path) in ["relative.db", ":memory:"].into_iter().enumerate() {
        // CLI profile editing intentionally resolves relative paths to absolute;
        // use the store API to model an older/external relative configuration.
        dbide_profiles::ProfileStore::open(dir.path().join("profiles.json"))
            .unwrap()
            .update(|state| {
                state.profiles[0].sqlite_path = Some(path.into());
                Ok(())
            })
            .unwrap();
        let output = not_published(dir.path(), "SELECT 1", &format!("invalid-{index}.csv"), &[]);
        assert_eq!(output.status.code(), Some(2));
    }
    success(run(
        dir.path(),
        &[
            "profiles",
            "add",
            "--id",
            "network",
            "--name",
            "Never connected",
            "--engine",
            "postgres",
        ],
    ));
    let output = run(
        dir.path(),
        &[
            "export",
            "--profile",
            "network",
            "--sql",
            "SELECT 1",
            "--output",
            dir.path().join("network.csv").to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("SQLite files only"));
    assert!(!dir.path().join("network.csv").exists());
}

#[test]
fn native_deadline_before_first_row_leaves_no_output() {
    let dir = fixture();
    let output = not_published(
        dir.path(),
        "WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<100000000) SELECT sum(v) FROM n",
        "deadline.csv",
        &["--timeout-secs", "1"],
    );
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stderr).contains("deadline"));
}

#[test]
#[cfg(unix)]
fn non_utf8_output_is_refused_before_temp_creation_or_sql_execution() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let dir = fixture();
    let output_path = dir
        .path()
        .join(OsString::from_vec(b"invalid-\xff.csv".to_vec()));
    let files = || {
        let mut names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before = files();
    // This would fail natively if dispatched; configuration refusal must win.
    let output = command(
        dir.path(),
        &[
            "export",
            "--profile",
            "fixture",
            "--sql",
            "SELECT * FROM must_not_be_prepared",
        ],
    )
    .arg("--output")
    .arg(&output_path)
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("output path must be UTF-8"), "{message}");
    assert!(message.contains("not published"), "{message}");
    assert!(!output_path.exists());
    assert_eq!(files(), before);
}

#[test]
#[cfg(unix)]
fn interrupt_during_native_execution_discards_the_provisional_file() {
    let dir = fixture();
    let output_path = dir.path().join("cancelled.csv");
    let mut child = command(dir.path(), &["export", "--profile", "fixture", "--sql",
        "WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<1000000000) SELECT sum(v) FROM n",
        "--output", output_path.to_str().unwrap()])
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::read_dir(dir.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp")
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "export never created its provisional output"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    // Only this test-owned process receives the signal.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("export did not observe cancellation");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not published"));
    assert!(!output_path.exists());
}
