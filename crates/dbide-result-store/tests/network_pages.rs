//! Full native network source -> private disk spool -> page reads.
//! Explicit opt-in and exact Docker ownership/loopback checks precede connections.
use dbide_export::{ExportOptions, QueryRequest};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine, TlsMode};
use dbide_result_store::{PageLimits, query_pages};
use std::{process::Command, sync::Arc, time::Instant};

fn owned_request(engine: DatabaseEngine) -> ConnectionRequest {
    assert_eq!(
        std::env::var("DBIDE_STREAM_OWNED_LIVE").as_deref(),
        Ok("1"),
        "set DBIDE_STREAM_OWNED_LIVE=1 to opt into the project-owned fixtures"
    );
    let (container, owner, native_port, host_port, username) = match engine {
        DatabaseEngine::PostgreSql => (
            "dbide-m0-pg",
            "rust-database-ide-m0",
            "5432/tcp",
            55439,
            "postgres",
        ),
        DatabaseEngine::MySql => (
            "dbide-mysql",
            "rust-database-ide-mysql",
            "3306/tcp",
            55441,
            "root",
        ),
        _ => panic!("network fixture engine required"),
    };
    let inspected = Command::new("docker")
        .args(["inspect", "--format", "{{json .}}", container])
        .output()
        .expect("inspect already-running project-owned Docker fixture");
    assert!(inspected.status.success(), "owned fixture inspect failed");
    let inspection: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(inspection["Config"]["Labels"]["owner"], owner);
    assert_eq!(inspection["State"]["Running"], true);
    let bindings = inspection["NetworkSettings"]["Ports"][native_port]
        .as_array()
        .unwrap();
    assert_eq!(bindings.len(), 1, "fixture must have one loopback binding");
    assert_eq!(bindings[0]["HostIp"], "127.0.0.1");
    assert_eq!(bindings[0]["HostPort"], host_port.to_string());
    ConnectionRequest {
        profile: ConnectionProfile {
            id: format!("owned-result-store-{container}"),
            name: "Project-owned streaming page fixture".into(),
            engine,
            host: "127.0.0.1".into(),
            port: host_port,
            database: "dbide_fixture".into(),
            username: username.into(),
            tls_mode: TlsMode::Disable,
            connect_timeout_secs: 3,
            read_only: false,
            ..ConnectionProfile::default()
        },
        password: "dbide_fixture_only".into(),
    }
}

fn assert_complete_capture(engine: DatabaseEngine, sql: &str, first_id: u64, expected_rows: u64) {
    let started = Instant::now();
    let store = query_pages(
        QueryRequest {
            connection: owned_request(engine),
            sql: sql.into(),
            options: ExportOptions::default(),
        },
        PageLimits::default(),
        Arc::default(),
        Arc::default(),
    )
    .expect("one complete native query must produce a finished disk store");
    assert_eq!(store.summary.rows, expected_rows);
    assert!(store.summary.payload_bytes > 512 * 1024);
    assert!(store.encoded_bytes() > 512 * 1024);
    assert_eq!(store.page_count(), expected_rows.div_ceil(200) as usize);
    assert_eq!(
        store.columns(),
        [
            "id",
            "payload",
            "source_connection",
            "absent",
            "empty_value"
        ]
    );
    let first_page = store.read_page(0).unwrap();
    let connection_id = first_page.rows[0][2]
        .clone()
        .expect("native connection identifier");
    assert!(connection_id.parse::<u64>().unwrap() > 0);
    let mut next_id = first_id;
    let mut count = 0;
    for page_index in 0..store.page_count() {
        let page = store.read_page(page_index).unwrap();
        assert_eq!(page.row_start, count);
        assert_eq!(page.columns, store.columns());
        assert!(!page.rows.is_empty() && page.rows.len() <= 200);
        for row in page.rows {
            assert_eq!(row.len(), 5);
            assert_eq!(row[0].as_deref().unwrap().parse::<u64>().unwrap(), next_id);
            assert_eq!(row[1].as_deref(), Some("x".repeat(128).as_str()));
            assert_eq!(row[2].as_deref(), Some(connection_id.as_str()));
            assert_eq!(row[3], None);
            assert_eq!(row[4].as_deref(), Some(""));
            next_id += 1;
            count += 1;
        }
    }
    assert_eq!(count, expected_rows);
    assert_eq!(next_id - 1, first_id + expected_rows - 1);
    assert_eq!(store.read_page(0).unwrap(), first_page);
    assert!(store.read_page(store.page_count()).is_err());
    eprintln!(
        "{engine:?}: rows={count}, pages={}, encoded_bytes={}, one execution_id={}, capture+walk={:?}",
        store.page_count(),
        store.encoded_bytes(),
        store.summary.execution_id,
        started.elapsed()
    );
}

#[test]
#[ignore = "requires DBIDE_STREAM_OWNED_LIVE=1 and owned PostgreSQL loopback fixture on 55439"]
fn postgres_source_to_spool_to_all_pages() {
    assert_complete_capture(
        DatabaseEngine::PostgreSql,
        "SELECT i::text AS id, repeat('x',128) AS payload, pg_backend_pid()::text AS source_connection,
         NULL::text AS absent, ''::text AS empty_value FROM generate_series(1,5001) i ORDER BY i",
        1,
        5001,
    );
}

#[test]
#[ignore = "requires DBIDE_STREAM_OWNED_LIVE=1 and owned MySQL loopback fixture on 55441"]
fn mysql_source_to_spool_to_all_pages() {
    assert_complete_capture(
        DatabaseEngine::MySql,
        "WITH d AS (SELECT 0 n UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4
         UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9),
         nums AS (SELECT a.n+10*b.n+100*c.n+1000*e.n AS n FROM d a CROSS JOIN d b CROSS JOIN d c CROSS JOIN d e)
         SELECT n AS id, REPEAT('x',128) AS payload, CONNECTION_ID() AS source_connection,
         NULL AS absent, '' AS empty_value FROM nums WHERE n<5000 ORDER BY n",
        0,
        5000,
    );
}
