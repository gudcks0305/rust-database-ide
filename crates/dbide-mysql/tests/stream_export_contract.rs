//! Opt-in project-owned loopback fixture; no user profiles or databases.
use dbide_driver_api::{
    DatabaseEngine,
    stream::{StreamErrorKind, StreamEvent, StreamLimits},
};
use dbide_mysql::export::stream_read_only;
use dbide_profiles::{ConnectionProfile, ConnectionRequest, TlsMode};
use mysql_async::{Conn, OptsBuilder, prelude::Queryable};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn request() -> ConnectionRequest {
    assert_eq!(
        std::env::var("DBIDE_MYSQL_FIXTURE").as_deref(),
        Ok("owned-loopback")
    );
    let inspection = std::process::Command::new("docker")
        .args([
            "inspect",
            "dbide-mysql",
            "--format",
            "{{index .Config.Labels \"owner\"}} {{json .NetworkSettings.Ports}}",
        ])
        .output()
        .expect("inspect project-owned fixture");
    let output = String::from_utf8(inspection.stdout).unwrap();
    assert!(
        inspection.status.success()
            && output.starts_with("rust-database-ide-mysql ")
            && output.contains("127.0.0.1")
            && output.contains("55441"),
        "fixture ownership/loopback verification failed"
    );
    ConnectionRequest {
        profile: ConnectionProfile {
            engine: DatabaseEngine::MySql,
            host: "127.0.0.1".into(),
            port: 55441,
            database: "dbide_fixture".into(),
            username: "root".into(),
            tls_mode: TlsMode::Disable,
            connect_timeout_secs: 3,
            // Source must enforce READ ONLY even when the profile doesn't.
            read_only: false,
            ..ConnectionProfile::default()
        },
        password: "dbide_fixture_only".into(),
    }
}
async fn observer() -> Conn {
    let r = request();
    Conn::new(
        OptsBuilder::default()
            .ip_or_hostname(r.profile.host)
            .tcp_port(r.profile.port)
            .user(Some(r.profile.username))
            .pass(Some(r.password))
            .db_name(Some(r.profile.database))
            .prefer_socket(false),
    )
    .await
    .unwrap()
}
fn flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn exports_past_ui_caps_and_preserves_last_row() {
    let sql = "WITH d AS (SELECT 0 n UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9), nums AS (SELECT a.n+10*b.n+100*c.n+1000*e.n AS n FROM d a CROSS JOIN d b CROSS JOIN d c CROSS JOIN d e) SELECT n, REPEAT('x',128) AS payload FROM nums WHERE n<5000 ORDER BY n";
    let mut last = String::new();
    let mut rows = 0;
    let summary = stream_read_only(request(), sql, StreamLimits::default(), flag(), |event| {
        if let StreamEvent::Row(values) = event {
            rows += 1;
            last.clone_from(values[0].as_ref().unwrap());
            assert_eq!(values[1].as_ref().unwrap().len(), 128);
        }
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(summary.rows, 5000);
    assert_eq!(rows, 5000);
    assert_eq!(last, "4999");
    assert!(summary.payload_bytes > 512 * 1024);
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn scalars_null_empty_and_readonly_transaction_are_native() {
    let mut rows = Vec::new();
    stream_read_only(request(), "SELECT CAST(18446744073709551615 AS UNSIGNED), CAST('12345678901234567890.123456789' AS DECIMAL(29,9)), NULL, '', CAST('2026-10-22' AS DATE), CAST('2026-10-22 01:02:03.456789' AS DATETIME(6)), CAST('-25:02:03.456789' AS TIME(6)), '한글'", StreamLimits::default(), flag(), |event| {
        if let StreamEvent::Row(values) = event { rows.push(values.to_vec()); } Ok(())
    }).await.unwrap();
    assert_eq!(
        rows,
        vec![vec![
            Some("18446744073709551615".into()),
            Some("12345678901234567890.123456789".into()),
            None,
            Some("".into()),
            Some("2026-10-22".into()),
            Some("2026-10-22 01:02:03.456789".into()),
            Some("-25:02:03.456789".into()),
            Some("한글".into())
        ]]
    );
    let mut mode = String::new();
    stream_read_only(request(), "SELECT ACCESS_MODE FROM performance_schema.events_transactions_current WHERE THREAD_ID=(SELECT THREAD_ID FROM performance_schema.threads WHERE PROCESSLIST_ID=CONNECTION_ID())", StreamLimits::default(), flag(), |event| { if let StreamEvent::Row(values) = event { mode.clone_from(values[0].as_ref().unwrap()); } Ok(()) }).await.unwrap();
    assert_eq!(mode, "READ ONLY");
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn empty_result_limits_binary_and_multi_statement_contracts() {
    let mut columns = 0;
    let summary = stream_read_only(
        request(),
        "SELECT 1 AS empty_result WHERE FALSE",
        StreamLimits::default(),
        flag(),
        |event| {
            if let StreamEvent::Columns(values) = event {
                columns = values.len();
            } else {
                panic!("empty result emitted row");
            }
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(columns, 1);
    assert_eq!(summary.rows, 0);
    for sql in [
        "SELECT 1; SELECT 2",
        "SELECT 1 INTO OUTFILE '/tmp/dbide-stream-refused'",
        "SELECT 1 INTO DUMPFILE '/tmp/dbide-stream-refused'",
        "SET TRANSACTION READ WRITE",
        "SELECT @x:=1",
        "SELECT 1 FOR UPDATE",
    ] {
        let error = stream_read_only(request(), sql, StreamLimits::default(), flag(), |_| {
            panic!("refused query reached sink")
        })
        .await
        .unwrap_err();
        assert_eq!(error.kind, StreamErrorKind::Configuration, "{sql}");
    }
    for sql in ["SELECT CAST('abc' AS BINARY)", "SELECT UNHEX('ff')"] {
        assert_eq!(
            stream_read_only(request(), sql, StreamLimits::default(), flag(), |_| Ok(()))
                .await
                .unwrap_err()
                .kind,
            StreamErrorKind::Execution
        );
    }
    let mut rows = 0;
    let error = stream_read_only(
        request(),
        "SELECT 1 UNION ALL SELECT 2",
        StreamLimits {
            max_rows: 1,
            ..StreamLimits::default()
        },
        flag(),
        |event| {
            if matches!(event, StreamEvent::Row(_)) {
                rows += 1;
            }
            Ok(())
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::Limit);
    assert_eq!(rows, 1);
    let error = stream_read_only(
        request(),
        "SELECT REPEAT('x',1024)",
        StreamLimits {
            max_row_bytes: 32,
            ..StreamLimits::default()
        },
        flag(),
        |_| Ok(()),
    )
    .await
    .unwrap_err();
    // Native packet bound may reject before application-level row check.
    assert!(matches!(
        error.kind,
        StreamErrorKind::Limit | StreamErrorKind::Execution
    ));
    let error = stream_read_only(
        request(),
        "SELECT 1 AS abc",
        StreamLimits {
            max_payload_bytes: 2,
            ..StreamLimits::default()
        },
        flag(),
        |_| panic!("over-budget header reached sink"),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::Limit);
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn sink_and_prepare_errors_retire_owned_connection() {
    let mut id = 0u32;
    let error = stream_read_only(
        request(),
        "SELECT CONNECTION_ID()",
        StreamLimits::default(),
        flag(),
        |event| {
            if let StreamEvent::Row(values) = event {
                id = values[0].as_ref().unwrap().parse().unwrap();
                return Err("fixture sink stopped".into());
            }
            Ok(())
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::Sink);
    let mut observer = observer().await;
    assert_eq!(
        observer
            .exec_first::<u64, _, _>(
                "SELECT COUNT(*) FROM information_schema.processlist WHERE ID=?",
                (id,)
            )
            .await
            .unwrap(),
        Some(0)
    );
    let error = stream_read_only(
        request(),
        "SELECT missing_column FROM missing_dbide_export_table",
        StreamLimits::default(),
        flag(),
        |_| Ok(()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::Execution);
    observer.disconnect().await.unwrap();
}

async fn running_id(observer: &mut Conn, marker: &str) -> u32 {
    for _ in 0..100 {
        let id: Option<u32> = observer.exec_first("SELECT ID FROM information_schema.processlist WHERE INFO LIKE ? AND ID<>CONNECTION_ID()",(format!("%{marker}%"),)).await.unwrap();
        if let Some(id) = id {
            return id;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("export query did not start");
}
#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn cancellation_and_deadline_kill_long_query_and_backend() {
    for timeout in [false, true] {
        let mut observer = observer().await;
        let cancel = flag();
        let worker_cancel = cancel.clone();
        let r = request();
        let marker = if timeout {
            "dbide_export_timeout_contract"
        } else {
            "dbide_export_cancel_contract"
        };
        let sql = format!("SELECT SLEEP(30) /* {marker} */");
        let started = std::time::Instant::now();
        let task = tokio::spawn(async move {
            stream_read_only(
                r,
                &sql,
                StreamLimits {
                    timeout: if timeout {
                        Duration::from_millis(600)
                    } else {
                        Duration::from_secs(30)
                    },
                    ..StreamLimits::default()
                },
                worker_cancel,
                |_| Ok(()),
            )
            .await
        });
        let id = running_id(&mut observer, marker).await;
        if !timeout {
            cancel.store(true, Ordering::Release);
        }
        let error = tokio::time::timeout(Duration::from_secs(7), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error.kind,
            if timeout {
                StreamErrorKind::Timeout
            } else {
                StreamErrorKind::Cancelled
            }
        );
        assert!(started.elapsed() < Duration::from_secs(7));
        assert_eq!(
            observer
                .exec_first::<u64, _, _>(
                    "SELECT COUNT(*) FROM information_schema.processlist WHERE ID=?",
                    (id,)
                )
                .await
                .unwrap(),
            Some(0)
        );
        observer.disconnect().await.unwrap();
    }
}
