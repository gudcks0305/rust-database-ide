//! Only this standalone source contract is run for streaming export.
//! Live tests require the project-owned loopback Docker fixture, never user profiles.
use dbide_driver_api::stream::{StreamErrorKind, StreamEvent, StreamLimits};
use dbide_pg_spike::export::stream_read_only;
use dbide_profiles::{ConnectionProfile, ConnectionRequest};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn request() -> ConnectionRequest {
    ConnectionRequest {
        profile: ConnectionProfile::fixture(),
        password: "dbide_fixture_only".into(),
    }
}
fn running() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

#[tokio::test]
async fn local_validation_uses_the_operation_deadline() {
    let mut unreachable = request();
    unreachable.profile.port = 1;
    // The valid local parse necessarily exceeds one nanosecond. A deadline
    // created after validation would attempt the unreachable connection instead.
    let result = stream_read_only(
        unreachable,
        "WITH x AS (SELECT 1 AS i) SELECT i FROM x",
        StreamLimits {
            timeout: Duration::from_nanos(1),
            ..StreamLimits::default()
        },
        running(),
        |_| panic!("expired operation reached sink"),
    )
    .await;
    assert_eq!(result.unwrap_err().kind, StreamErrorKind::Timeout);
}

#[tokio::test]
async fn invalid_configuration_and_sql_fail_before_connect_or_sink() {
    let mut unreachable = request();
    unreachable.profile.port = 1;
    for sql in [
        "",
        "SELECT 1; SELECT 2",
        "SELECT 1 INTO export_must_not_exist",
        "BEGIN",
        "EXPLAIN SELECT 1",
        "SELECT $1",
        "SELECT * FROM pg_class FOR UPDATE",
        "WITH x AS (DELETE FROM export_must_not_exist RETURNING *) SELECT * FROM x",
        "WITH x AS (SELECT 1 INTO export_must_not_exist) SELECT * FROM x",
        "SELECT * FROM (SELECT * FROM pg_class FOR SHARE) q",
    ] {
        let result = stream_read_only(
            unreachable.clone(),
            sql,
            StreamLimits::default(),
            running(),
            |_| panic!("invalid SQL dispatched to sink"),
        )
        .await;
        assert_eq!(
            result.unwrap_err().kind,
            StreamErrorKind::Configuration,
            "{sql}"
        );
    }
    for limits in [
        StreamLimits {
            max_rows: 0,
            ..StreamLimits::default()
        },
        StreamLimits {
            max_row_bytes: 0,
            ..StreamLimits::default()
        },
        StreamLimits {
            max_payload_bytes: 0,
            ..StreamLimits::default()
        },
        StreamLimits {
            timeout: Duration::ZERO,
            ..StreamLimits::default()
        },
        StreamLimits {
            timeout: Duration::MAX,
            ..StreamLimits::default()
        },
        StreamLimits {
            max_rows: u64::MAX,
            ..StreamLimits::default()
        },
        StreamLimits {
            max_row_bytes: usize::MAX,
            ..StreamLimits::default()
        },
        StreamLimits {
            max_payload_bytes: u64::MAX,
            ..StreamLimits::default()
        },
    ] {
        let result = stream_read_only(unreachable.clone(), "SELECT 1", limits, running(), |_| {
            panic!("invalid limits dispatched")
        })
        .await;
        assert_eq!(result.unwrap_err().kind, StreamErrorKind::Configuration);
    }
    let oversized = format!("SELECT '{}'", "x".repeat(64 * 1024));
    assert_eq!(
        stream_read_only(
            unreachable.clone(),
            &oversized,
            StreamLimits::default(),
            running(),
            |_| Ok(())
        )
        .await
        .unwrap_err()
        .kind,
        StreamErrorKind::Configuration
    );
    let mut invalid_profile = unreachable.clone();
    invalid_profile.profile.host = "postgresql://secret:never-print-me@localhost/db".into();
    let diagnostic = stream_read_only(
        invalid_profile,
        "SELECT 1",
        StreamLimits::default(),
        running(),
        |_| Ok(()),
    )
    .await
    .unwrap_err();
    assert_eq!(diagnostic.kind, StreamErrorKind::Configuration);
    assert!(!format!("{diagnostic:?}").contains("never-print-me"));
    assert_eq!(
        stream_read_only(
            unreachable,
            "SELECT 1",
            StreamLimits::default(),
            Arc::new(AtomicBool::new(true)),
            |_| panic!("pre-cancel dispatched")
        )
        .await
        .unwrap_err()
        .kind,
        StreamErrorKind::Cancelled
    );
}

fn owned_fixture() {
    let inspected = std::process::Command::new("docker")
        .args(["inspect", "--format", "{{json .}}", "dbide-m0-pg"])
        .output()
        .expect("Docker inspect owned fixture");
    assert!(
        inspected.status.success(),
        "owned fixture must already be running"
    );
    let value: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(value["Config"]["Labels"]["owner"], "rust-database-ide-m0");
    assert_eq!(value["State"]["Running"], true);
    let ports = value["NetworkSettings"]["Ports"]["5432/tcp"]
        .as_array()
        .unwrap();
    assert_eq!(ports.len(), 1);
    assert_eq!(ports[0]["HostIp"], "127.0.0.1");
    assert_eq!(ports[0]["HostPort"], "55439");
}
async fn no_export_backend(observer: &tokio_postgres::Client) {
    for _ in 0..100 {
        let count: i64 = observer.query_one("SELECT count(*) FROM pg_stat_activity WHERE application_name = 'dbide-readonly-stream'", &[]).await.unwrap().get(0);
        if count == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("owned export backend remained alive after terminal result");
}
async fn active_pid(observer: &tokio_postgres::Client, marker: &str) -> i32 {
    for _ in 0..200 {
        let rows = observer.query("SELECT pid FROM pg_stat_activity WHERE application_name = 'dbide-readonly-stream' AND state = 'active' AND query LIKE $1", &[&format!("%{marker}%")]).await.unwrap();
        if let Some(row) = rows.first() {
            return row.get(0);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("expected owned export query did not become active");
}

#[tokio::test]
#[ignore = "requires already-running project-owned loopback PostgreSQL fixture; explicit opt-in"]
async fn standalone_postgres_streaming_contract() {
    owned_fixture();
    let (observer, connection) = tokio_postgres::connect(
        "postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture",
        tokio_postgres::NoTls,
    )
    .await
    .unwrap();
    let observer_driver = tokio::spawn(async move {
        connection.await.unwrap();
    });
    let version: String = observer
        .query_one("SHOW server_version", &[])
        .await
        .unwrap()
        .get(0);
    assert!(
        version.starts_with("18.6"),
        "recorded fixture contract uses PostgreSQL 18.6, found {version}"
    );

    let mut rows = 0;
    let mut last = String::new();
    let summary = stream_read_only(request(), "SELECT i::text AS id, repeat('한글', 128) AS body, NULL::text AS absent, ''::text AS empty FROM generate_series(1, 5001) i ORDER BY i", StreamLimits::default(), running(), |event| {
        match event {
            StreamEvent::Columns(names) => assert_eq!(names, ["id", "body", "absent", "empty"]),
            StreamEvent::Row(row) => {
                rows += 1;
                last = row[0].clone().unwrap();
                assert_eq!(row[1].as_ref().unwrap().len(), 768);
                assert_eq!(row[2], None);
                assert_eq!(row[3].as_deref(), Some(""));
            }
        }
        Ok(())
    }).await.unwrap();
    assert_eq!(rows, 5001);
    assert_eq!(last, "5001");
    assert_eq!(summary.rows, 5001);
    assert!(summary.payload_bytes > 512 * 1024);
    no_export_backend(&observer).await;

    let mut header = 0;
    let summary = stream_read_only(
        request(),
        "SELECT 1 AS empty_result WHERE false",
        StreamLimits::default(),
        running(),
        |event| {
            match event {
                StreamEvent::Columns(cols) => {
                    header += 1;
                    assert_eq!(cols, ["empty_result"]);
                }
                StreamEvent::Row(_) => panic!("zero rows"),
            }
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(header, 1);
    assert_eq!(summary.rows, 0);
    assert_eq!(summary.payload_bytes, "empty_result".len() as u64);
    no_export_backend(&observer).await;

    let summary = stream_read_only(
        request(),
        "SELECT NULL::text AS n, ''::text AS e",
        StreamLimits {
            max_payload_bytes: 4,
            ..StreamLimits::default()
        },
        running(),
        |event| {
            if let StreamEvent::Row(row) = event {
                assert_eq!(row, [None, Some(String::new())]);
            }
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(summary.rows, 1);
    assert_eq!(
        summary.payload_bytes, 4,
        "two header bytes plus one byte per NULL/empty value"
    );
    no_export_backend(&observer).await;
    let mut rows_dispatched = 0;
    let error = stream_read_only(
        request(),
        "SELECT NULL::text AS n, ''::text AS e",
        StreamLimits {
            max_payload_bytes: 3,
            ..StreamLimits::default()
        },
        running(),
        |event| {
            if matches!(event, StreamEvent::Row(_)) {
                rows_dispatched += 1;
            }
            Ok(())
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::Limit);
    assert_eq!(
        rows_dispatched, 0,
        "NULL/empty row rejected before sink dispatch"
    );
    no_export_backend(&observer).await;

    for (sql, limits, kind) in [
        (
            "SELECT i FROM generate_series(1, 3) i",
            StreamLimits {
                max_rows: 2,
                ..StreamLimits::default()
            },
            StreamErrorKind::Limit,
        ),
        (
            "SELECT repeat('x', 4096)",
            StreamLimits {
                max_row_bytes: 1024,
                ..StreamLimits::default()
            },
            StreamErrorKind::Limit,
        ),
        (
            "SELECT repeat('x', 100) FROM generate_series(1, 10)",
            StreamLimits {
                max_payload_bytes: 300,
                ..StreamLimits::default()
            },
            StreamErrorKind::Limit,
        ),
        (
            "SELECT 1 AS oversized_header",
            StreamLimits {
                max_row_bytes: 1,
                ..StreamLimits::default()
            },
            StreamErrorKind::Limit,
        ),
        (
            "SELECT decode('cafe', 'hex')",
            StreamLimits::default(),
            StreamErrorKind::Configuration,
        ),
        (
            "SELECT ARRAY[decode('cafe', 'hex')]",
            StreamLimits::default(),
            StreamErrorKind::Configuration,
        ),
        (
            "SELECT 'NaN'::numeric",
            StreamLimits::default(),
            StreamErrorKind::Execution,
        ),
        (
            "SELECT 'Infinity'::float8",
            StreamLimits::default(),
            StreamErrorKind::Execution,
        ),
        (
            "SELECT 1 / (2 - i) FROM generate_series(1, 3) i",
            StreamLimits::default(),
            StreamErrorKind::Execution,
        ),
        (
            "SELECT dbide_export_column_does_not_exist",
            StreamLimits::default(),
            StreamErrorKind::Execution,
        ),
    ] {
        let result = stream_read_only(request(), sql, limits, running(), |_| Ok(())).await;
        assert_eq!(result.unwrap_err().kind, kind, "{sql}");
        no_export_backend(&observer).await;
    }
    let text = stream_read_only(
        request(),
        "SELECT decode('cafe', 'hex')::text, 'NaN'::numeric::text, 'Infinity'::float8::text",
        StreamLimits::default(),
        running(),
        |event| {
            if let StreamEvent::Row(row) = event {
                assert_eq!(row[0].as_deref(), Some("\\xcafe"));
                assert_eq!(row[1].as_deref(), Some("NaN"));
                assert_eq!(row[2].as_deref(), Some("Infinity"));
            }
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(text.rows, 1);
    no_export_backend(&observer).await;

    let summary = stream_read_only(
        request(),
        "WITH mode AS (SELECT current_setting('transaction_read_only') AS value) SELECT value FROM mode",
        StreamLimits::default(),
        running(),
        |event| {
            if let StreamEvent::Row(row) = event {
                assert_eq!(row[0].as_deref(), Some("on"));
            }
            Ok(())
        },
    ).await.unwrap();
    assert_eq!(summary.rows, 1);
    no_export_backend(&observer).await;

    let flag = running();
    let task_flag = flag.clone();
    let task = tokio::spawn(async move {
        stream_read_only(
            request(),
            "SELECT pg_sleep(30) /* dbide_stream_cancel_probe */",
            StreamLimits::default(),
            task_flag,
            |_| Ok(()),
        )
        .await
    });
    let pid = active_pid(&observer, "dbide_stream_cancel_probe").await;
    flag.store(true, Ordering::Release);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .kind,
        StreamErrorKind::Cancelled
    );
    no_export_backend(&observer).await;
    assert_eq!(
        observer
            .query_one(
                "SELECT count(*) FROM pg_stat_activity WHERE pid = $1",
                &[&pid]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );

    let result = stream_read_only(
        request(),
        "SELECT pg_sleep(30)",
        StreamLimits {
            timeout: Duration::from_millis(200),
            ..StreamLimits::default()
        },
        running(),
        |_| Ok(()),
    )
    .await;
    assert_eq!(result.unwrap_err().kind, StreamErrorKind::Timeout);
    no_export_backend(&observer).await;

    let mut dispatched = false;
    let result = stream_read_only(
        request(),
        "SELECT i FROM generate_series(1, 100000) i",
        StreamLimits::default(),
        running(),
        |event| {
            if matches!(event, StreamEvent::Row(_)) {
                dispatched = true;
                return Err("sink-secret-must-not-leak".into());
            }
            Ok(())
        },
    )
    .await;
    assert!(dispatched);
    let error = result.unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::Sink);
    assert!(!format!("{error:?}").contains("sink-secret"));
    no_export_backend(&observer).await;
    drop(observer);
    observer_driver.await.unwrap();
}
