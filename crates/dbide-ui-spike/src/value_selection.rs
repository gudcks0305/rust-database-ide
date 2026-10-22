//! Bounded, immutable inspection and clipboard operations over a loaded result page.
use crate::ResultPage;
use serde::Deserialize;
use std::{fmt, ops::Range};

pub const NULL_MARKER: &str = "\\N";
pub const MAX_PREVIEW_BYTES: usize = 64 * 1024;
pub const MAX_INSPECTOR_FIELDS: usize = 256;

#[derive(Clone, Copy, Debug)]
pub struct SelectionLimits {
    pub max_cells: usize,
    pub max_bytes: usize,
}
impl Default for SelectionLimits {
    fn default() -> Self {
        Self {
            max_cells: 10_000,
            max_bytes: 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionError {
    OutOfBounds,
    EmptySelection,
    CellLimit,
    ByteLimit,
    InvalidJson,
    JsonDepthLimit,
}
impl fmt::Display for SelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OutOfBounds => "Selection is outside the loaded page",
            Self::EmptySelection => "Select at least one loaded cell",
            Self::CellLimit => "Selection exceeds the cell limit",
            Self::ByteLimit => "Selection exceeds the byte limit",
            Self::InvalidJson => "Value is not valid JSON",
            Self::JsonDepthLimit => "JSON exceeds the nesting limit (64)",
        })
    }
}
impl std::error::Error for SelectionError {}

#[derive(Clone, Debug)]
pub enum CopySelection {
    Cell {
        row: usize,
        column: usize,
    },
    Row(usize),
    Column(usize),
    /// Both bounds are zero-based and exclusive at the end. Never crosses pages.
    Range {
        rows: Range<usize>,
        columns: Range<usize>,
    },
}

fn append(out: &mut String, value: &str, max: usize) -> Result<(), SelectionError> {
    if value.len() > max.saturating_sub(out.len()) {
        return Err(SelectionError::ByteLimit);
    }
    out.push_str(value);
    Ok(())
}
fn append_tsv(out: &mut String, value: Option<&str>, max: usize) -> Result<(), SelectionError> {
    let Some(value) = value else {
        return append(out, NULL_MARKER, max);
    };
    // Literal null markers are quoted so they remain distinct from SQL NULL.
    if value == NULL_MARKER || value.contains(['\t', '\r', '\n', '"']) {
        append(out, "\"", max)?;
        for piece in value.split_inclusive('"') {
            append(out, piece, max)?;
            if piece.ends_with('"') {
                append(out, "\"", max)?;
            }
        }
        append(out, "\"", max)
    } else {
        append(out, value, max)
    }
}
pub fn copy_selection(
    page: &ResultPage,
    selection: CopySelection,
    limits: SelectionLimits,
) -> Result<String, SelectionError> {
    if let CopySelection::Cell { row, column } = selection {
        if limits.max_cells == 0 {
            return Err(SelectionError::CellLimit);
        }
        if column >= page.columns.len() {
            return Err(SelectionError::OutOfBounds);
        }
        let value = page
            .rows
            .get(row)
            .and_then(|r| r.get(column))
            .ok_or(SelectionError::OutOfBounds)?;
        let raw = value.as_deref().unwrap_or(NULL_MARKER);
        if raw.len() > limits.max_bytes {
            return Err(SelectionError::ByteLimit);
        }
        return Ok(raw.to_owned());
    }
    let (rows, columns) = match selection {
        CopySelection::Row(row) => (
            row..row.checked_add(1).ok_or(SelectionError::OutOfBounds)?,
            0..page.columns.len(),
        ),
        CopySelection::Column(column) => (
            0..page.rows.len(),
            column..column.checked_add(1).ok_or(SelectionError::OutOfBounds)?,
        ),
        CopySelection::Range { rows, columns } => (rows, columns),
        CopySelection::Cell { .. } => unreachable!(),
    };
    if rows.start > rows.end
        || columns.start > columns.end
        || rows.end > page.rows.len()
        || columns.end > page.columns.len()
    {
        return Err(SelectionError::OutOfBounds);
    }
    if rows.is_empty() || columns.is_empty() {
        return Err(SelectionError::EmptySelection);
    }
    if rows
        .len()
        .checked_mul(columns.len())
        .is_none_or(|count| count > limits.max_cells)
    {
        return Err(SelectionError::CellLimit);
    }
    let mut out = String::new();
    for row in rows.clone() {
        if row != rows.start {
            append(&mut out, "\n", limits.max_bytes)?;
        }
        for column in columns.clone() {
            if column != columns.start {
                append(&mut out, "\t", limits.max_bytes)?;
            }
            let value = page.rows[row]
                .get(column)
                .ok_or(SelectionError::OutOfBounds)?;
            append_tsv(&mut out, value.as_deref(), limits.max_bytes)?;
        }
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueSnapshot {
    pub source_label: String,
    pub row_index: usize,
    pub column: String,
    /// Representation metadata, not a claim about the database's wire type.
    pub type_label: String,
    pub raw: Option<String>,
}
impl ValueSnapshot {
    pub fn capture(
        page: &ResultPage,
        source_label: impl Into<String>,
        row: usize,
        column: usize,
    ) -> Result<Self, SelectionError> {
        let source_label = source_label.into();
        let name = page
            .columns
            .get(column)
            .ok_or(SelectionError::OutOfBounds)?;
        let raw = page
            .rows
            .get(row)
            .and_then(|r| r.get(column))
            .ok_or(SelectionError::OutOfBounds)?;
        let total = source_label
            .len()
            .checked_add(name.len())
            .and_then(|n| n.checked_add(raw.as_ref().map_or(0, String::len)));
        if total.is_none_or(|n| n > SelectionLimits::default().max_bytes) {
            return Err(SelectionError::ByteLimit);
        }
        Ok(Self {
            source_label,
            row_index: row,
            column: name.clone(),
            type_label: "Text representation".into(),
            raw: raw.clone(),
        })
    }
    pub fn raw_copy(&self) -> &str {
        self.raw.as_deref().unwrap_or(NULL_MARKER)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowSnapshot {
    pub source_label: String,
    pub row_index: usize,
    pub values: Vec<ValueSnapshot>,
}
impl RowSnapshot {
    pub fn capture(
        page: &ResultPage,
        source_label: impl Into<String>,
        row: usize,
    ) -> Result<Self, SelectionError> {
        let source_label = source_label.into();
        let values = page.rows.get(row).ok_or(SelectionError::OutOfBounds)?;
        if page.columns.is_empty() {
            return Err(SelectionError::EmptySelection);
        }
        if page.columns.len() > MAX_INSPECTOR_FIELDS {
            return Err(SelectionError::CellLimit);
        }
        if values.len() != page.columns.len() {
            return Err(SelectionError::OutOfBounds);
        }
        let mut bytes = source_label.len();
        for (name, raw) in page.columns.iter().zip(values) {
            // Every captured field owns its provenance too; charge actual retained data.
            bytes = bytes
                .checked_add(source_label.len())
                .and_then(|n| n.checked_add(name.len()))
                .and_then(|n| n.checked_add(raw.as_ref().map_or(0, String::len)))
                .ok_or(SelectionError::ByteLimit)?;
            if bytes > SelectionLimits::default().max_bytes {
                return Err(SelectionError::ByteLimit);
            }
        }
        let values = (0..page.columns.len())
            .map(|column| ValueSnapshot::capture(page, source_label.clone(), row, column))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            source_label,
            row_index: row,
            values,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectionSnapshot {
    Value(ValueSnapshot),
    Row(RowSnapshot),
}
impl InspectionSnapshot {
    pub fn values(&self) -> &[ValueSnapshot] {
        match self {
            Self::Value(value) => std::slice::from_ref(value),
            Self::Row(row) => &row.values,
        }
    }
}

/// Preview clipping is explicit and UTF-8 safe; copying uses the original snapshot.
pub fn value_preview(raw: Option<&str>) -> (String, bool) {
    let Some(raw) = raw else {
        return ("NULL".into(), false);
    };
    let mut end = raw.len().min(MAX_PREVIEW_BYTES);
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    (raw[..end].into(), end < raw.len())
}

/// Validate with an ignored-value visitor, then format lexical tokens. Numbers never
/// pass through f64, preserving huge integers, decimals and exponent spelling.
pub fn pretty_json(raw: &str, limits: SelectionLimits) -> Result<String, SelectionError> {
    if raw.len() > limits.max_bytes {
        return Err(SelectionError::ByteLimit);
    }
    let mut quoted = false;
    let mut escaped = false;
    let mut depth = 0usize;
    for byte in raw.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte == b'[' || byte == b'{' {
            depth += 1;
            if depth > 64 {
                return Err(SelectionError::JsonDepthLimit);
            }
        } else if byte == b']' || byte == b'}' {
            depth = depth.saturating_sub(1);
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    serde::de::IgnoredAny::deserialize(&mut deserializer)
        .map_err(|_| SelectionError::InvalidJson)?;
    deserializer
        .end()
        .map_err(|_| SelectionError::InvalidJson)?;
    let mut out = String::new();
    quoted = false;
    escaped = false;
    depth = 0;
    let mut previous = None;
    for ch in raw.chars() {
        if quoted {
            append(&mut out, ch.encode_utf8(&mut [0; 4]), limits.max_bytes)?;
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        if ch.is_ascii_whitespace() {
            continue;
        }
        if matches!(previous, Some('{' | '[')) && !matches!(ch, '}' | ']') {
            append(&mut out, "\n", limits.max_bytes)?;
            append(&mut out, &"  ".repeat(depth), limits.max_bytes)?;
        }
        match ch {
            '"' => {
                quoted = true;
                append(&mut out, "\"", limits.max_bytes)?;
            }
            '{' | '[' => {
                append(&mut out, ch.encode_utf8(&mut [0; 4]), limits.max_bytes)?;
                depth += 1;
            }
            '}' | ']' => {
                depth -= 1;
                if previous != Some(if ch == '}' { '{' } else { '[' }) {
                    append(&mut out, "\n", limits.max_bytes)?;
                    append(&mut out, &"  ".repeat(depth), limits.max_bytes)?;
                }
                append(&mut out, ch.encode_utf8(&mut [0; 4]), limits.max_bytes)?;
            }
            ',' => {
                append(&mut out, ",\n", limits.max_bytes)?;
                append(&mut out, &"  ".repeat(depth), limits.max_bytes)?;
            }
            ':' => append(&mut out, ": ", limits.max_bytes)?,
            _ => append(&mut out, ch.encode_utf8(&mut [0; 4]), limits.max_bytes)?,
        }
        previous = Some(ch);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn page() -> ResultPage {
        ResultPage {
            columns: vec!["a".into(), "b".into()],
            rows: vec![
                vec![None, Some("".into())],
                vec![Some("서울\t\"x\"\nnext".into()), Some("\\N".into())],
            ],
            ..Default::default()
        }
    }
    #[test]
    fn null_empty_and_tsv_escaping_are_distinct() {
        let p = page();
        let l = SelectionLimits::default();
        assert_eq!(
            copy_selection(&p, CopySelection::Row(0), l).unwrap(),
            "\\N\t"
        );
        assert_eq!(
            copy_selection(&p, CopySelection::Row(1), l).unwrap(),
            "\"서울\t\"\"x\"\"\nnext\"\t\"\\N\""
        );
        assert_eq!(
            copy_selection(&p, CopySelection::Cell { row: 1, column: 0 }, l).unwrap(),
            "서울\t\"x\"\nnext"
        );
    }
    #[test]
    fn ranges_are_bounded_and_never_partially_copied() {
        let p = page();
        let l = SelectionLimits::default();
        assert_eq!(
            copy_selection(&p, CopySelection::Column(1), l).unwrap(),
            "\n\"\\N\""
        );
        assert_eq!(
            copy_selection(
                &p,
                CopySelection::Range {
                    rows: 1..2,
                    columns: 1..2
                },
                l
            )
            .unwrap(),
            "\"\\N\""
        );
        assert_eq!(
            copy_selection(
                &p,
                CopySelection::Range {
                    rows: 0..3,
                    columns: 0..1
                },
                l
            ),
            Err(SelectionError::OutOfBounds)
        );
        assert_eq!(
            copy_selection(
                &p,
                CopySelection::Range {
                    rows: 0..0,
                    columns: 0..1
                },
                l
            ),
            Err(SelectionError::EmptySelection)
        );
        assert_eq!(
            copy_selection(
                &p,
                CopySelection::Row(1),
                SelectionLimits { max_cells: 1, ..l }
            ),
            Err(SelectionError::CellLimit)
        );
        assert_eq!(
            copy_selection(
                &p,
                CopySelection::Row(1),
                SelectionLimits { max_bytes: 4, ..l }
            ),
            Err(SelectionError::ByteLimit)
        );
    }
    #[test]
    fn capture_is_immutable_and_preserves_empty_null() {
        let mut p = page();
        let row = RowSnapshot::capture(&p, "original", 0).unwrap();
        let value = ValueSnapshot::capture(&p, "original", 1, 0).unwrap();
        p.rows[1][0] = Some("changed".into());
        p.columns[0] = "renamed".into();
        assert_eq!(value.raw_copy(), "서울\t\"x\"\nnext");
        assert_eq!(value.column, "a");
        assert_eq!(row.values[0].raw, None);
        assert_eq!(row.values[1].raw.as_deref(), Some(""));
    }
    #[test]
    fn json_keeps_large_numbers_and_escaped_strings_exact() {
        let raw = r#"{"integer":123456789012345678901234567890,"decimal":0.123456789012345678901,"exp":1e999,"s":"a\\\"b"}"#;
        let pretty = pretty_json(raw, SelectionLimits::default()).unwrap();
        assert_eq!(
            pretty_json("{\"x\":[{},1,\"서울\"]}", SelectionLimits::default()).unwrap(),
            "{\n  \"x\": [\n    {},\n    1,\n    \"서울\"\n  ]\n}"
        );
        assert!(pretty.contains("123456789012345678901234567890"));
        assert!(pretty.contains("0.123456789012345678901"));
        assert!(pretty.contains("1e999"));
        assert_eq!(
            pretty_json("{bad}", SelectionLimits::default()),
            Err(SelectionError::InvalidJson)
        );
        assert_eq!(
            pretty_json(
                &format!("{}0{}", "[".repeat(65), "]".repeat(65)),
                SelectionLimits::default()
            ),
            Err(SelectionError::JsonDepthLimit)
        );
    }
    #[test]
    fn ragged_pages_and_large_snapshots_are_rejected() {
        let mut p = page();
        p.rows[0].pop();
        assert_eq!(
            copy_selection(&p, CopySelection::Row(0), SelectionLimits::default()),
            Err(SelectionError::OutOfBounds)
        );
        assert_eq!(
            RowSnapshot::capture(&p, "source", 0),
            Err(SelectionError::OutOfBounds)
        );
        p.rows[0] = vec![Some("x".repeat(SelectionLimits::default().max_bytes)), None];
        assert_eq!(
            ValueSnapshot::capture(&p, "source", 0, 0),
            Err(SelectionError::ByteLimit)
        );
        assert_eq!(
            RowSnapshot::capture(&p, "source", 0),
            Err(SelectionError::ByteLimit)
        );
        p.columns = vec!["field".into(); MAX_INSPECTOR_FIELDS + 1];
        assert_eq!(
            RowSnapshot::capture(&p, "source", 0),
            Err(SelectionError::CellLimit)
        );
    }
    #[test]
    fn utf8_preview_and_byte_budget_are_measured_in_bytes() {
        let mut p = page();
        p.rows[0][0] = Some("서울".into());
        assert_eq!(
            copy_selection(
                &p,
                CopySelection::Cell { row: 0, column: 0 },
                SelectionLimits {
                    max_cells: 1,
                    max_bytes: 5
                }
            ),
            Err(SelectionError::ByteLimit)
        );
        let large = "서".repeat(MAX_PREVIEW_BYTES / 3 + 1);
        let (preview, clipped) = value_preview(Some(&large));
        assert!(clipped);
        assert!(preview.len() <= MAX_PREVIEW_BYTES);
        assert!(large.starts_with(&preview));
        assert_eq!(
            pretty_json(
                "[1,2]",
                SelectionLimits {
                    max_cells: 10,
                    max_bytes: 5
                }
            ),
            Err(SelectionError::ByteLimit)
        );
    }
}
