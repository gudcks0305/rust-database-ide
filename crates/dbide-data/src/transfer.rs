use crate::*;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormulaMode {
    /// Preserve all text exactly, including spreadsheet formulas.
    #[default]
    Preserve,
    /// Prefix dangerous spreadsheet text with apostrophe. Explicit, non-reversible output mode.
    Escape,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CsvScalarMode {
    /// CSV imports only Null/Text. Reject export of cells carrying other type information.
    #[default]
    TextOnly,
    /// Render scalar values as text; CSV import deliberately does not infer their types.
    TextualValues,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsvOptions {
    pub delimiter: u8,
    pub header: bool,
    /// None forbids NULL cells. A nonempty marker distinguishes NULL and empty text.
    pub null_marker: Option<String>,
    pub formula_mode: FormulaMode,
    pub scalar_mode: CsvScalarMode,
}
impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: b',',
            header: true,
            null_marker: Some("\\N".into()),
            formula_mode: FormulaMode::Preserve,
            scalar_mode: CsvScalarMode::TextOnly,
        }
    }
}
impl CsvOptions {
    fn validate(&self) -> Result<()> {
        if !self.delimiter.is_ascii() || matches!(self.delimiter, 0 | b'\r' | b'\n' | b'"') {
            return Err(Error::Invalid(
                "delimiter must be ASCII, excluding NUL/CR/LF/quote".into(),
            ));
        }
        if self.null_marker.as_ref().is_some_and(String::is_empty) {
            return Err(Error::Invalid(
                "empty NULL marker would collide with empty text".into(),
            ));
        }
        if self.formula_mode == FormulaMode::Escape
            && self
                .null_marker
                .as_ref()
                .is_some_and(|marker| spreadsheet_text(marker, FormulaMode::Escape) != *marker)
        {
            return Err(Error::Invalid(
                "spreadsheet escaping requires a non-formula NULL marker".into(),
            ));
        }
        Ok(())
    }
}

/// Preview stops at max_rows and reports one observed extra record. Import rejects excess rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    Preview,
    Import,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataPreview {
    pub data: TabularData,
    pub truncated: bool,
}

/// Reads one CSV record at a time. UTF-8, arity, raw byte and logical payload budgets are enforced.
/// With header=false the caller must supply columns. Header=true obtains names from the file.
/// The NULL marker is an explicit file contract; literal matching text cannot be distinguished
/// from NULL when reading files authored elsewhere.
pub fn read_csv<R: Read>(
    reader: R,
    options: &CsvOptions,
    columns: Option<Vec<Column>>,
    limits: Limits,
    mode: ReadMode,
) -> Result<DataPreview> {
    limits.validate()?;
    options.validate()?;
    if options.header && columns.is_some() {
        return Err(Error::Invalid(
            "supply columns only for headerless CSV".into(),
        ));
    }
    let input = BoundedReader {
        inner: reader,
        remaining: limits.max_bytes,
    };
    let mut csv = csv::ReaderBuilder::new()
        .delimiter(options.delimiter)
        .has_headers(options.header)
        .flexible(true)
        .from_reader(input);
    let columns = if options.header {
        csv.headers()?
            .iter()
            .map(|name| Column { name: name.into() })
            .collect()
    } else {
        columns.ok_or_else(|| Error::Invalid("headerless CSV requires columns".into()))?
    };
    let mut bytes = validate_columns(&columns, limits)?;
    let mut rows = Vec::new();
    let mut record = csv::StringRecord::new();
    while csv.read_record(&mut record)? {
        if record.len() != columns.len() {
            return Err(Error::Invalid("row/column arity mismatch".into()));
        }
        if rows.len() == limits.max_rows {
            return match mode {
                ReadMode::Preview => Ok(DataPreview {
                    data: TabularData { columns, rows },
                    truncated: true,
                }),
                ReadMode::Import => Err(Error::Limit("row count".into())),
            };
        }
        let row: Vec<Cell> = record
            .iter()
            .map(|value| {
                if options.null_marker.as_deref() == Some(value) {
                    Cell::Null
                } else {
                    Cell::Text(value.into())
                }
            })
            .collect();
        validate_row(&row, columns.len(), &mut bytes, limits)?;
        rows.push(row);
    }
    Ok(DataPreview {
        data: TabularData { columns, rows },
        truncated: false,
    })
}

/// Streams rows to Write without constructing the full export. Errors may leave partial output;
/// callers saving files should write a fresh temporary file then atomically publish on success.
/// NULL marker collisions are rejected even for quoted fields: quoting does not escape semantics.
pub fn write_csv<W: Write, I>(
    writer: W,
    columns: &[Column],
    rows: I,
    options: &CsvOptions,
    limits: Limits,
) -> Result<()>
where
    I: IntoIterator,
    I::Item: AsRef<[Cell]>,
{
    options.validate()?;
    let mut bytes = validate_columns(columns, limits)?;
    let output = BoundedWriter {
        inner: writer,
        remaining: limits.max_bytes,
    };
    let mut csv = csv::WriterBuilder::new()
        .delimiter(options.delimiter)
        .from_writer(output);
    if options.header {
        csv.write_record(
            columns
                .iter()
                .map(|c| spreadsheet_text(&c.name, options.formula_mode)),
        )?;
    }
    for (index, row) in rows.into_iter().enumerate() {
        if index >= limits.max_rows {
            return Err(Error::Limit("row count".into()));
        }
        let row = row.as_ref();
        validate_row(row, columns.len(), &mut bytes, limits)?;
        let values: Result<Vec<String>> = row.iter().map(|cell| csv_value(cell, options)).collect();
        csv.write_record(values?)?;
    }
    csv.flush()?;
    Ok(())
}
fn spreadsheet_text(value: &str, mode: FormulaMode) -> String {
    let candidate = value.trim_start_matches(' ');
    if mode == FormulaMode::Escape && candidate.starts_with(['=', '+', '-', '@', '\t', '\r', '\n'])
    {
        format!("'{value}")
    } else {
        value.into()
    }
}
fn csv_value(cell: &Cell, options: &CsvOptions) -> Result<String> {
    if matches!(cell, Cell::Null) {
        return options
            .null_marker
            .clone()
            .ok_or_else(|| Error::Invalid("NULL requires an explicit marker".into()));
    }
    let text = match cell {
        Cell::Text(value) => value.clone(),
        _ if options.scalar_mode == CsvScalarMode::TextOnly => {
            return Err(Error::Invalid(
                "typed CSV export requires explicit TextualValues mode".into(),
            ));
        }
        Cell::Bool(value) => value.to_string(),
        Cell::Integer(value) => value.to_string(),
        Cell::Decimal(value) => value.clone(),
        Cell::Binary(_) => {
            return Err(Error::Invalid(
                "binary CSV export unsupported; use tagged JSON".into(),
            ));
        }
        Cell::Null => unreachable!(),
    };
    if options.null_marker.as_deref() == Some(text.as_str()) {
        return Err(Error::Invalid(
            "text collides with NULL marker; choose another marker".into(),
        ));
    }
    let output = spreadsheet_text(&text, options.formula_mode);
    if options.null_marker.as_deref() == Some(output.as_str()) {
        return Err(Error::Invalid(
            "escaped text collides with NULL marker".into(),
        ));
    }
    Ok(output)
}

/// Versioned tagged JSON retains schema, NULL, empty strings, binary, exact decimals and kinds.
/// It intentionally does not import arbitrary untyped JSON with guessed SQL types.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonDocument {
    version: u32,
    data: TabularData,
}
pub fn write_json<W: Write>(writer: W, data: &TabularData, limits: Limits) -> Result<()> {
    data.validate(limits)?;
    let mut writer = BoundedWriter {
        inner: writer,
        remaining: limits.max_bytes,
    };
    #[derive(Serialize)]
    struct BorrowedDocument<'a> {
        version: u32,
        data: &'a TabularData,
    }
    serde_json::to_writer(&mut writer, &BorrowedDocument { version: 1, data })?;
    writer.flush()?;
    Ok(())
}
pub fn read_json<R: Read>(reader: R, limits: Limits) -> Result<TabularData> {
    limits.validate()?;
    let mut bytes = Vec::new();
    reader
        .take((limits.max_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > limits.max_bytes {
        return Err(Error::Limit("raw JSON bytes".into()));
    }
    let document: JsonDocument = serde_json::from_slice(&bytes)?;
    if document.version != 1 {
        return Err(Error::Invalid("unsupported JSON transfer version".into()));
    }
    document.data.validate(limits)?;
    Ok(document.data)
}

struct BoundedReader<R> {
    inner: R,
    remaining: usize,
}
impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0];
            return if self.inner.read(&mut probe)? == 0 {
                Ok(0)
            } else {
                Err(std::io::Error::other("raw CSV byte budget exceeded"))
            };
        }
        let length = buf.len().min(self.remaining);
        let n = self.inner.read(&mut buf[..length])?;
        self.remaining -= n;
        Ok(n)
    }
}
struct BoundedWriter<W> {
    inner: W,
    remaining: usize,
}
impl<W: Write> Write for BoundedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > self.remaining {
            return Err(std::io::Error::other("encoded output byte budget exceeded"));
        }
        let n = self.inner.write(buf)?;
        self.remaining -= n;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnMapping {
    pub source_index: usize,
    pub target_name: String,
    /// NULL may be allowed independently. Other kinds must match exactly; no coercion occurs.
    pub expected_kind: CellKind,
    pub nullable: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingIssue {
    pub row: usize,
    pub column: usize,
    pub actual: CellKind,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPreview {
    pub mapping: Vec<ColumnMapping>,
    pub rows: Vec<Vec<Cell>>,
    pub issues: Vec<MappingIssue>,
}
impl ImportPreview {
    pub fn ready(&self) -> bool {
        self.issues.is_empty()
    }
}
/// Explicit one-to-one mapping; no dropped source columns, duplicate targets or silent coercion.
/// Row/column positions in issues are zero-based. Issues prevent ready() and must block submit.
pub fn preview_import(
    data: &TabularData,
    mapping: &[ColumnMapping],
    limits: Limits,
) -> Result<ImportPreview> {
    data.validate(limits)?;
    if mapping.len() != data.columns.len() {
        return Err(Error::Invalid(
            "mapping must cover every source column".into(),
        ));
    }
    let mut sources = std::collections::HashSet::new();
    let mut targets = std::collections::HashSet::new();
    let mut name_bytes = 0;
    for column in mapping {
        if column.source_index >= data.columns.len()
            || !sources.insert(column.source_index)
            || column.target_name.is_empty()
            || !targets.insert(&column.target_name)
        {
            return Err(Error::Invalid(
                "mapping has invalid/duplicate source or target".into(),
            ));
        }
        add_bytes(&mut name_bytes, column.target_name.len(), limits)?;
    }
    let mut issues = Vec::new();
    let mut rows = Vec::with_capacity(data.rows.len());
    for (row_index, row) in data.rows.iter().enumerate() {
        let mut mapped = Vec::with_capacity(mapping.len());
        for (column_index, column) in mapping.iter().enumerate() {
            let value = &row[column.source_index];
            let matches = if value.kind() == CellKind::Null {
                column.nullable
            } else {
                value.kind() == column.expected_kind
            };
            if !matches {
                issues.push(MappingIssue {
                    row: row_index,
                    column: column_index,
                    actual: value.kind(),
                });
            }
            mapped.push(value.clone());
        }
        rows.push(mapped);
    }
    Ok(ImportPreview {
        mapping: mapping.to_vec(),
        rows,
        issues,
    })
}
