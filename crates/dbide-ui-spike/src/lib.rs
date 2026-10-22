//! Deterministic logical data source with no retained row collection.
pub mod console_session;
pub mod explorer_model;
pub mod history_model;
pub mod navigator;
pub mod object_finder_model;
pub mod project_files;
pub mod recovery_model;
pub mod result_find_model;
pub mod selection_execution;
pub mod session_credentials;
pub mod table_open_model;
pub mod workbench_model;
use std::ops::Range;

pub const LOGICAL_ROWS: usize = 1_000_000;
pub const LOGICAL_COLUMNS: usize = 200;

/// Defensive boundary for virtualizer callbacks (exclusive end, including empty ranges).
pub fn bounded_range(range: Range<usize>, length: usize) -> Range<usize> {
    let start = range.start.min(length);
    start..range.end.min(length).max(start)
}

/// Useful for fixture expectations; no materialized row or cell collection is retained.
pub fn fixture_value(row: usize, column: usize) -> String {
    match column {
        0 => (row + 1).to_string(),
        1 => format!("customer_{:07}", row + 1),
        2 => format!("customer{}@example.test", row + 1),
        3 => ["active", "pending", "archived"][row % 3].to_string(),
        4 => format!("{}.{:02}", row % 10_000, (row * 7) % 100),
        5 => format!("서울 · 고객 {}", row + 1),
        _ if (row + column).is_multiple_of(17) => "NULL".to_string(),
        _ => format!("r{} · c{}", row + 1, column + 1),
    }
}

pub fn column_name(column: usize) -> String {
    match column {
        0 => "id".to_string(),
        1 => "name".to_string(),
        2 => "email".to_string(),
        3 => "status".to_string(),
        4 => "balance".to_string(),
        5 => "display_name".to_string(),
        _ => format!("field_{:03}", column + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_ranges_clamp_to_logical_edges() {
        assert_eq!(
            bounded_range(999_990..1_000_020, LOGICAL_ROWS),
            999_990..1_000_000
        );
        assert_eq!(bounded_range(197..210, LOGICAL_COLUMNS), 197..200);
        assert_eq!(bounded_range(210..220, LOGICAL_COLUMNS), 200..200);
        assert_eq!(bounded_range(0..5, 0), 0..0);
        assert_eq!(bounded_range(Range { start: 10, end: 5 }, 20), 10..10);
        assert_eq!(bounded_range(0..usize::MAX, LOGICAL_ROWS), 0..LOGICAL_ROWS);
    }

    #[test]
    fn fixture_is_stable_at_far_corner_without_allocating_rows() {
        assert_eq!(fixture_value(999_999, 0), "1000000");
        assert_eq!(fixture_value(999_999, 199), "r1000000 · c200");
        assert_eq!(fixture_value(0, 17), "NULL");
        assert_eq!(fixture_value(0, 5), "서울 · 고객 1");
        assert_eq!(column_name(199), "field_200");
    }
}

/// Translate the virtualizer's scrollable-only range into full logical columns.
/// The fixed columns are reported separately; ranges include partially visible/overscan cells.
pub fn logical_column_range(range: Range<usize>, fixed: usize, total: usize) -> Range<usize> {
    bounded_range(
        range.start.saturating_add(fixed)..range.end.saturating_add(fixed),
        total,
    )
}

#[derive(Debug, Default)]
pub struct ResultPage {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub affected_rows: Option<u64>,
    pub command_only: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct QuerySummary {
    pub outcome: dbide_pg_spike::Outcome,
    pub execution: dbide_pg_spike::ExecutionKey,
    pub result_count: usize,
    pub retained_rows: usize,
    pub retained_bytes: usize,
    pub truncated: bool,
    pub connection_retired: bool,
    pub elapsed_micros: u128,
    pub transaction_state: dbide_pg_spike::ServerTransactionState,
    pub terminal_error: Option<String>,
    pub notices: Vec<String>,
}

/// Keep result-set and command order while moving bounded driver-owned row strings.
/// No extra full-result clone is needed at the UI boundary.
pub fn split_report(report: dbide_pg_spike::ExecutionReport) -> (Vec<ResultPage>, QuerySummary) {
    use dbide_pg_spike::Event;
    let mut pages: Vec<ResultPage> = Vec::new();
    let mut active = None;
    let mut event_error = None;
    for event in report.events {
        match event {
            Event::ResultSetStarted { columns } => {
                pages.push(ResultPage {
                    columns,
                    ..Default::default()
                });
                active = Some(pages.len() - 1);
            }
            Event::Row { values } => {
                if let Some(ix) = active {
                    pages[ix].rows.push(values);
                }
            }
            Event::CommandComplete { affected_rows } => {
                if let Some(ix) = active.take() {
                    pages[ix].affected_rows = Some(affected_rows);
                } else {
                    pages.push(ResultPage {
                        affected_rows: Some(affected_rows),
                        command_only: true,
                        ..Default::default()
                    });
                }
            }
            Event::CommandCompleteUnknown => {
                if active.take().is_none() {
                    pages.push(ResultPage {
                        command_only: true,
                        ..Default::default()
                    });
                }
            }
            Event::Error { sqlstate, message } => {
                event_error = Some(format!(
                    "{}{}",
                    sqlstate.map(|s| format!("{s}: ")).unwrap_or_default(),
                    message
                ));
            }
        }
    }
    let terminal_error = report
        .terminal_error
        .map(|e| {
            format!(
                "{}{}",
                e.sqlstate.map(|s| format!("{s}: ")).unwrap_or_default(),
                e.message
            )
        })
        .or(event_error);
    let summary = QuerySummary {
        outcome: report.outcome,
        execution: report.key,
        result_count: pages.len(),
        retained_rows: pages.iter().map(|p| p.rows.len()).sum(),
        retained_bytes: report.retained_bytes,
        truncated: report.truncated,
        connection_retired: report.connection_retired,
        elapsed_micros: report.elapsed_micros,
        transaction_state: report.transaction_state,
        terminal_error,
        notices: report
            .notices
            .into_iter()
            .map(|n| format!("{} {}: {}", n.severity, n.sqlstate, n.message))
            .collect(),
    };
    (pages, summary)
}

#[cfg(test)]
mod report_tests {
    use super::*;
    use dbide_pg_spike::{
        CommitPolicy, Event, ExecutionKey, ExecutionReport, Outcome, ServerError,
        ServerTransactionState,
    };

    #[test]
    fn virtual_columns_account_for_pinned_id() {
        assert_eq!(logical_column_range(191..199, 1, 200), 192..200);
        assert_eq!(logical_column_range(0..6, 1, 200), 1..7);
        assert_eq!(logical_column_range(199..300, 1, 200), 200..200);
    }

    #[test]
    fn ordered_results_keep_commands_null_empty_partial_error() {
        let report = ExecutionReport {
            key: ExecutionKey {
                session: 1,
                generation: 1,
                execution: 1,
            },
            commit_policy: CommitPolicy::Auto,
            transaction_state: ServerTransactionState::Unknown,
            outcome: Outcome::Failed,
            terminal_error: Some(ServerError {
                sqlstate: Some("XX000".into()),
                message: "terminal retained".into(),
            }),
            events: vec![
                Event::ResultSetStarted {
                    columns: vec!["value".into()],
                },
                Event::Row { values: vec![None] },
                Event::Row {
                    values: vec![Some(String::new())],
                },
                Event::CommandComplete { affected_rows: 2 },
                Event::CommandComplete { affected_rows: 7 },
                Event::CommandCompleteUnknown,
                Event::ResultSetStarted {
                    columns: vec!["partial".into()],
                },
                Event::Row {
                    values: vec![Some("서울".into())],
                },
                Event::CommandCompleteUnknown,
            ],
            notices: vec![],
            retained_bytes: 300,
            truncated: true,
            connection_retired: true,
            elapsed_micros: 123,
        };
        let (pages, summary) = split_report(report);
        assert_eq!(pages.len(), 4);
        assert_eq!(pages[0].rows, vec![vec![None], vec![Some(String::new())]]);
        assert_eq!(pages[0].affected_rows, Some(2));
        assert!(pages[1].command_only);
        assert_eq!(pages[1].affected_rows, Some(7));
        assert!(pages[2].command_only);
        assert_eq!(pages[2].affected_rows, None);
        assert_eq!(pages[3].rows[0][0].as_deref(), Some("서울"));
        assert_eq!(summary.retained_rows, 3);
        assert!(summary.truncated && summary.connection_retired);
        assert_eq!(
            summary.terminal_error.as_deref(),
            Some("XX000: terminal retained")
        );
    }
}

/// The driver currently exposes row values as text plus a distinct SQL NULL.
/// Preserve those exact semantics; do not infer lossy numeric/date types.
pub fn result_data(page: &ResultPage) -> dbide_data::TabularData {
    dbide_data::TabularData {
        columns: page
            .columns
            .iter()
            .map(|name| dbide_data::Column { name: name.clone() })
            .collect(),
        rows: page
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|value| {
                        value.as_ref().map_or(dbide_data::Cell::Null, |text| {
                            dbide_data::Cell::Text(text.clone())
                        })
                    })
                    .collect()
            })
            .collect(),
    }
}
#[cfg(test)]
mod transfer_tests {
    use super::*;
    #[test]
    fn result_transfer_preserves_null_empty_and_literal_marker() {
        let page = ResultPage {
            columns: vec!["value".into()],
            rows: vec![
                vec![None],
                vec![Some(String::new())],
                vec![Some("\\N".into())],
            ],
            ..Default::default()
        };
        let data = result_data(&page);
        assert_eq!(
            data.rows,
            vec![
                vec![dbide_data::Cell::Null],
                vec![dbide_data::Cell::Text(String::new())],
                vec![dbide_data::Cell::Text("\\N".into())]
            ]
        );
        let mut csv = vec![];
        assert!(
            dbide_data::write_csv(
                &mut csv,
                &data.columns,
                &data.rows,
                &dbide_data::CsvOptions::default(),
                dbide_data::Limits::default()
            )
            .is_err()
        );
        let mut json = vec![];
        dbide_data::write_json(&mut json, &data, dbide_data::Limits::default()).unwrap();
        assert_eq!(
            dbide_data::read_json(json.as_slice(), dbide_data::Limits::default()).unwrap(),
            data
        );
    }
}

/// Connection is intentionally not a gate: the worker reconnects once before
/// dispatch. Busy includes opening/cancelling to prevent duplicate submissions.
pub fn can_submit_query(worker_available: bool, configured: bool, busy: bool, sql: &str) -> bool {
    worker_available && configured && !busy && !sql.trim().is_empty()
}

#[cfg(test)]
mod submission_tests {
    use super::*;
    #[test]
    fn configured_disconnected_workbench_allows_run() {
        assert!(can_submit_query(true, true, false, "SELECT 1"));
    }
    #[test]
    fn incomplete_or_busy_workbench_never_admits_duplicate_run() {
        assert!(!can_submit_query(true, false, false, "SELECT 1"));
        assert!(!can_submit_query(false, true, false, "SELECT 1"));
        assert!(!can_submit_query(true, true, true, "SELECT 1"));
        assert!(!can_submit_query(true, true, false, " \n\t"));
    }
}

/// Settle only the metadata request currently displayed by the explorer.
/// This deliberately receives no main-session/execution state: a separate
/// metadata connection failure cannot retire or clear the user's SQL session.
pub fn settle_schema_operation(
    pending: &mut Option<dbide_bridge::OperationId>,
    loading: &mut bool,
    completed: dbide_bridge::OperationId,
) -> bool {
    if *pending != Some(completed) {
        return false;
    }
    *pending = None;
    *loading = false;
    true
}

#[cfg(test)]
mod metadata_tests {
    use super::*;
    use dbide_bridge::OperationId;
    #[test]
    fn metadata_failure_releases_loading_and_allows_refresh() {
        let mut pending = Some(OperationId(5));
        let mut loading = true;
        assert!(settle_schema_operation(
            &mut pending,
            &mut loading,
            OperationId(5)
        ));
        assert!(!loading);
        assert_eq!(pending, None);
    }
    #[test]
    fn old_profile_metadata_failure_cannot_settle_new_profile_refresh() {
        let mut pending = Some(OperationId(6));
        let mut loading = true;
        assert!(!settle_schema_operation(
            &mut pending,
            &mut loading,
            OperationId(5)
        ));
        assert!(loading);
        assert_eq!(pending, Some(OperationId(6)));
    }
}

pub fn write_atomic(
    path: &std::path::Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    write(file.as_file_mut())?;
    file.as_file_mut().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
#[cfg(test)]
mod file_tests {
    use super::*;
    #[test]
    fn failed_export_preserves_existing_destination_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("result.csv");
        std::fs::write(&path, b"original").unwrap();
        assert!(
            write_atomic(&path, |out| {
                std::io::Write::write_all(out, b"partial")?;
                Err(std::io::Error::other("simulated disk failure"))
            })
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
    }
}
pub mod sql_files;
pub mod table_edit_model;
pub mod value_selection;
/// Column keys are positional because SQL may return empty/duplicate labels.
pub fn result_grid_columns(names: Vec<String>) -> Vec<gpui_component::table::Column> {
    names
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let width = match index {
                0 => 90.,
                2 => 250.,
                5 => 190.,
                _ => 155.,
            };
            let column = gpui_component::table::Column::new(format!("column-{index}"), name)
                .width(gpui::px(width))
                .movable(false);
            if index == 0 {
                column.fixed_left()
            } else {
                column
            }
        })
        .collect()
}
#[cfg(test)]
mod column_tests {
    #[test]
    fn duplicate_and_empty_headers_keep_distinct_column_keys() {
        let columns = super::result_grid_columns(vec!["id".into(), "id".into(), String::new()]);
        assert_eq!(
            columns
                .iter()
                .map(|c| c.name.to_string())
                .collect::<Vec<_>>(),
            vec!["id", "id", ""]
        );
        assert_eq!(
            columns
                .iter()
                .map(|c| c.key.clone())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
    }
}
