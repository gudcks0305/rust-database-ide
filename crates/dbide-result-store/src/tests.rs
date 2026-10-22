use super::*;
use dbide_export::ExportOptions;
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine};
use rusqlite::Connection;
use tempfile::TempDir;

struct Fixture {
    _dir: TempDir,
    connection: Connection,
    profile: ConnectionProfile,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT)")
            .unwrap();
        Self {
            profile: ConnectionProfile {
                engine: DatabaseEngine::Sqlite,
                sqlite_path: Some(path.to_str().unwrap().into()),
                ..ConnectionProfile::default()
            },
            connection,
            _dir: dir,
        }
    }

    fn request(&self, sql: &str) -> QueryRequest {
        QueryRequest {
            connection: ConnectionRequest {
                profile: self.profile.clone(),
                password: String::new(),
            },
            sql: sql.into(),
            options: ExportOptions::default(),
        }
    }

    fn query(&self, sql: &str, limits: PageLimits) -> Result<PagedResult, ExportError> {
        query_pages(self.request(sql), limits, Arc::default(), Arc::default())
    }
}

fn temp_builder(dir: &TempDir, limits: PageLimits) -> StoreBuilder {
    let mut builder = StoreBuilder::new(limits).unwrap();
    builder.file = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    builder
}

fn fresh_deadline() -> CaptureDeadline {
    CaptureDeadline {
        started: Instant::now(),
        timeout: Duration::from_secs(30),
    }
}

fn expired_deadline() -> CaptureDeadline {
    CaptureDeadline {
        started: Instant::now() - Duration::from_secs(2),
        timeout: Duration::from_secs(1),
    }
}

#[test]
fn capture_over_4096_rows_and_512kib_has_one_stable_execution() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<PagedResult>();
    let fixture = Fixture::new();
    fixture
        .connection
        .execute_batch(
            "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<5000)
         INSERT INTO items SELECT i, printf('%0256d',i) FROM n",
        )
        .unwrap();
    let store = fixture
        .query(
            "SELECT id,value FROM items ORDER BY id",
            PageLimits::default(),
        )
        .unwrap();
    assert_eq!(store.summary.rows, 5000);
    assert!(store.encoded_bytes() > 512 * 1024);
    assert_eq!(store.page_count(), 25);
    let id = store.summary.execution_id;
    let first = store.read_page(0).unwrap();
    let last = store.read_page(24).unwrap();
    assert_eq!(first.row_start, 0);
    assert_eq!(last.row_start, 4800);
    assert_eq!(last.rows.last().unwrap()[0].as_deref(), Some("5000"));
    fixture
        .connection
        .execute_batch("DELETE FROM items; INSERT INTO items VALUES(99999,'changed')")
        .unwrap();
    for index in 0..store.page_count() {
        let page = store.read_page(index).unwrap();
        assert_eq!(page.rows.len(), 200);
        assert_eq!(store.summary.execution_id, id);
    }
    assert_eq!(store.read_page(0).unwrap(), first);
    assert_eq!(store.read_page(24).unwrap(), last);
}

#[test]
fn escaped_unicode_null_and_wide_rows_use_variable_byte_bounded_pages() {
    let fixture = Fixture::new();
    let values = [
        Some("\"\\\n한글😊".repeat(7)),
        None,
        Some(String::new()),
        Some("\0\r\t".repeat(30)),
    ];
    for (id, value) in values.iter().cycle().take(25).enumerate() {
        fixture
            .connection
            .execute(
                "INSERT INTO items VALUES (?1,?2)",
                rusqlite::params![id as i64, value],
            )
            .unwrap();
    }
    let limits = PageLimits {
        max_page_bytes: 600,
        ..PageLimits::default()
    };
    let store = fixture
        .query("SELECT value FROM items ORDER BY id", limits)
        .unwrap();
    let mut rows = Vec::new();
    for index in 0..store.page_count() {
        assert!(store.pages[index].len <= limits.max_page_bytes);
        rows.extend(store.read_page(index).unwrap().rows);
    }
    assert!(store.page_count() > 1);
    assert_eq!(
        rows,
        values
            .iter()
            .cycle()
            .take(25)
            .map(|value| vec![value.clone()])
            .collect::<Vec<_>>()
    );
    assert_eq!(
        store.encoded_bytes,
        store
            .file
            .lock()
            .unwrap()
            .as_file()
            .metadata()
            .unwrap()
            .len()
    );
}

#[test]
fn empty_result_has_columns_and_one_empty_page() {
    let fixture = Fixture::new();
    let store = fixture
        .query("SELECT id,value FROM items", PageLimits::default())
        .unwrap();
    assert_eq!(store.summary.rows, 0);
    assert_eq!(store.page_count(), 1);
    assert_eq!(
        store.read_page(0).unwrap(),
        StoredPage {
            columns: vec!["id".into(), "value".into()],
            rows: vec![],
            row_start: 0,
        }
    );
    assert!(store.read_page(1).is_err());
    assert!(store.read_page(usize::MAX).is_err());
}

#[test]
fn oversized_encoded_row_and_header_reject_the_entire_capture() {
    let fixture = Fixture::new();
    fixture
        .connection
        .execute("INSERT INTO items VALUES (1,?1)", ["\n".repeat(100)])
        .unwrap();
    let error = fixture
        .query(
            "SELECT value FROM items",
            PageLimits {
                max_page_bytes: 150,
                ..PageLimits::default()
            },
        )
        .err()
        .unwrap();
    assert!(error.message.contains("record exceeds byte limit"));
    let error = fixture
        .query(
            "SELECT 1 AS very_long_column_name",
            PageLimits {
                max_page_bytes: 10,
                ..PageLimits::default()
            },
        )
        .err()
        .unwrap();
    assert!(error.message.contains("record exceeds byte limit"));
}

#[test]
fn encoded_spool_cap_counts_header_and_json_escaping() {
    let fixture = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let builder = temp_builder(
        &dir,
        PageLimits {
            max_spool_bytes: 20,
            ..PageLimits::default()
        },
    );
    let path = builder.file.path().to_owned();
    let error = capture(
        fixture.request("SELECT char(10,10,10,10,10,10,10,10) AS x"),
        Arc::default(),
        Arc::default(),
        builder,
        fresh_deadline(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("spool byte limit"));
    assert!(!path.exists());
}

#[test]
fn late_source_error_and_cancellation_remove_provisional_files() {
    let fixture = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let builder = temp_builder(&dir, PageLimits::default());
    let path = builder.file.path().to_owned();
    let error = capture(
        fixture.request(
            "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<500)
         SELECT CASE WHEN i=300 THEN abs(-9223372036854775808) ELSE i END FROM n",
        ),
        Arc::default(),
        Arc::default(),
        builder,
        fresh_deadline(),
    )
    .err()
    .unwrap();
    assert_eq!(error.kind, ExportErrorKind::Execution);
    assert!(!path.exists());
    let builder = temp_builder(&dir, PageLimits::default());
    let path = builder.file.path().to_owned();
    let error = capture(
        fixture.request("SELECT 1"),
        Arc::new(AtomicBool::new(true)),
        Arc::default(),
        builder,
        fresh_deadline(),
    )
    .err()
    .unwrap();
    assert_eq!(error.kind, ExportErrorKind::Cancelled);
    assert!(!path.exists());
    let store = fixture.query("SELECT 1", PageLimits::default()).unwrap();
    let mut builder = temp_builder(&dir, PageLimits::default());
    let path = builder.file.path().to_owned();
    builder.push(StreamEvent::Columns(&["x".into()])).unwrap();
    builder
        .push(StreamEvent::Row(&[Some("provisional".into())]))
        .unwrap();
    assert!(
        builder
            .finish(
                store.summary.clone(),
                &AtomicBool::new(true),
                fresh_deadline()
            )
            .is_err()
    );
    assert!(!path.exists());
}

#[test]
fn expired_capture_preflight_and_final_sync_never_publish_and_remove_files() {
    let fixture = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let builder = temp_builder(&dir, PageLimits::default());
    let path = builder.file.path().to_owned();
    let error = capture(
        fixture.request("SELECT missing_column_before_execution"),
        Arc::default(),
        Arc::default(),
        builder,
        expired_deadline(),
    )
    .err()
    .unwrap();
    assert_eq!(error.kind, ExportErrorKind::Execution);
    assert_eq!(error.message, "query capture deadline exceeded");
    assert!(!path.exists());

    let summary = fixture
        .query("SELECT 1", PageLimits::default())
        .unwrap()
        .summary
        .clone();
    let mut builder = temp_builder(&dir, PageLimits::default());
    let path = builder.file.path().to_owned();
    builder.push(StreamEvent::Columns(&["x".into()])).unwrap();
    builder.push(StreamEvent::Row(&[Some("1".into())])).unwrap();
    // Inject expiration without sleeping: finish must reject even after a
    // complete successful sync_all rather than returning this finished store.
    let error = builder
        .finish(summary, &AtomicBool::new(false), expired_deadline())
        .err()
        .unwrap();
    assert_eq!(error.kind, ExportErrorKind::Execution);
    assert_eq!(error.message, "query capture deadline exceeded");
    assert!(!path.exists());
}

#[test]
fn dropping_last_arc_removes_file_and_read_failure_keeps_caller_page() {
    let fixture = Fixture::new();
    let store = Arc::new(
        fixture
            .query("SELECT 'stable' AS value", PageLimits::default())
            .unwrap(),
    );
    let clone = store.clone();
    let path = store.file.lock().unwrap().path().to_owned();
    let displayed = store.read_page(0).unwrap();
    store.file.lock().unwrap().as_file().set_len(0).unwrap();
    assert!(store.read_page(0).is_err());
    assert_eq!(displayed.rows, vec![vec![Some("stable".into())]]);
    drop(store);
    assert!(path.exists());
    drop(clone);
    assert!(!path.exists());
}

#[test]
fn corrupt_utf8_width_and_record_counts_are_rejected() {
    let fixture = Fixture::new();
    for bytes in [
        b"[\"\xff\"]\n".as_slice(),
        b"[null,null]\n",
        b"[null]\n[null]\n",
        b"[null] ",
    ] {
        let mut store = fixture
            .query("SELECT 'one' AS value", PageLimits::default())
            .unwrap();
        let page = &mut store.pages[0];
        page.len = bytes.len();
        store.encoded_bytes = page.offset + bytes.len() as u64;
        let mut file = store.file.lock().unwrap();
        file.as_file_mut()
            .seek(SeekFrom::Start(page.offset))
            .unwrap();
        file.as_file_mut().write_all(bytes).unwrap();
        drop(file);
        assert!(store.read_page(0).is_err());
    }
}

#[test]
fn protocol_and_hard_page_count_limit_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let limits = PageLimits {
        max_rows_per_page: 1,
        ..PageLimits::default()
    };
    let mut builder = temp_builder(&dir, limits);
    assert!(builder.push(StreamEvent::Row(&[None])).is_err());
    builder.push(StreamEvent::Columns(&["x".into()])).unwrap();
    assert!(builder.push(StreamEvent::Columns(&["x".into()])).is_err());
    assert!(builder.push(StreamEvent::Row(&[None, None])).is_err());
    for _ in 0..MAX_PAGES {
        builder.push(StreamEvent::Row(&[None])).unwrap();
    }
    let path = builder.file.path().to_owned();
    assert!(
        builder
            .push(StreamEvent::Row(&[None]))
            .unwrap_err()
            .contains("page limit")
    );
    drop(builder);
    assert!(!path.exists());
}

#[test]
fn invalid_limits_and_unbounded_source_options_rejected() {
    let fixture = Fixture::new();
    for limits in [
        PageLimits {
            max_rows_per_page: 0,
            ..PageLimits::default()
        },
        PageLimits {
            max_rows_per_page: 201,
            ..PageLimits::default()
        },
        PageLimits {
            max_page_bytes: 0,
            ..PageLimits::default()
        },
        PageLimits {
            max_page_bytes: MAX_PAGE_BYTES + 1,
            ..PageLimits::default()
        },
        PageLimits {
            max_spool_bytes: 0,
            ..PageLimits::default()
        },
        PageLimits {
            max_spool_bytes: MAX_SPOOL_BYTES + 1,
            ..PageLimits::default()
        },
    ] {
        assert_eq!(
            fixture.query("SELECT 1", limits).err().unwrap().kind,
            ExportErrorKind::Configuration
        );
    }
    let mut request = fixture.request("SELECT 1");
    request.options.max_rows = MAX_ROWS + 1;
    assert_eq!(
        query_pages(
            request,
            PageLimits::default(),
            Arc::default(),
            Arc::default()
        )
        .err()
        .unwrap()
        .kind,
        ExportErrorKind::Configuration
    );
}
