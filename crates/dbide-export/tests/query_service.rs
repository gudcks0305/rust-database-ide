use dbide_driver_api::stream::StreamEvent;
use dbide_export::{
    ExportErrorKind, ExportOptions, ExportProgress, ExportRequest, QueryRequest, export_csv,
    stream_query,
};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine, StoredExecutionTarget};
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn fixture() -> (tempfile::TempDir, ConnectionProfile) {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("query-owned.db");
    rusqlite::Connection::open(&source)
        .unwrap()
        .execute_batch("CREATE TABLE original(v); INSERT INTO original VALUES(7)")
        .unwrap();
    let profile = ConnectionProfile {
        id: "owned-query-fixture".into(),
        name: "Owned query fixture".into(),
        engine: DatabaseEngine::Sqlite,
        sqlite_path: Some(source.to_str().unwrap().into()),
        ..ConnectionProfile::default()
    };
    (directory, profile)
}

fn request(profile: &ConnectionProfile, sql: &str) -> QueryRequest {
    QueryRequest {
        connection: ConnectionRequest {
            profile: profile.clone(),
            password: "fixture-secret-must-not-appear".into(),
        },
        sql: sql.into(),
        options: ExportOptions::default(),
    }
}

#[test]
fn generated_stream_preserves_null_text_and_matches_csv_identity_and_native_totals() {
    let (directory, profile) = fixture();
    let sql = "WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<6000) SELECT v AS id, NULL AS n, '' AS e, '한글🫧' AS ko, v*1.5 AS real FROM n";
    let progress = Arc::new(ExportProgress::default());
    let mut rows = 0;
    let mut headers = 0;
    let summary = stream_query(
        request(&profile, sql),
        Arc::new(AtomicBool::new(false)),
        progress.clone(),
        |event| {
            match event {
                StreamEvent::Columns(names) => {
                    headers += 1;
                    assert_eq!(names, &["id", "n", "e", "ko", "real"]);
                }
                StreamEvent::Row(values) => {
                    rows += 1;
                    assert_eq!(values[0].as_deref(), Some(rows.to_string().as_str()));
                    assert_eq!(values[1], None);
                    assert_eq!(values[2].as_deref(), Some(""));
                    assert_eq!(values[3].as_deref(), Some("한글🫧"));
                    assert_eq!(
                        values[4].as_deref(),
                        Some((rows as f64 * 1.5).to_string().as_str())
                    );
                }
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(headers, 1);
    assert_eq!(rows, 6000);
    assert_eq!(summary.rows, rows);
    // SQLite header=10, each native row=8+1+1+10+8 bytes.
    assert_eq!(summary.payload_bytes, 10 + rows * 28);
    assert_eq!(progress.snapshot().rows, rows);
    assert_eq!(progress.snapshot().payload_bytes, summary.payload_bytes);
    assert_eq!(
        summary.execution_target,
        StoredExecutionTarget::capture(&profile, "main").unwrap()
    );
    assert_eq!(summary.sql_sha256, dbide_project::digest_sql(sql));
    assert_ne!(summary.execution_id, 0);
    let output = directory.path().join("stream-parity.csv");
    let csv = export_csv(
        ExportRequest {
            profile: profile.clone(),
            sql: sql.into(),
            output: output.clone(),
            options: ExportOptions::default(),
        },
        Arc::new(AtomicBool::new(false)),
        Arc::new(ExportProgress::default()),
    )
    .unwrap();
    assert_eq!(csv.rows, summary.rows);
    assert_eq!(csv.payload_bytes, summary.payload_bytes);
    assert_eq!(csv.execution_target, summary.execution_target);
    assert_eq!(csv.sql_sha256, summary.sql_sha256);
    let text = fs::read_to_string(output).unwrap();
    assert_eq!(text.lines().count(), rows as usize + 1);
    assert!(text.ends_with("6000,\\N,,한글🫧,9000\n"));
}

#[test]
fn query_sink_failures_and_cancellation_never_claim_file_publication() {
    let (_directory, profile) = fixture();
    let sql = "SELECT 1 AS v UNION ALL SELECT 2 UNION ALL SELECT 3";
    let error = stream_query(
        request(&profile, sql),
        Arc::new(AtomicBool::new(false)),
        Arc::new(ExportProgress::default()),
        |event| {
            if matches!(event, StreamEvent::Row(_)) {
                Err("owned sink failed".into())
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, ExportErrorKind::Execution);
    assert!(error.message.contains("owned sink failed"));
    assert!(!error.message.contains("published"));
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(ExportProgress::default());
    let error = stream_query(
        request(&profile, sql),
        cancel.clone(),
        progress.clone(),
        |event| {
            if matches!(event, StreamEvent::Row(_)) {
                cancel.store(true, Ordering::Release);
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, ExportErrorKind::Cancelled);
    assert_eq!(progress.snapshot().rows, 1);
    assert!(!error.message.contains("published"));
    // A failed worker cannot leave a console-owned connection or SQL cursor alive.
    assert_eq!(
        stream_query(
            request(&profile, "SELECT v FROM original"),
            Arc::new(AtomicBool::new(false)),
            Arc::new(ExportProgress::default()),
            |_| Ok(())
        )
        .unwrap()
        .rows,
        1
    );
}

#[test]
fn query_deadline_includes_synchronous_sink_and_does_not_reset_between_events() {
    let (_directory, profile) = fixture();
    let mut query = request(&profile, "SELECT 1 UNION ALL SELECT 2");
    query.options.timeout = Duration::from_millis(10);
    let error = stream_query(
        query,
        Arc::new(AtomicBool::new(false)),
        Arc::new(ExportProgress::default()),
        |_| {
            std::thread::sleep(Duration::from_millis(20));
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, ExportErrorKind::Execution);
    assert!(error.message.contains("deadline"));
}

#[test]
fn network_preflight_validates_hash_target_and_options_before_connecting() {
    for engine in [DatabaseEngine::PostgreSql, DatabaseEngine::MySql] {
        // Reserved .invalid host must never be resolved: local preflight rejects.
        let profile = ConnectionProfile {
            engine,
            host: "owned-fixture.invalid".into(),
            ..ConnectionProfile::default()
        };
        let mut invalid = request(&profile, "SELECT 1");
        invalid.options.expect_sql_sha256 = Some("changed".into());
        let error = stream_query(
            invalid,
            Arc::new(AtomicBool::new(false)),
            Arc::new(ExportProgress::default()),
            |_| panic!("no sink before connect"),
        )
        .unwrap_err();
        assert_eq!(error.kind, ExportErrorKind::Configuration);
        assert!(error.message.contains("nothing submitted"));
        assert!(!error.message.contains("fixture-secret"));
        let mut invalid = request(&profile, "SELECT 1");
        invalid.options.expect_target_fingerprint = Some("changed".into());
        assert_eq!(
            stream_query(
                invalid,
                Arc::new(AtomicBool::new(false)),
                Arc::new(ExportProgress::default()),
                |_| Ok(())
            )
            .unwrap_err()
            .kind,
            ExportErrorKind::Configuration
        );
        let mut invalid = request(&profile, "SELECT 1");
        invalid.options.timeout = Duration::ZERO;
        assert_eq!(
            stream_query(
                invalid,
                Arc::new(AtomicBool::new(false)),
                Arc::new(ExportProgress::default()),
                |_| Ok(())
            )
            .unwrap_err()
            .kind,
            ExportErrorKind::Configuration
        );
    }
}

#[test]
fn independent_query_ids_and_empty_result_terminal_progress() {
    let (_directory, profile) = fixture();
    let progress = Arc::new(ExportProgress::default());
    let mut ids = Vec::new();
    for _ in 0..2 {
        let summary = stream_query(
            request(&profile, "SELECT 1 AS empty WHERE 0"),
            Arc::new(AtomicBool::new(false)),
            progress.clone(),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(summary.rows, 0);
        assert_eq!(summary.payload_bytes, 5);
        assert_eq!(progress.snapshot().rows, 0);
        assert_eq!(progress.snapshot().payload_bytes, 5);
        ids.push(summary.execution_id);
    }
    assert_ne!(ids[0], ids[1]);
}
