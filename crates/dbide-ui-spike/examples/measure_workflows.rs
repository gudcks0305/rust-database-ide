//! Generated, owned-fixture model/filesystem measurements; no GUI, DB, or network.
//! Run: cargo run --release -p dbide-ui-spike --example measure_workflows
//! Timed calls include their normal allocation/I/O and result destruction is excluded.
//! Fixtures, assertions, JSON encoding, and reporting are outside timed intervals.
use dbide_catalog_store::{
    Data, Key, Object, Scope, SearchLimits, SearchProfile, SearchQuery, Snapshot, Store,
};
use dbide_project::Project;
use dbide_ui_spike::{
    ResultPage,
    result_find_model::{CellMatch, FindLimits, search_page},
};
use serde_json::{Value, json};
use std::{
    fs,
    hint::black_box,
    path::Path,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const WARMUPS: usize = 3;
const SAMPLES: usize = 30;
const ROWS: usize = 10_000;
const COLUMNS: usize = 10;
const SQL_BYTES: usize = 64 * 1024;

fn measure<T>(
    name: &str,
    fixture: Value,
    mut operation: impl FnMut() -> T,
    check: impl Fn(&T) -> Value,
) -> Value {
    for _ in 0..WARMUPS {
        let value = operation();
        black_box(&value);
        black_box(check(&value));
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut observation = Value::Null;
    for _ in 0..SAMPLES {
        let start = Instant::now();
        let value = operation();
        black_box(&value);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        observation = check(&value);
    }
    let mut sorted = samples.clone();
    sorted.sort_by(f64::total_cmp);
    // Nearest rank: sorted[ceil(p * N) - 1], without interpolation.
    json!({"name": name, "fixture": fixture, "sample_ms": samples,
        "p50_ms": sorted[(SAMPLES * 50).div_ceil(100) - 1],
        "p95_ms": sorted[(SAMPLES * 95).div_ceil(100) - 1], "observed": observation})
}

fn folder_fixture(root: &Path, name: &str, files: usize, directories: usize) {
    let folder = root.join(name);
    fs::create_dir(&folder).expect("owned fixture directory");
    for n in 0..files {
        fs::write(folder.join(format!("query-{n:04}.sql")), b"SELECT 1;\n")
            .expect("owned fixture SQL");
    }
    for n in 0..directories {
        fs::create_dir(folder.join(format!("folder-{n:02}"))).expect("owned fixture child");
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("use cargo run --release for measurements".into());
    }
    let temporary = tempfile::tempdir()?;
    let project = Project::init(temporary.path(), "generated-workflow-measurement")?;
    folder_fixture(project.root(), "small", 256, 8);
    folder_fixture(project.root(), "boundary", 512, 8);
    folder_fixture(project.root(), "large", 2000, 0);
    let sql_a = format!("-- a{}", "x".repeat(SQL_BYTES - 4));
    let sql_b = format!("-- b{}", "y".repeat(SQL_BYTES - 4));
    assert_eq!(sql_a.len(), SQL_BYTES);
    assert_eq!(sql_b.len(), SQL_BYTES);
    let path = Path::new("queries/measured.sql");
    let mut expected_hash = project.write_sql(path, &sql_a, None)?.sha256;
    let page = ResultPage {
        columns: (0..COLUMNS).map(|c| format!("column_{c}")).collect(),
        rows: (0..ROWS)
            .map(|row| {
                (0..COLUMNS)
                    .map(|column| {
                        Some(if row == ROWS - 1 && column == COLUMNS - 1 {
                            "record LAST_ROW_UNIQUE_9f31 서울".to_owned()
                        } else {
                            format!("record r{row:05} c{column:02} 서울 café")
                        })
                    })
                    .collect()
            })
            .collect(),
        ..Default::default()
    };
    let text_bytes: usize = page
        .rows
        .iter()
        .flatten()
        .filter_map(Option::as_ref)
        .map(String::len)
        .sum();
    assert!(text_bytes < FindLimits::default().max_bytes);
    let catalog_root = project.root().join("catalog");
    let mut catalog = Store::new(catalog_root);
    for shard in 0..16 {
        let schema = format!("schema_{shard:02}");
        let key = Key {
            profile_id: "generated-fixture".into(),
            fingerprint: "owned-offline".into(),
            database: "generated".into(),
            scope: Scope::Objects(schema.clone()),
        };
        let snapshot = Snapshot {
            captured_unix_ms: shard,
            truncated: false,
            fields_partial: false,
            data: Data::Objects(
                (0..500)
                    .map(|object| Object {
                        schema: schema.clone(),
                        name: if shard == 0 && object == 499 {
                            "last_unique_object_9f31".into()
                        } else {
                            format!("object_{shard:02}_{object:04}")
                        },
                        kind: "table".into(),
                    })
                    .collect(),
            ),
        };
        catalog.save(key, snapshot).expect("owned catalog fixture");
    }
    let profiles = vec![SearchProfile {
        profile_id: "generated-fixture".into(),
        fingerprint: "owned-offline".into(),
        profile_name: "generated".into(),
        engine: "offline-fixture".into(),
        database: Some("generated".into()),
    }];
    let mut measurements = Vec::new();
    for (name, query, sensitive, expected_hits, partial) in [
        (
            "result_find_last_row_sensitive",
            "LAST_ROW_UNIQUE_9f31",
            true,
            1,
            false,
        ),
        (
            "result_find_last_row_insensitive",
            "last_row_unique_9f31",
            false,
            1,
            false,
        ),
        (
            "result_find_absent_sensitive",
            "ABSENT_NEEDLE_83b2",
            true,
            0,
            false,
        ),
        (
            "result_find_absent_insensitive",
            "absent_needle_83b2",
            false,
            0,
            false,
        ),
        ("result_find_match_limit", "record", true, 1000, true),
    ] {
        measurements.push(measure(name, json!({"rows": ROWS, "columns": COLUMNS,
            "cells": ROWS * COLUMNS, "utf8_text_bytes": text_bytes, "case_sensitive": sensitive,
            "max_cells": 100_000, "max_bytes": 8 * 1024 * 1024, "max_cell_bytes": 64 * 1024, "max_matches": 1000}),
            || search_page(black_box(&page), black_box(query), sensitive, FindLimits::default(), || false).expect("search"),
            |report| {
                assert_eq!(report.matches.len(), expected_hits);
                assert_eq!(report.partial, partial);
                if !partial {
                    assert_eq!(report.scanned_cells, ROWS * COLUMNS);
                    assert_eq!(report.scanned_bytes, text_bytes);
                    if expected_hits == 1 { assert_eq!(report.selection(), Some(CellMatch { row: ROWS - 1, column: COLUMNS - 1 })); }
                } else { assert_eq!(report.scanned_cells, 1000); }
                json!({"hits": report.matches.len(), "scanned_cells": report.scanned_cells,
                    "scanned_bytes": report.scanned_bytes, "partial": report.partial})
            }));
    }
    for (name, files, directories, partial) in [
        ("small", 256, 8, false),
        ("boundary", 512, 8, true),
        ("large", 2000, 0, true),
    ] {
        measurements.push(measure(
            &format!("project_list_{name}"),
            json!({"sql_files": files,
            "directories": directories, "max_returned_entries": dbide_project::MAX_LIST_ENTRIES,
            "max_scanned_entries": dbide_project::MAX_SCANNED_ENTRIES,
            "sql_contents_read": false}),
            || project.list(black_box(Path::new(name))).expect("listing"),
            |listing| {
                assert_eq!(
                    listing.entries.len(),
                    (files + directories).min(dbide_project::MAX_LIST_ENTRIES)
                );
                assert_eq!(listing.truncated, partial);
                json!({"returned_entries": listing.entries.len(), "partial": listing.truncated,
                    "scanned_entries": "not exposed by API; fixture below scan cap"})
            },
        ));
    }
    measurements.push(measure(
        "project_read_sql_64kib",
        json!({"utf8_bytes": SQL_BYTES, "includes_sha256": true}),
        || project.read_sql(black_box(path)).expect("SQL read"),
        |file| {
            assert_eq!(file.sql, sql_a);
            assert_eq!(file.sha256, expected_hash);
            json!({"bytes": file.sql.len(), "partial": false})
        },
    ));
    let mut revision = 0usize;
    measurements.push(measure(
        "project_cas_save_sql_64kib",
        json!({"utf8_bytes": SQL_BYTES,
        "includes_cooperative_lock": true, "includes_expected_digest_check": true,
        "includes_sync_all_and_atomic_publication": true, "writers": 1}),
        || {
            revision += 1;
            let sql = if revision % 2 == 1 { &sql_b } else { &sql_a };
            let saved = project
                .write_sql(black_box(path), black_box(sql), Some(&expected_hash))
                .expect("CAS SQL save");
            expected_hash.clone_from(&saved.sha256);
            saved
        },
        |file| {
            assert_eq!(file.sql.len(), SQL_BYTES);
            json!({"bytes": file.sql.len(), "partial": false})
        },
    ));
    let saved = project.read_sql(path)?;
    assert_eq!(saved.sha256, expected_hash);
    assert_eq!(
        saved.sql,
        if revision % 2 == 1 { &sql_b } else { &sql_a }.as_str()
    );
    for (name, text, expected_hits, partial) in [
        ("catalog_search_unique", "last_unique_object_9f31", 1, false),
        ("catalog_search_absent", "absent_object_83b2", 0, false),
        ("catalog_search_match_limit", "object", 100, true),
    ] {
        let query = SearchQuery {
            text: text.into(),
            kind: Some("table".into()),
        };
        measurements.push(measure(name, json!({"shards": 16, "objects": 8000,
            "max_results": 100, "max_shards": 64, "max_bytes": 4 * 1024 * 1024,
            "source": "generated historical metadata, not live database"}),
            || catalog.search_objects(black_box(&profiles), black_box(&query), SearchLimits::default()).expect("catalog search"),
            |report| {
                assert_eq!(report.matches.len(), expected_hits);
                assert_eq!(report.partial, partial);
                assert_eq!(report.skipped_shards, 0);
                assert!(report.cache_present);
                if !partial { assert_eq!(report.scanned_shards, 16); }
                json!({"hits": report.matches.len(), "scanned_shards": report.scanned_shards,
                    "payload_bytes_read": report.bytes_read, "partial": report.partial, "skipped_shards": report.skipped_shards})
            }));
    }
    let report = json!({"schema_version": 1, "scope": "model and warm local filesystem API calls only; excludes UI paint, event-loop latency, database execution and end-to-end application latency",
        "method": {"clock": "std::time::Instant", "warmups_per_case": WARMUPS, "samples_per_case": SAMPLES,
            "percentile": "nearest-rank sorted[ceil(p*N)-1]; N=30, p50=rank15, p95=rank29",
            "setup_timed": false, "assertions_timed": false, "result_drop_timed": false,
            "cache": "warm repeated calls; OS caches not cleared", "fixtures": "deterministic generated data in owned TempDir removed on normal exit",
            "cas_hash_update_timed": true, "parallel_workloads": false},
        "environment": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            "family": std::env::consts::FAMILY, "available_parallelism": std::thread::available_parallelism().map(|n| n.get()).ok(),
            "debug_assertions": cfg!(debug_assertions), "unix_timestamp_seconds": SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            "build_profile": "release requested by documented invocation; debug_assertions disabled",
            "cpu_model": "not collected", "filesystem": "host temporary directory; storage medium not detected"},
        "measurements": measurements});
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
