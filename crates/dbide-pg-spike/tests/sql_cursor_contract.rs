//! Only generated data in the labelled, loopback project fixture is used.
use dbide_driver_api::stream::{StreamErrorKind, StreamEvent, StreamLimits, StreamSummary};
use dbide_pg_spike::cursor::{CursorEvent, CursorOptions, query_cursor};
use dbide_profiles::{ConnectionProfile, ConnectionRequest};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
static LIVE_FIXTURE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
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
async fn cursor_validates_before_connection_and_cancellation() {
    let mut request = request();
    request.profile.port = 1;
    for sql in [
        "",
        "SELECT 1; SELECT 2",
        "SELECT $1",
        "SELECT 1 INTO forbidden",
        "SELECT * FROM pg_class FOR UPDATE",
        "WITH x AS (DELETE FROM forbidden RETURNING *) SELECT * FROM x",
    ] {
        let (_send, recv) = mpsc::sync_channel(1);
        assert_eq!(
            query_cursor(
                request.clone(),
                sql,
                StreamLimits::default(),
                CursorOptions::default(),
                running(),
                recv,
                |_| panic!("invalid dispatched")
            )
            .await
            .unwrap_err()
            .kind,
            StreamErrorKind::Configuration
        );
    }
    for options in [
        CursorOptions {
            fetch_rows: 0,
            ..CursorOptions::default()
        },
        CursorOptions {
            fetch_rows: 201,
            ..CursorOptions::default()
        },
        CursorOptions {
            idle_timeout: Duration::ZERO,
            ..CursorOptions::default()
        },
        CursorOptions {
            idle_timeout: Duration::MAX,
            ..CursorOptions::default()
        },
        CursorOptions {
            lifetime_timeout: Duration::ZERO,
            ..CursorOptions::default()
        },
        CursorOptions {
            lifetime_timeout: Duration::MAX,
            ..CursorOptions::default()
        },
    ] {
        let (_send, recv) = mpsc::sync_channel(1);
        assert_eq!(
            query_cursor(
                request.clone(),
                "SELECT 1",
                StreamLimits::default(),
                options,
                running(),
                recv,
                |_| Ok(())
            )
            .await
            .unwrap_err()
            .kind,
            StreamErrorKind::Configuration
        );
    }
    for limits in [
        StreamLimits {
            max_rows: 0,
            ..StreamLimits::default()
        },
        StreamLimits {
            timeout: Duration::from_secs(31),
            ..StreamLimits::default()
        },
    ] {
        let (_send, recv) = mpsc::sync_channel(1);
        assert_eq!(
            query_cursor(
                request.clone(),
                "SELECT 1",
                limits,
                CursorOptions::default(),
                running(),
                recv,
                |_| Ok(())
            )
            .await
            .unwrap_err()
            .kind,
            StreamErrorKind::Configuration
        );
    }
    let (_send, recv) = mpsc::sync_channel(1);
    assert_eq!(
        query_cursor(
            request.clone(),
            "SELECT 1",
            StreamLimits::default(),
            CursorOptions::default(),
            Arc::new(AtomicBool::new(true)),
            recv,
            |_| panic!("cancelled dispatched")
        )
        .await
        .unwrap_err()
        .kind,
        StreamErrorKind::Cancelled
    );
    let (_send, recv) = mpsc::sync_channel(1);
    assert_eq!(
        query_cursor(
            request,
            "SELECT 1",
            StreamLimits {
                timeout: Duration::from_nanos(1),
                ..StreamLimits::default()
            },
            CursorOptions::default(),
            running(),
            recv,
            |_| panic!("expired dispatched")
        )
        .await
        .unwrap_err()
        .kind,
        StreamErrorKind::Timeout
    );
}

#[derive(Debug)]
enum Event {
    Columns(Vec<String>),
    Row(Vec<Option<String>>),
    Batch(bool, u64),
}
type Task = tokio::task::JoinHandle<Result<StreamSummary, dbide_driver_api::stream::StreamError>>;
fn start(
    sql: &str,
    options: CursorOptions,
    limits: StreamLimits,
    flag: Arc<AtomicBool>,
) -> (
    mpsc::SyncSender<u64>,
    tokio::sync::mpsc::UnboundedReceiver<Event>,
    Task,
) {
    let (send, recv) = mpsc::sync_channel(1);
    let (events, rows) = tokio::sync::mpsc::unbounded_channel();
    let sql = sql.to_owned();
    let task = tokio::spawn(async move {
        query_cursor(request(), &sql, limits, options, flag, recv, |event| {
            events
                .send(match event {
                    CursorEvent::Data(StreamEvent::Columns(cols)) => Event::Columns(cols.to_vec()),
                    CursorEvent::Data(StreamEvent::Row(row)) => Event::Row(row.to_vec()),
                    CursorEvent::BatchEnd {
                        eof,
                        fetch_sequence,
                    } => Event::Batch(eof, fetch_sequence),
                })
                .map_err(|_| "test sink disconnected".into())
        })
        .await
    });
    (send, rows, task)
}
async fn batch(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    columns: &mut usize,
) -> (Vec<Vec<Option<String>>>, bool, u64) {
    let mut rows = vec![];
    loop {
        match tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .expect("cursor ended before batch")
        {
            Event::Columns(names) => {
                *columns += 1;
                assert!(!names.is_empty());
            }
            Event::Row(row) => rows.push(row),
            Event::Batch(eof, sequence) => return (rows, eof, sequence),
        }
    }
}
fn owned_fixture() {
    let inspected = std::process::Command::new("docker")
        .args(["inspect", "--format", "{{json .}}", "dbide-m0-pg"])
        .output()
        .expect("inspect owned fixture");
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
async fn no_backend(observer: &tokio_postgres::Client) {
    for _ in 0..150 {
        let count: i64 = observer.query_one("SELECT count(*) FROM pg_stat_activity WHERE application_name = 'dbide-readonly-cursor'", &[]).await.unwrap().get(0);
        if count == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("owned cursor backend remains");
}

#[tokio::test]
#[ignore = "requires already-running project-owned loopback PostgreSQL fixture; explicit opt-in"]
async fn sql_cursor_owned_fixture_contract() {
    let _fixture = LIVE_FIXTURE.lock().await;
    owned_fixture();
    let (observer, driver) = tokio_postgres::connect(
        "postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture",
        tokio_postgres::NoTls,
    )
    .await
    .unwrap();
    let driver = tokio::spawn(async move {
        driver.await.unwrap();
    });
    let defaults = CursorOptions::default();
    let limits = StreamLimits::default();
    // A materialized volatile expression and stable backend prove one declaration,
    // while pg_cursors confirms exactly one open SQL cursor in that backend.
    let sql = "WITH once AS MATERIALIZED (SELECT random() AS anchor) SELECT i, anchor, pg_backend_pid(), current_setting('transaction_read_only'), (SELECT count(*) FROM pg_cursors WHERE name LIKE 'dbide_cursor_%') FROM generate_series(1,401) i CROSS JOIN once; -- trailing comment";
    let (send, mut events, task) = start(sql, defaults, limits, running());
    let mut headers = 0;
    let (first, eof, seq) = batch(&mut events, &mut headers).await;
    assert_eq!((first.len(), eof, seq), (200, false, 1));
    let pid: i32 = first[0][2].as_ref().unwrap().parse().unwrap();
    let anchor = first[0][1].clone();
    assert!(first.iter().all(|r| r[1] == anchor
        && r[2] == first[0][2]
        && r[3].as_deref() == Some("on")
        && r[4].as_deref() == Some("1")));
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(events.try_recv().is_err(), "source fetched without demand");
    let paused = observer
        .query_one(
            "SELECT state, query FROM pg_stat_activity WHERE pid=$1",
            &[&pid],
        )
        .await
        .unwrap();
    assert_eq!(paused.get::<_, String>(0), "idle in transaction");
    assert!(
        paused
            .get::<_, String>(1)
            .starts_with("FETCH FORWARD 56 FROM dbide_cursor_")
    );
    send.send(2).unwrap();
    let (second, eof, seq) = batch(&mut events, &mut headers).await;
    assert_eq!((second.len(), eof, seq), (200, false, 2));
    assert!(second.iter().all(|r| r[1] == anchor && r[2] == first[0][2]));
    send.send(3).unwrap();
    let (last, eof, seq) = batch(&mut events, &mut headers).await;
    assert_eq!((last.len(), eof, seq), (1, true, 3));
    assert_eq!(last[0][0].as_deref(), Some("401"));
    assert_eq!(last[0][1], anchor);
    assert_eq!(headers, 1);
    assert_eq!(task.await.unwrap().unwrap().rows, 401);
    no_backend(&observer).await;

    for (count, expected_batches) in [(400, vec![200, 200, 0]), (0, vec![0])] {
        let (send, mut events, task) = start(
            &format!("SELECT i FROM generate_series(1,{count}) i"),
            defaults,
            limits,
            running(),
        );
        let mut headers = 0;
        for (index, expected) in expected_batches.iter().enumerate() {
            let (rows, eof, seq) = batch(&mut events, &mut headers).await;
            assert_eq!(rows.len(), *expected);
            assert_eq!(seq, index as u64 + 1);
            assert_eq!(eof, index + 1 == expected_batches.len());
            if !eof {
                send.send(seq + 1).unwrap();
            }
        }
        assert_eq!(headers, 1);
        assert_eq!(task.await.unwrap().unwrap().rows, count as u64);
        no_backend(&observer).await;
    }
    // NULL and empty text each consume a source byte; oversized headers fail too.
    for (sql, limits, kind) in [
        (
            "SELECT NULL::text AS n, ''::text AS e",
            StreamLimits {
                max_payload_bytes: 3,
                ..limits
            },
            StreamErrorKind::Limit,
        ),
        (
            "SELECT 1 AS oversized",
            StreamLimits {
                max_row_bytes: 1,
                ..limits
            },
            StreamErrorKind::Limit,
        ),
        (
            "SELECT repeat('한글',1000)",
            StreamLimits {
                max_row_bytes: 100,
                ..limits
            },
            StreamErrorKind::Limit,
        ),
        (
            "SELECT i FROM generate_series(1,3) i",
            StreamLimits {
                max_rows: 2,
                ..limits
            },
            StreamErrorKind::Limit,
        ),
        (
            "SELECT decode('cafe','hex')",
            limits,
            StreamErrorKind::Configuration,
        ),
        (
            "SELECT ARRAY[decode('cafe','hex')]",
            limits,
            StreamErrorKind::Configuration,
        ),
        ("SELECT 'NaN'::numeric", limits, StreamErrorKind::Execution),
    ] {
        let (_send, recv) = mpsc::sync_channel(1);
        assert_eq!(
            query_cursor(
                request(),
                sql,
                limits,
                defaults,
                running(),
                recv,
                |_| Ok(())
            )
            .await
            .unwrap_err()
            .kind,
            kind,
            "{sql}"
        );
        no_backend(&observer).await;
    }
    let (_send, recv) = mpsc::sync_channel(1);
    let summary = query_cursor(
        request(),
        "SELECT NULL::text AS n, ''::text AS e",
        StreamLimits {
            max_payload_bytes: 4,
            ..limits
        },
        defaults,
        running(),
        recv,
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(
        summary,
        StreamSummary {
            rows: 1,
            payload_bytes: 4
        }
    );
    no_backend(&observer).await;

    // Errors after publication preserve callbacks but never report success/EOF.
    let (send, mut events, task) = start(
        "SELECT i, 1/(201-i) FROM generate_series(1,401) i /* cursor-secret-never-echo */",
        defaults,
        limits,
        running(),
    );
    assert_eq!(batch(&mut events, &mut 0).await.0.len(), 200);
    send.send(2).unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::Execution);
    assert!(error.message.contains("SQLSTATE 22012"));
    assert!(!format!("{error:?}").contains("cursor-secret-never-echo"));
    assert!(events.recv().await.is_none());
    no_backend(&observer).await;
    let (_send, recv) = mpsc::sync_channel(1);
    let error = query_cursor(
        request(),
        "SELECT 1",
        limits,
        defaults,
        running(),
        recv,
        |_| Err("secret sink diagnostic".into()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::Sink);
    assert!(!format!("{error:?}").contains("secret"));
    no_backend(&observer).await;

    for mode in ["cancel", "drop", "idle", "lifetime", "stale"] {
        let flag = running();
        let options = match mode {
            "idle" => CursorOptions {
                idle_timeout: Duration::from_millis(60),
                ..defaults
            },
            "lifetime" => CursorOptions {
                lifetime_timeout: Duration::from_millis(400),
                ..defaults
            },
            _ => defaults,
        };
        let (send, mut events, task) = start(
            "SELECT i FROM generate_series(1,401) i",
            options,
            limits,
            flag.clone(),
        );
        assert_eq!(batch(&mut events, &mut 0).await.0.len(), 200);
        match mode {
            "cancel" => flag.store(true, Ordering::Release),
            "drop" => drop(send),
            "stale" => send.send(1).unwrap(),
            _ => { /* Keep sender live until terminal result. */ }
        }
        let error = tokio::time::timeout(Duration::from_secs(4), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error.kind,
            match mode {
                "idle" | "lifetime" => StreamErrorKind::Timeout,
                "stale" => StreamErrorKind::Configuration,
                _ => StreamErrorKind::Cancelled,
            },
            "{mode}"
        );
        no_backend(&observer).await;
    }
    // Waiting demand is excluded from the aggregate active budget.
    let (send, mut events, task) = start(
        "SELECT i FROM generate_series(1,201) i",
        defaults,
        StreamLimits {
            timeout: Duration::from_millis(700),
            ..limits
        },
        running(),
    );
    assert_eq!(batch(&mut events, &mut 0).await.0.len(), 200);
    tokio::time::sleep(Duration::from_millis(800)).await;
    send.send(2).unwrap();
    assert!(batch(&mut events, &mut 0).await.1);
    assert_eq!(task.await.unwrap().unwrap().rows, 201);
    no_backend(&observer).await;

    // The active budget is shared across FETCH operations, never renewed.
    let (send, mut events, task) = start(
        "SELECT i, pg_sleep(0.2) FROM generate_series(1,4) i",
        CursorOptions {
            fetch_rows: 2,
            ..defaults
        },
        StreamLimits {
            timeout: Duration::from_millis(700),
            ..limits
        },
        running(),
    );
    assert_eq!(batch(&mut events, &mut 0).await.0.len(), 2);
    send.send(2).unwrap();
    assert_eq!(
        task.await.unwrap().unwrap_err().kind,
        StreamErrorKind::Timeout
    );
    no_backend(&observer).await;

    let (_send, recv) = mpsc::sync_channel(1);
    let error = query_cursor(
        request(),
        "SELECT 1",
        StreamLimits {
            timeout: Duration::from_millis(100),
            ..limits
        },
        defaults,
        running(),
        recv,
        |event| {
            if matches!(event, CursorEvent::Data(StreamEvent::Columns(_))) {
                std::thread::sleep(Duration::from_millis(120));
            }
            Ok(())
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.kind,
        StreamErrorKind::Timeout,
        "sink work counts as active time"
    );
    no_backend(&observer).await;

    for cancel in [true, false] {
        let flag = running();
        let (_send, _events, task) = start(
            "SELECT pg_sleep(30) /* cursor_active_cancel */",
            defaults,
            StreamLimits {
                timeout: Duration::from_millis(200),
                ..limits
            },
            flag.clone(),
        );
        if cancel {
            for _ in 0..100 {
                let count: i64 = observer.query_one("SELECT count(*) FROM pg_stat_activity WHERE application_name='dbide-readonly-cursor' AND state='active' AND query LIKE 'FETCH%'", &[]).await.unwrap().get(0);
                if count > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            flag.store(true, Ordering::Release);
        }
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(4), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err()
                .kind,
            if cancel {
                StreamErrorKind::Cancelled
            } else {
                StreamErrorKind::Timeout
            }
        );
        no_backend(&observer).await;
    }
    drop(observer);
    driver.await.unwrap();
}

#[tokio::test]
#[ignore = "requires already-running project-owned loopback PostgreSQL fixture; explicit opt-in"]
async fn sql_cursor_delivers_prefix_before_logical_batch_end() {
    let _fixture = LIVE_FIXTURE.lock().await;
    // SQL FETCH itself materializes its wire result. The source uses 16/64-row
    // wire chunks within one logical demand of 200 rows, so a streamable slow
    // SELECT publishes a prefix well before logical BatchEnd. Blocking server
    // plans may still postpone the first chunk. No driver patch is involved.
    owned_fixture();
    let (observer, driver) = tokio_postgres::connect(
        "postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture",
        tokio_postgres::NoTls,
    )
    .await
    .unwrap();
    let driver = tokio::spawn(async move {
        driver.await.unwrap();
    });
    for payload in [512, 4096] {
        let (_send, recv) = mpsc::sync_channel(1);
        let (first_send, first_recv) = tokio::sync::oneshot::channel();
        let mut first_send = Some(first_send);
        let (batch_send, batch_recv) = tokio::sync::oneshot::channel();
        let mut batch_send = Some(batch_send);
        let flag = running();
        let source_flag = flag.clone();
        let started = std::time::Instant::now();
        let task = tokio::spawn(async move {
            let mut received = 0usize;
            query_cursor(
                request(),
                &format!("SELECT i::text AS id, repeat('x',{payload}) AS payload, pg_sleep(0.03)::text AS wait, pg_backend_pid() AS backend FROM generate_series(1,401) i"),
                StreamLimits::default(), CursorOptions::default(), source_flag, recv,
                |event| {
                    match event {
                        CursorEvent::Data(StreamEvent::Row(row)) => {
                            received += 1;
                            if let Some(send) = first_send.take() {
                                send.send((started.elapsed(), row[3].as_ref().unwrap().parse::<i32>().unwrap())).unwrap();
                            }
                        },
                        CursorEvent::BatchEnd { eof, fetch_sequence } => {
                            assert_eq!((received, eof, fetch_sequence), (200, false, 1));
                            batch_send.take().unwrap().send((started.elapsed(), received)).unwrap();
                            // This probe needs only one FETCH. Use the normal cancel
                            // path so native cleanup remains observable and bounded.
                            flag.store(true, Ordering::Release);
                        },
                        _ => {},
                    }
                    Ok(())
                },
            ).await
        });
        let (first, pid) = tokio::time::timeout(Duration::from_secs(20), first_recv)
            .await
            .unwrap()
            .unwrap();
        let active: String = observer
            .query_opt("SELECT state FROM pg_stat_activity WHERE pid=$1", &[&pid])
            .await
            .unwrap()
            .map_or_else(|| "retired".into(), |r| r.get(0));

        let (batch, rows) = tokio::time::timeout(Duration::from_secs(8), batch_recv)
            .await
            .unwrap()
            .unwrap();
        eprintln!(
            "cursor timing payload={payload} first_row_ms={} batch_end_ms={} rows={rows} first_callback_backend_state={active}",
            first.as_millis(),
            batch.as_millis()
        );
        assert!(
            first + Duration::from_secs(2) < batch,
            "prefix must arrive before logical batch completion"
        );
        assert_ne!(
            active, "retired",
            "cursor must remain held when first prefix arrives"
        );
        assert_eq!(
            task.await.unwrap().unwrap_err().kind,
            StreamErrorKind::Cancelled
        );
        let mut retired = false;
        for _ in 0..150 {
            let count: i64 = observer
                .query_one(
                    "SELECT count(*) FROM pg_stat_activity WHERE pid=$1",
                    &[&pid],
                )
                .await
                .unwrap()
                .get(0);
            if count == 0 {
                retired = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(retired, "timing probe backend did not retire");
    }
    drop(observer);
    driver.await.unwrap();
}
