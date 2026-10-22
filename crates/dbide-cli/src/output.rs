use super::*;
use dbide_data::{Cell, Column, CsvOptions, Limits, TabularData};
use dbide_driver_api::{Event, ExecutionReport};
use tempfile::NamedTempFile;

pub(super) fn check(destination: &Destination) -> Result<()> {
    check_destination(
        destination,
        "output exists; choose a fresh path or use --force",
    )
}
pub(super) fn check_fresh(path: &Path) -> Result<()> {
    check_destination(
        &Destination {
            output: Some(path.into()),
            force: false,
        },
        "output exists; export requires a fresh path",
    )
}
fn check_destination(destination: &Destination, conflict: &str) -> Result<()> {
    if let Some(path) = &destination.output {
        let exists = match fs::symlink_metadata(path) {
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(config(error)),
        };
        if !destination.force && exists {
            return Err(config(conflict));
        }
        if path.is_dir() {
            return Err(config("output must be a file"));
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if !parent.is_dir() {
            return Err(config(
                "output parent directory must already exist; nothing submitted",
            ));
        }
        // Reject an unwritable destination before potentially mutating SQL runs.
        // The final atomic publication still handles races independently.
        drop(NamedTempFile::new_in(parent).map_err(config)?);
    }
    Ok(())
}
pub(super) fn publish(destination: &Destination, bytes: &[u8]) -> Result<()> {
    if let Some(path) = &destination.output {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temporary = NamedTempFile::new_in(parent).map_err(config)?;
        temporary.write_all(bytes).map_err(config)?;
        temporary.as_file().sync_all().map_err(config)?;
        if destination.force {
            temporary.persist(path).map_err(config)?;
        } else {
            temporary.persist_noclobber(path).map_err(config)?;
        }
        Ok(())
    } else {
        let mut stdout = io::stdout().lock();
        stdout.write_all(bytes).map_err(config)?;
        if !bytes.ends_with(b"\n") {
            stdout.write_all(b"\n").map_err(config)?;
        }
        stdout.flush().map_err(config)
    }
}
/// Back up the exact original bytes with private permissions before the store mutates.
pub(super) fn backup(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut source = fs::File::open(path).map_err(config)?;
    let prefix = format!(
        "{}.backup-",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    let mut temporary = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".json")
        .tempfile_in(parent)
        .map_err(config)?;
    io::copy(&mut source, &mut temporary).map_err(config)?;
    temporary.as_file().sync_all().map_err(config)?;
    let (_, path) = temporary.keep().map_err(config)?;
    Ok(path)
}
fn sets(report: &ExecutionReport) -> Result<Vec<TabularData>> {
    let mut sets: Vec<TabularData> = vec![];
    for event in &report.events {
        match event {
            Event::ResultSetStarted { columns } => sets.push(TabularData {
                columns: columns
                    .iter()
                    .map(|name| Column { name: name.clone() })
                    .collect(),
                rows: vec![],
            }),
            Event::Row { values } => {
                let set = sets
                    .last_mut()
                    .ok_or_else(|| Failure::new(4, "driver row has no result-set header"))?;
                if values.len() != set.columns.len() {
                    return Err(Failure::new(4, "driver row/header arity mismatch"));
                }
                set.rows.push(
                    values
                        .iter()
                        .map(|v| v.clone().map_or(Cell::Null, Cell::Text))
                        .collect(),
                );
            }
            _ => {}
        }
    }
    Ok(sets)
}
pub(super) fn report(
    report: &ExecutionReport,
    format: Format,
    index: Option<usize>,
    null_marker: &str,
    max_rows: usize,
) -> Result<Vec<u8>> {
    if matches!(format, Format::Json) {
        if index.is_some() {
            return Err(config(
                "--result-index is for table/csv; JSON always includes the full report",
            ));
        }
        return serde_json::to_vec_pretty(report).map_err(config);
    }
    let sets = sets(report)?;
    if matches!(format, Format::Csv) {
        if sets.is_empty() {
            return Err(Failure::new(
                4,
                "CSV needs a result set; use JSON for command counts/errors",
            ));
        }
        if sets.len() > 1 && index.is_none() {
            return Err(config(
                "multiple result sets; supply --result-index (zero-based) for CSV or use JSON",
            ));
        }
        let set = sets
            .get(index.unwrap_or(0))
            .ok_or_else(|| config("result index is out of range"))?;
        let mut bytes = vec![];
        let options = CsvOptions {
            null_marker: Some(null_marker.into()),
            ..CsvOptions::default()
        };
        // Bridge strings preserve their exact contents and positional labels. No type guessing.
        dbide_data::write_csv(
            &mut bytes,
            &set.columns,
            &set.rows,
            &options,
            Limits::default(),
        )
        .map_err(config)?;
        return Ok(bytes);
    }
    if index.is_some_and(|i| i >= sets.len()) {
        return Err(config("result index is out of range"));
    }
    let mut text = String::new();
    for (i, set) in sets
        .iter()
        .enumerate()
        .filter(|(i, _)| index.is_none_or(|wanted| wanted == *i))
    {
        text.push_str(&format!("Result set {i}\n"));
        text.push_str(
            &set.columns
                .iter()
                .enumerate()
                .map(|(i, c)| format!("[{i}] {}", display(&c.name)))
                .collect::<Vec<_>>()
                .join("\t"),
        );
        text.push('\n');
        for row in set.rows.iter().take(max_rows) {
            text.push_str(
                &row.iter()
                    .map(|c| match c {
                        Cell::Null => "NULL".into(),
                        Cell::Text(v) => display(v),
                        _ => unreachable!(),
                    })
                    .collect::<Vec<_>>()
                    .join("\t"),
            );
            text.push('\n');
        }
        if set.rows.len() > max_rows {
            text.push_str(&format!("… {} retained rows hidden; --max-rows controls table display; JSON/CSV export all retained rows\n",set.rows.len()-max_rows));
        }
    }
    for event in &report.events {
        match event {
            Event::CommandComplete { affected_rows } => {
                text.push_str(&format!("Affected rows: {affected_rows}\n"))
            }
            Event::CommandCompleteUnknown => {
                text.push_str("Command completed (affected count unavailable)\n")
            }
            Event::Error { message, .. } => {
                text.push_str(&format!("Error: {}\n", display(message)))
            }
            _ => {}
        }
    }
    text.push_str(&format!(
        "Outcome: {:?}; retained bytes: {}; truncated: {}\n",
        report.outcome, report.retained_bytes, report.truncated
    ));
    Ok(text.into_bytes())
}
fn display(value: &str) -> String {
    let prefix: String = value.chars().take(256).collect();
    let suffix = if value.chars().nth(256).is_some() {
        "… [display shortened; use JSON/CSV]"
    } else {
        ""
    };
    format!(
        "{}{suffix}",
        serde_json::to_string(&prefix).expect("string serialization")
    )
}
