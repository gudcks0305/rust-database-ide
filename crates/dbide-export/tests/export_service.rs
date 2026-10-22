use dbide_export::{
    ExportErrorKind, ExportOptions, ExportProgress, ExportRequest, MAX_BYTES, MAX_ROW_BYTES,
    MAX_ROWS, MAX_TIMEOUT, export_csv,
};
use dbide_profiles::{ConnectionProfile, DatabaseEngine, StoredExecutionTarget};
use rusqlite::Connection;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn fixture() -> (tempfile::TempDir, ConnectionProfile) {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("owned-source.db");
    let connection = Connection::open(&source).unwrap();
    connection
        .execute_batch("CREATE TABLE original(v); INSERT INTO original VALUES(7)")
        .unwrap();
    drop(connection);
    let profile = ConnectionProfile {
        id: "owned-export-service-fixture".into(),
        name: "Owned export service fixture".into(),
        engine: DatabaseEngine::Sqlite,
        sqlite_path: Some(source.to_str().unwrap().into()),
        sqlite_create: false,
        ..ConnectionProfile::default()
    };
    (directory, profile)
}

fn request(profile: &ConnectionProfile, output: PathBuf, sql: &str) -> ExportRequest {
    ExportRequest {
        profile: profile.clone(),
        sql: sql.into(),
        output,
        options: ExportOptions::default(),
    }
}

fn export(
    request: ExportRequest,
) -> Result<dbide_export::ExportSummary, dbide_export::ExportError> {
    export_csv(
        request,
        Arc::new(AtomicBool::new(false)),
        Arc::new(ExportProgress::default()),
    )
}

fn no_provisional_files(directory: &Path) {
    assert!(fs::read_dir(directory).unwrap().all(|entry| {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        !name.starts_with(".tmp")
    }));
}

#[test]
fn independent_read_only_stream_reports_exact_identity_and_progress() {
    let (directory, profile) = fixture();
    let source = Path::new(profile.sqlite_path.as_deref().unwrap());
    let original = fs::read(source).unwrap();
    let output = directory.path().join("full.csv");
    let sql = "WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<6000) SELECT v AS id, printf('%0120d',v) AS payload FROM n";
    let progress = Arc::new(ExportProgress::default());
    let summary = export_csv(
        request(&profile, output.clone(), sql),
        Arc::new(AtomicBool::new(false)),
        progress.clone(),
    )
    .unwrap();
    let bytes = fs::read(&output).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert_eq!(text.lines().count(), 6001);
    assert!(text.lines().last().unwrap().starts_with("6000,"));
    assert!(bytes.len() > 512 * 1024);
    assert_eq!(summary.rows, 6000);
    assert_eq!(summary.encoded_bytes, bytes.len() as u64);
    assert_eq!(summary.payload_bytes, 9 + 6000 * 128);
    assert_eq!(
        summary.output,
        fs::canonicalize(output).unwrap().to_str().unwrap()
    );
    assert_eq!(summary.sql_sha256, dbide_project::digest_sql(sql));
    assert_eq!(
        summary.execution_target,
        StoredExecutionTarget::capture(&profile, "main").unwrap()
    );
    assert!(summary.read_only && summary.independent_connection && summary.ok);
    assert_eq!(summary.schema_version, 1);
    assert_eq!(summary.format, "csv");
    assert_eq!(progress.snapshot().rows, summary.rows);
    assert_eq!(progress.snapshot().payload_bytes, summary.payload_bytes);
    assert_eq!(fs::read(source).unwrap(), original);
    let json = serde_json::to_value(summary).unwrap();
    assert_eq!(json.as_object().unwrap().len(), 11);
    no_provisional_files(directory.path());
}

#[test]
fn empty_result_progress_contains_header_and_csv_preserves_null_empty_and_unicode() {
    let (directory, profile) = fixture();
    let progress = Arc::new(ExportProgress::default());
    let empty = directory.path().join("empty.csv");
    let summary = export_csv(
        request(&profile, empty.clone(), "SELECT 1 AS empty WHERE 0"),
        Arc::new(AtomicBool::new(false)),
        progress.clone(),
    )
    .unwrap();
    assert_eq!(summary.rows, 0);
    assert_eq!(summary.payload_bytes, 5);
    assert_eq!(progress.snapshot().rows, 0);
    assert_eq!(progress.snapshot().payload_bytes, 5);
    assert_eq!(fs::read_to_string(empty).unwrap(), "empty\n");
    let output = directory.path().join("values.csv");
    export(request(
        &profile,
        output.clone(),
        "SELECT NULL AS n, '' AS e, '한글' AS ko, '=1+2' AS formula",
    ))
    .unwrap();
    assert_eq!(
        fs::read_to_string(output).unwrap(),
        "n,e,ko,formula\n\\N,,한글,=1+2\n"
    );
    let mut escaped = request(
        &profile,
        directory.path().join("escaped.csv"),
        "SELECT '=1+2' AS formula",
    );
    escaped.options.escape_formulas = true;
    export(escaped.clone()).unwrap();
    assert_eq!(
        fs::read_to_string(escaped.output).unwrap(),
        "formula\n'=1+2\n"
    );
}

#[test]
fn precancelled_request_does_not_connect_or_create_output_or_temporary_file() {
    let (directory, mut profile) = fixture();
    profile.sqlite_path = Some(directory.path().join("missing.db").to_str().unwrap().into());
    profile.sqlite_create = true;
    let output = directory.path().join("cancelled.csv");
    let progress = Arc::new(ExportProgress::default());
    let error = export_csv(
        request(&profile, output.clone(), "SELECT 1"),
        Arc::new(AtomicBool::new(true)),
        progress.clone(),
    )
    .unwrap_err();
    assert_eq!(error.kind, ExportErrorKind::Cancelled);
    assert!(
        error
            .to_string()
            .contains("export output was not published")
    );
    assert!(!output.exists());
    assert!(!Path::new(profile.sqlite_path.as_deref().unwrap()).exists());
    assert_eq!(progress.snapshot().rows, 0);
    assert_eq!(progress.snapshot().payload_bytes, 0);
    no_provisional_files(directory.path());
}

#[test]
fn cancellation_observed_after_encoded_rows_discards_provisional_output() {
    let (directory, profile) = fixture();
    let output = directory.path().join("running.csv");
    let progress = Arc::new(ExportProgress::default());
    let cancelled = Arc::new(AtomicBool::new(false));
    let observed = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let monitor = {
        let progress = progress.clone();
        let cancelled = cancelled.clone();
        let observed = observed.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(5);
            while !done.load(Ordering::Acquire) && Instant::now() < until {
                if progress.snapshot().rows >= 32 {
                    observed.store(true, Ordering::Release);
                    cancelled.store(true, Ordering::Release);
                    break;
                }
                std::thread::yield_now();
            }
        })
    };
    let result = export_csv(
        request(
            &profile,
            output.clone(),
            "WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<10000000) SELECT v FROM n",
        ),
        cancelled,
        progress.clone(),
    );
    done.store(true, Ordering::Release);
    monitor.join().unwrap();
    assert!(observed.load(Ordering::Acquire));
    assert_eq!(result.unwrap_err().kind, ExportErrorKind::Cancelled);
    assert!(progress.snapshot().rows >= 32);
    assert!(!output.exists());
    no_provisional_files(directory.path());
}

#[test]
fn statement_write_blob_and_limit_errors_never_publish_or_mutate_source() {
    let (directory, profile) = fixture();
    let source = Path::new(profile.sqlite_path.as_deref().unwrap());
    let original = fs::read(source).unwrap();
    for sql in [
        "INSERT INTO original VALUES(8) RETURNING v",
        "SELECT 1; SELECT 2",
        "PRAGMA journal_mode=wal",
        "SELECT 1 UNION ALL SELECT x'00'",
        "SELECT 1 UNION ALL SELECT abs(-9223372036854775808)",
    ] {
        let output = directory.path().join("failed.csv");
        let error = export(request(&profile, output.clone(), sql)).unwrap_err();
        assert!(error.message.contains("not published"), "{error}");
        assert!(!output.exists());
        assert_eq!(fs::read(source).unwrap(), original);
        no_provisional_files(directory.path());
    }
    for options in [
        ExportOptions {
            max_rows: 1,
            ..ExportOptions::default()
        },
        ExportOptions {
            max_bytes: 5,
            ..ExportOptions::default()
        },
        ExportOptions {
            max_row_bytes: 1,
            ..ExportOptions::default()
        },
    ] {
        let mut req = request(
            &profile,
            directory.path().join("limit.csv"),
            "SELECT 10 AS value UNION ALL SELECT 20",
        );
        req.options = options;
        let error = export(req).unwrap_err();
        assert_eq!(error.kind, ExportErrorKind::Execution);
        assert!(!directory.path().join("limit.csv").exists());
        no_provisional_files(directory.path());
    }
}

#[test]
fn service_validates_all_numeric_bounds_without_trusting_cli() {
    let (directory, profile) = fixture();
    for options in [
        ExportOptions {
            max_rows: 0,
            ..ExportOptions::default()
        },
        ExportOptions {
            max_rows: MAX_ROWS + 1,
            ..ExportOptions::default()
        },
        ExportOptions {
            max_bytes: 0,
            ..ExportOptions::default()
        },
        ExportOptions {
            max_bytes: MAX_BYTES + 1,
            ..ExportOptions::default()
        },
        ExportOptions {
            max_row_bytes: 0,
            ..ExportOptions::default()
        },
        ExportOptions {
            max_row_bytes: MAX_ROW_BYTES + 1,
            ..ExportOptions::default()
        },
        ExportOptions {
            timeout: Duration::ZERO,
            ..ExportOptions::default()
        },
        ExportOptions {
            timeout: MAX_TIMEOUT + Duration::from_nanos(1),
            ..ExportOptions::default()
        },
        ExportOptions {
            null_marker: String::new(),
            ..ExportOptions::default()
        },
    ] {
        let mut req = request(&profile, directory.path().join("invalid.csv"), "SELECT 1");
        req.options = options;
        assert_eq!(
            export(req).unwrap_err().kind,
            ExportErrorKind::Configuration
        );
        assert!(!directory.path().join("invalid.csv").exists());
    }
    for sql in [" ".into(), "x".repeat(dbide_project::MAX_SQL_BYTES + 1)] {
        let req = request(&profile, directory.path().join("invalid.csv"), &sql);
        assert_eq!(
            export(req).unwrap_err().kind,
            ExportErrorKind::Configuration
        );
    }
    no_provisional_files(directory.path());
}

#[test]
fn changed_sql_target_and_invalid_sources_fail_before_execution() {
    let (directory, profile) = fixture();
    for options in [
        ExportOptions {
            expect_sql_sha256: Some(dbide_project::digest_sql("SELECT 2")),
            ..ExportOptions::default()
        },
        ExportOptions {
            expect_target_fingerprint: Some("changed".into()),
            ..ExportOptions::default()
        },
    ] {
        let mut req = request(&profile, directory.path().join("guard.csv"), "SELECT 1");
        req.options = options;
        let error = export(req).unwrap_err();
        assert_eq!(error.kind, ExportErrorKind::Configuration);
        assert!(error.message.contains("nothing submitted"));
        assert!(!directory.path().join("guard.csv").exists());
    }
    for path in [":memory:".to_owned(), "relative.db".to_owned()] {
        let mut profile = profile.clone();
        profile.sqlite_path = Some(path);
        assert_eq!(
            export(request(
                &profile,
                directory.path().join("guard.csv"),
                "SELECT 1"
            ))
            .unwrap_err()
            .kind,
            ExportErrorKind::Configuration
        );
    }
    let mut missing = profile.clone();
    missing.sqlite_path = Some(directory.path().join("missing.db").to_str().unwrap().into());
    missing.sqlite_create = true;
    assert_eq!(
        export(request(
            &missing,
            directory.path().join("guard.csv"),
            "SELECT 1"
        ))
        .unwrap_err()
        .kind,
        ExportErrorKind::Connection
    );
    assert!(!Path::new(missing.sqlite_path.as_deref().unwrap()).exists());
    let mut postgres = profile;
    postgres.engine = DatabaseEngine::PostgreSql;
    assert_eq!(
        export(request(
            &postgres,
            directory.path().join("guard.csv"),
            "SELECT 1"
        ))
        .unwrap_err()
        .kind,
        ExportErrorKind::Configuration
    );
    no_provisional_files(directory.path());
}

#[test]
fn source_sidecars_ascii_case_aliases_and_existing_outputs_are_preserved() {
    let (directory, profile) = fixture();
    let source = fs::canonicalize(profile.sqlite_path.as_deref().unwrap()).unwrap();
    for suffix in ["-wal", "-shm", "-journal"] {
        for uppercase in [false, true] {
            let name = format!(
                "{}{}",
                source.file_name().unwrap().to_str().unwrap(),
                suffix
            );
            let name = if uppercase {
                name.to_ascii_uppercase()
            } else {
                name
            };
            let output = source.parent().unwrap().join(name);
            let error = export(request(&profile, output.clone(), "SELECT 1")).unwrap_err();
            assert_eq!(error.kind, ExportErrorKind::Configuration);
            assert!(error.message.contains("sidecar"));
            assert!(!output.exists());
        }
    }
    let output = directory.path().join("existing.csv");
    fs::write(&output, "preserve").unwrap();
    assert_eq!(
        export(request(&profile, output.clone(), "SELECT 1"))
            .unwrap_err()
            .kind,
        ExportErrorKind::Configuration
    );
    assert_eq!(fs::read_to_string(output).unwrap(), "preserve");
    no_provisional_files(directory.path());
}

#[test]
fn native_deadline_interrupt_discards_output() {
    let (directory, profile) = fixture();
    let output = directory.path().join("timeout.csv");
    let mut req = request(
        &profile,
        output.clone(),
        "WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<100000000) SELECT sum(v) FROM n",
    );
    req.options.timeout = Duration::from_millis(2);
    let error = export(req).unwrap_err();
    assert_eq!(error.kind, ExportErrorKind::Execution);
    assert!(error.message.contains("deadline"));
    assert!(!output.exists());
    no_provisional_files(directory.path());
}

#[cfg(unix)]
#[test]
fn non_utf8_output_is_refused_without_running_sql_or_creating_temp_files() {
    use std::os::unix::ffi::OsStringExt;
    let (directory, profile) = fixture();
    let output = directory
        .path()
        .join(std::ffi::OsString::from_vec(b"invalid-\xff.csv".to_vec()));
    let error = export(request(
        &profile,
        output.clone(),
        "SELECT does_not_exist FROM missing",
    ))
    .unwrap_err();
    assert_eq!(error.kind, ExportErrorKind::Configuration);
    assert!(error.message.contains("output path must be UTF-8"));
    assert!(!output.exists());
    no_provisional_files(directory.path());
}
