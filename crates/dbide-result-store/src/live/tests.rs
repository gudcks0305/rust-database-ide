use super::*;
use dbide_profiles::{ConnectionProfile, ConnectionRequest, StoredExecutionTarget};
use std::sync::mpsc::channel;

fn summary(rows: u64) -> QuerySummary {
    let profile = ConnectionProfile::fixture();
    QuerySummary {
        rows,
        payload_bytes: rows,
        execution_target: StoredExecutionTarget::capture(&profile, &profile.database).unwrap(),
        sql_sha256: dbide_project::digest_sql("SELECT 1"),
        execution_id: 1,
    }
}

fn setup(limits: PageLimits, mode: LiveMode) -> (LiveQuery, LiveBuilder, mpsc::Receiver<u64>) {
    let (query, receiver) = new_control(limits, mode);
    let builder = LiveBuilder::new(
        query.shared.clone(),
        Duration::from_secs(30),
        Instant::now(),
    )
    .unwrap();
    (query, builder, receiver)
}

fn columns(builder: &mut LiveBuilder) {
    builder
        .push(StreamEvent::Columns(&["value".into()]))
        .unwrap();
}

fn row(builder: &mut LiveBuilder, value: &str) {
    builder
        .push(StreamEvent::Row(&[Some(value.into())]))
        .unwrap();
}

#[test]
fn first_page_is_readable_while_producer_is_gated_before_eof() {
    let (query, mut builder, _) = setup(PageLimits::default(), LiveMode::Stream);
    let (ready_tx, ready_rx) = channel();
    let (gate_tx, gate_rx) = channel();
    let (second_ready_tx, second_ready_rx) = channel();
    let (eof_tx, eof_rx) = channel();
    let worker = std::thread::spawn(move || {
        columns(&mut builder);
        row(&mut builder, "first");
        ready_tx.send(()).unwrap();
        gate_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        row(&mut builder, "second");
        second_ready_tx.send(()).unwrap();
        eof_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        builder.finish(summary(2)).unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let first = query.snapshot();
    assert_eq!(first.rows, 1);
    assert_eq!(first.pages, 1);
    assert_eq!(first.state, LiveState::Streaming);
    assert!(!query.is_finished());
    assert_eq!(
        query.read_page(0).unwrap().rows,
        vec![vec![Some("first".into())]]
    );
    gate_tx.send(()).unwrap();
    second_ready_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    // The source has returned its second callback and now blocks before EOF.
    // Visibility cannot depend on a third callback or a source-driven timer.
    assert_eq!(query.snapshot().rows, 2);
    assert_eq!(query.read_page(0).unwrap().rows.len(), 2);
    assert!(!query.is_finished());
    eof_tx.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(query.snapshot().state, LiveState::Complete);
    assert_eq!(query.read_page(0).unwrap().rows.len(), 2);
}

#[test]
fn published_last_page_grows_but_prior_pages_are_immutable() {
    let limits = PageLimits {
        max_rows_per_page: 2,
        ..PageLimits::default()
    };
    let (query, mut builder, _) = setup(limits, LiveMode::Stream);
    columns(&mut builder);
    row(&mut builder, "a");
    let version = query.snapshot().version;
    row(&mut builder, "b");
    assert!(query.read_page_versioned(0).unwrap().1 > version);
    assert_eq!(query.read_page(0).unwrap().rows.len(), 2);
    let sealed = query.read_page(0).unwrap();
    row(&mut builder, "c");
    assert_eq!(query.snapshot().pages, 2);
    assert_eq!(query.read_page(0).unwrap(), sealed);
    assert_eq!(query.read_page(1).unwrap().row_start, 2);
    builder.finish(summary(3)).unwrap();
    assert_eq!(query.read_page(0).unwrap(), sealed);
    assert_eq!(
        query.read_page_versioned(1).unwrap().1,
        query.snapshot().version
    );
}

#[test]
fn immediate_click_at_awaiting_publication_cannot_reopen_duplicate_admission() {
    let (query, mut builder, demand) = setup(PageLimits::default(), LiveMode::PostgresCursor);
    columns(&mut builder);
    row(&mut builder, "first batch");
    builder
        .end_batch(false, 1, || {
            // Deterministic interleaving: the UI runs immediately after AwaitingFetch
            // becomes visible, before batch_end has returned to the source.
            query.fetch_next().unwrap();
            assert_eq!(demand.try_recv().unwrap(), 2);
        })
        .unwrap();
    // Releasing producer-side admission after the callback would let this click
    // enqueue the same sequence again, or request a second future batch.
    query.fetch_next().unwrap();
    assert!(demand.try_recv().is_err());
    assert_eq!(query.snapshot().state, LiveState::Streaming);
    row(&mut builder, "second batch");
    builder
        .end_batch(false, 2, || {
            query.fetch_next().unwrap();
            assert_eq!(demand.try_recv().unwrap(), 3);
        })
        .unwrap();
    query.fetch_next().unwrap();
    assert!(demand.try_recv().is_err());
    builder
        .end_batch(true, 3, || {
            assert!(query.fetch_next().is_err());
        })
        .unwrap();
}

#[test]
fn batch_boundary_seals_partial_page_and_demand_is_bounded_and_deduplicated() {
    let (query, mut builder, demand) = setup(PageLimits::default(), LiveMode::PostgresCursor);
    columns(&mut builder);
    row(&mut builder, "a");
    builder.batch_end(false, 1).unwrap();
    let sealed = query.read_page(0).unwrap();
    assert_eq!(query.snapshot().state, LiveState::AwaitingFetch);
    query.fetch_next().unwrap();
    query.fetch_next().unwrap();
    assert_eq!(query.snapshot().state, LiveState::Streaming);
    assert_eq!(demand.try_recv().unwrap(), 2);
    assert!(demand.try_recv().is_err());
    row(&mut builder, "b");
    builder.batch_end(false, 2).unwrap();
    assert_eq!(query.read_page(0).unwrap(), sealed);
    assert_eq!(query.read_page(1).unwrap().rows[0][0].as_deref(), Some("b"));
    query.fetch_next().unwrap();
    assert_eq!(demand.try_recv().unwrap(), 3);
    builder.batch_end(true, 3).unwrap();
    assert_ne!(query.snapshot().state, LiveState::Complete);
    builder.finish(summary(2)).unwrap();
    assert!(query.fetch_next().is_err());
}

#[test]
fn late_failure_and_cancellation_keep_prefix_with_terminal_state() {
    for state in [LiveState::Failed, LiveState::Cancelled] {
        let (query, mut builder, _) = setup(PageLimits::default(), LiveMode::Stream);
        columns(&mut builder);
        row(&mut builder, "prefix");
        row(&mut builder, "coalesced");
        builder.publish(LiveState::Streaming, None).unwrap();
        publish_state(
            &query.shared,
            state,
            Some("late source failure".into()),
            None,
        );
        assert_eq!(query.snapshot().state, state);
        assert_eq!(query.snapshot().rows, 2);
        assert_eq!(query.read_page(0).unwrap().rows.len(), 2);
        assert_eq!(
            query.read_page_versioned(0).unwrap().1,
            query.snapshot().version
        );
        assert!(query.snapshot().summary.is_none());
    }
}

#[test]
fn snapshot_and_cancel_do_not_wait_for_metadata_or_disk() {
    let (query, _builder, _) = setup(PageLimits::default(), LiveMode::Stream);
    let _metadata_guard = query.shared.metadata.lock().unwrap();
    assert_eq!(query.snapshot().state, LiveState::Starting);
    query.cancel();
    assert!(query.shared.cancel.load(Ordering::Acquire));
}

#[test]
fn last_control_drop_cancels_and_file_lives_only_until_worker_retires() {
    let (query, mut builder, _) = setup(PageLimits::default(), LiveMode::Stream);
    columns(&mut builder);
    row(&mut builder, "a");
    let path = builder.shared.spool.get().unwrap().path().to_owned();
    assert!(path.exists());
    drop(query);
    assert!(builder.shared.cancel.load(Ordering::Acquire));
    assert!(builder.check().is_err());
    assert!(path.exists());
    drop(builder);
    assert!(!path.exists());
}

#[test]
fn empty_eof_retains_one_empty_page_and_header() {
    let (query, mut builder, _) = setup(PageLimits::default(), LiveMode::Stream);
    columns(&mut builder);
    builder.finish(summary(0)).unwrap();
    assert_eq!(query.snapshot().pages, 1);
    assert_eq!(
        query.read_page(0).unwrap(),
        StoredPage {
            columns: vec!["value".into()],
            rows: vec![],
            row_start: 0
        }
    );
}

#[test]
fn bounds_cover_json_escaping_delimiters_headers_and_page_index() {
    let (query, mut builder, _) = setup(
        PageLimits {
            max_page_bytes: 16,
            max_spool_bytes: 24,
            ..PageLimits::default()
        },
        LiveMode::Stream,
    );
    columns(&mut builder);
    row(&mut builder, "a");
    assert!(
        builder
            .push(StreamEvent::Row(&[Some("\n".repeat(10))]))
            .is_err()
    );
    assert_eq!(query.read_page(0).unwrap().rows.len(), 1);
    row(&mut builder, "b");
    assert!(builder.push(StreamEvent::Row(&[Some("c".into())])).is_err());
    let (_query, mut header, _) = setup(
        PageLimits {
            max_page_bytes: 4,
            ..PageLimits::default()
        },
        LiveMode::Stream,
    );
    assert!(header.push(StreamEvent::Columns(&["long".into()])).is_err());
    let (_query, mut index, _) = setup(PageLimits::default(), LiveMode::Stream);
    columns(&mut index);
    index.pages = MAX_PAGES;
    assert!(index.push(StreamEvent::Row(&[Some("x".into())])).is_err());
}

#[test]
fn expired_or_cancelled_capture_cannot_be_complete() {
    let (query, mut builder, _) = setup(PageLimits::default(), LiveMode::Stream);
    columns(&mut builder);
    row(&mut builder, "prefix");
    builder.timeout = Duration::ZERO;
    assert!(builder.finish(summary(1)).is_err());
    assert_ne!(query.snapshot().state, LiveState::Complete);
    assert_eq!(query.read_page(0).unwrap().rows.len(), 1);
    let (query, mut builder, _) = setup(PageLimits::default(), LiveMode::Stream);
    columns(&mut builder);
    query.cancel();
    assert_eq!(
        builder.finish(summary(0)).unwrap_err().kind,
        ExportErrorKind::Cancelled
    );
}

fn sqlite_request(path: &std::path::Path, sql: &str) -> QueryRequest {
    QueryRequest {
        connection: ConnectionRequest {
            profile: ConnectionProfile {
                engine: DatabaseEngine::Sqlite,
                sqlite_path: Some(path.to_str().unwrap().into()),
                ..ConnectionProfile::default()
            },
            password: String::new(),
        },
        sql: sql.into(),
        options: dbide_export::ExportOptions::default(),
    }
}

fn wait_terminal(query: &LiveQuery) -> LiveSnapshot {
    let started = Instant::now();
    while !query.is_finished() {
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    query.snapshot()
}

#[test]
fn real_sqlite_stream_and_configuration_guards() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE data(value); INSERT INTO data VALUES('한글'),(NULL),('');")
        .unwrap();
    let request = sqlite_request(&path, "SELECT value FROM data ORDER BY rowid");
    let query = start_live_query(request.clone(), PageLimits::default(), LiveMode::Stream).unwrap();
    assert_eq!(wait_terminal(&query).state, LiveState::Complete);
    assert_eq!(
        query.read_page(0).unwrap().rows,
        vec![vec![Some("한글".into())], vec![None], vec![Some("".into())]]
    );
    assert!(
        start_live_query(
            request.clone(),
            PageLimits::default(),
            LiveMode::PostgresCursor
        )
        .is_err()
    );
    let mut changed = request;
    changed.options.expect_sql_sha256 = Some("wrong".into());
    assert!(start_live_query(changed, PageLimits::default(), LiveMode::Stream).is_err());
}

#[test]
fn real_sqlite_late_error_publishes_all_complete_prefix_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    rusqlite::Connection::open(&path).unwrap();
    let request = sqlite_request(
        &path,
        "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<400) SELECT CASE WHEN i=300 THEN abs(-9223372036854775808) ELSE i END FROM n",
    );
    let query = start_live_query(request, PageLimits::default(), LiveMode::Stream).unwrap();
    let snapshot = wait_terminal(&query);
    assert_eq!(snapshot.state, LiveState::Failed);
    assert_eq!(snapshot.rows, 299);
    assert_eq!(snapshot.pages, 2);
    assert!(snapshot.error.is_some());
    assert!(snapshot.summary.is_none());
    assert_eq!(query.read_page(0).unwrap().rows.len(), 200);
    let last = query.read_page(1).unwrap();
    assert_eq!(last.rows.len(), 99);
    assert_eq!(last.rows.last().unwrap()[0].as_deref(), Some("299"));
}

#[test]
fn local_sink_error_is_actionable_bounded_and_does_not_replace_source_errors() {
    use dbide_driver_api::stream::{StreamError, StreamErrorKind};
    let (query, mut builder, _) = setup(
        PageLimits {
            max_page_bytes: 16,
            ..PageLimits::default()
        },
        LiveMode::PostgresCursor,
    );
    columns(&mut builder);
    row(&mut builder, "a");
    let reason = builder
        .push(StreamEvent::Row(&[Some("\"".repeat(20))]))
        .unwrap_err();
    let error = cursor_error(
        StreamError::new(StreamErrorKind::Sink, "Cursor sink failed"),
        Some(bounded_sink_error(&reason)),
    );
    assert!(error.message.contains("16-byte page limit"));
    assert!(!error.message.contains("Cursor sink failed"));
    publish_state(&query.shared, LiveState::Failed, Some(error.message), None);
    assert_eq!(query.snapshot().state, LiveState::Failed);
    assert_eq!(
        query.read_page(0).unwrap().rows,
        vec![vec![Some("a".into())]]
    );
    let source = cursor_error(
        StreamError::new(StreamErrorKind::Execution, "sanitized source diagnostic"),
        Some("local reason".into()),
    );
    assert_eq!(source.message, "sanitized source diagnostic");
    assert_eq!(
        cursor_error(
            StreamError::new(StreamErrorKind::Cancelled, "cancelled"),
            Some("local reason".into())
        )
        .kind,
        ExportErrorKind::Cancelled
    );
    let bounded = bounded_sink_error(&"한글".repeat(1000));
    assert!(bounded.len() <= 1024);
    assert!(!bounded.is_empty());
}

fn owned_pg_fixture() {
    let inspected = std::process::Command::new("docker")
        .args(["inspect", "--format", "{{json .}}", "dbide-m0-pg"])
        .output()
        .expect("inspect project-owned fixture");
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

fn pg_request(sql: &str) -> QueryRequest {
    QueryRequest {
        connection: ConnectionRequest {
            profile: ConnectionProfile::fixture(),
            password: "dbide_fixture_only".into(),
        },
        sql: sql.into(),
        options: dbide_export::ExportOptions::default(),
    }
}

fn await_state(query: &LiveQuery, state: LiveState) -> LiveSnapshot {
    let started = Instant::now();
    loop {
        let snapshot = query.snapshot();
        if snapshot.state == state {
            return snapshot;
        }
        assert!(
            !snapshot.state.terminal(),
            "unexpected terminal snapshot: {snapshot:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
#[ignore = "requires already-running project-owned loopback PostgreSQL fixture; explicit opt-in"]
fn live_postgres_cursor_preserves_one_execution_and_partial_failure() {
    owned_pg_fixture();
    let query = start_live_query(pg_request("WITH once AS MATERIALIZED (SELECT random() AS anchor) SELECT i,anchor,pg_backend_pid() FROM generate_series(1,401) i CROSS JOIN once"), PageLimits::default(), LiveMode::PostgresCursor).unwrap();
    let first = await_state(&query, LiveState::AwaitingFetch);
    assert_eq!((first.rows, first.pages), (200, 1));
    let page = query.read_page(0).unwrap();
    let anchor = page.rows[0][1].clone();
    let pid = page.rows[0][2].clone();
    std::thread::sleep(Duration::from_millis(100));
    let paused = query.snapshot();
    assert_eq!(
        (paused.rows, paused.pages, paused.version),
        (first.rows, first.pages, first.version)
    );
    query.fetch_next().unwrap();
    assert_eq!(await_state(&query, LiveState::AwaitingFetch).rows, 400);
    assert_eq!(query.read_page(0).unwrap(), page);
    query.fetch_next().unwrap();
    let complete = wait_terminal(&query);
    assert_eq!(
        (complete.state, complete.rows, complete.pages),
        (LiveState::Complete, 401, 3)
    );
    assert_eq!(complete.summary.as_ref().unwrap().rows, 401);
    for index in 0..3 {
        let page = query.read_page(index).unwrap();
        assert!(
            page.rows
                .iter()
                .all(|row| row[1] == anchor && row[2] == pid)
        );
    }
    assert_eq!(
        query.read_page(2).unwrap().rows[0][0].as_deref(),
        Some("401")
    );

    let query = start_live_query(
        pg_request("SELECT i,1/(201-i) FROM generate_series(1,401) i"),
        PageLimits::default(),
        LiveMode::PostgresCursor,
    )
    .unwrap();
    assert_eq!(await_state(&query, LiveState::AwaitingFetch).rows, 200);
    let first = query.read_page(0).unwrap();
    query.fetch_next().unwrap();
    let failed = wait_terminal(&query);
    assert_eq!(
        (failed.state, failed.rows, failed.pages),
        (LiveState::Failed, 200, 1)
    );
    assert_eq!(query.read_page(0).unwrap(), first);
    assert!(failed.summary.is_none());

    let query = start_live_query(
        pg_request("SELECT CASE WHEN i=201 THEN repeat(chr(34),300000) ELSE i::text END AS value FROM generate_series(1,201) i"),
        PageLimits::default(), LiveMode::PostgresCursor,
    ).unwrap();
    assert_eq!(await_state(&query, LiveState::AwaitingFetch).rows, 200);
    let first = query.read_page(0).unwrap();
    query.fetch_next().unwrap();
    let failed = wait_terminal(&query);
    assert_eq!((failed.state, failed.rows), (LiveState::Failed, 200));
    let error = failed.error.unwrap();
    assert!(error.contains("524288-byte page limit"), "{error}");
    assert!(
        error.contains("encoded result record exceeds byte limit"),
        "{error}"
    );
    assert!(error.len() <= 1024);
    assert_eq!(query.read_page(0).unwrap(), first);

    let query = start_live_query(
        pg_request("SELECT i FROM generate_series(1,401) i"),
        PageLimits::default(),
        LiveMode::PostgresCursor,
    )
    .unwrap();
    await_state(&query, LiveState::AwaitingFetch);
    query.cancel();
    assert_eq!(wait_terminal(&query).state, LiveState::Cancelled);
    assert_eq!(query.read_page(0).unwrap().rows.len(), 200);

    let query = start_live_query(
        pg_request("SELECT i FROM generate_series(1,401) i"),
        PageLimits::default(),
        LiveMode::PostgresCursor,
    )
    .unwrap();
    await_state(&query, LiveState::AwaitingFetch);
    let path = query.shared.spool.get().unwrap().path().to_owned();
    drop(query);
    let started = Instant::now();
    while path.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "dropped live cursor did not retire"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
