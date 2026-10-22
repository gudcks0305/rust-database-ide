//! Bounded text search over one retained result page; no database access.
use crate::ResultPage;

pub const MAX_QUERY_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug)]
pub struct FindLimits {
    pub max_cells: usize,
    pub max_bytes: usize,
    pub max_cell_bytes: usize,
    pub max_matches: usize,
}
impl Default for FindLimits {
    fn default() -> Self {
        Self {
            max_cells: 100_000,
            max_bytes: 8 * 1024 * 1024,
            max_cell_bytes: 64 * 1024,
            max_matches: 1000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellMatch {
    pub row: usize,
    pub column: usize,
}

#[derive(Debug, Default)]
pub struct FindReport {
    /// One match per cell, in row-major order. SQL NULL has no searchable text.
    pub matches: Vec<CellMatch>,
    pub scanned_cells: usize,
    pub scanned_bytes: usize,
    /// A budget stopped traversal or clipped at least one cell's text prefix.
    pub partial: bool,
    pub selected: usize,
}
impl FindReport {
    pub fn selection(&self) -> Option<CellMatch> {
        self.matches.get(self.selected).copied()
    }
    pub fn move_selection(&mut self, forward: bool) {
        let length = self.matches.len();
        self.selected = if length == 0 {
            0
        } else if forward {
            (self.selected + 1) % length
        } else {
            (self.selected + length - 1) % length
        };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindError {
    QueryTooLong,
    Cancelled,
}

/// Literal substring search. Insensitive mode uses Unicode lowercase, without
/// normalization or full case folding. Run on a worker for retained real data.
/// Limits count visited cells and original UTF-8 bytes; folded copies are bounded
/// by those same input limits. Clipped cells are explicitly reported as partial.
pub fn search_page(
    page: &ResultPage,
    query: &str,
    case_sensitive: bool,
    limits: FindLimits,
    mut cancelled: impl FnMut() -> bool,
) -> Result<FindReport, FindError> {
    if query.len() > MAX_QUERY_BYTES {
        return Err(FindError::QueryTooLong);
    }
    let mut report = FindReport::default();
    if query.is_empty() || page.columns.is_empty() {
        return Ok(report);
    }
    let needle = if case_sensitive {
        query.to_owned()
    } else {
        query.to_lowercase()
    };
    for (row_index, row) in page.rows.iter().enumerate() {
        if cancelled() {
            return Err(FindError::Cancelled);
        }
        // Malformed empty/ragged rows must not bypass the traversal budget.
        if row_index >= limits.max_cells {
            report.partial = true;
            return Ok(report);
        }
        for (column, value) in row.iter().take(page.columns.len()).enumerate() {
            if cancelled() {
                return Err(FindError::Cancelled);
            }
            if report.scanned_cells >= limits.max_cells
                || report.matches.len() >= limits.max_matches
            {
                report.partial = true;
                return Ok(report);
            }
            report.scanned_cells += 1;
            let Some(value) = value else { continue };
            let available = limits.max_bytes.saturating_sub(report.scanned_bytes);
            let mut end = value.len().min(limits.max_cell_bytes).min(available);
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            report.partial |= end < value.len();
            report.scanned_bytes += end;
            let prefix = &value[..end];
            let found = if case_sensitive {
                prefix.contains(&needle)
            } else {
                prefix.to_lowercase().contains(&needle)
            };
            if found {
                report.matches.push(CellMatch {
                    row: row_index,
                    column,
                });
            }
            if end < value.len() && available <= limits.max_cell_bytes {
                // Total byte budget cannot admit this cell's remaining text.
                return Ok(report);
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn page(values: &[Option<&str>]) -> ResultPage {
        ResultPage {
            columns: vec!["value".into()],
            rows: values.iter().map(|v| vec![v.map(str::to_owned)]).collect(),
            ..Default::default()
        }
    }
    fn find(page: &ResultPage, query: &str, sensitive: bool, limits: FindLimits) -> FindReport {
        search_page(page, query, sensitive, limits, || false).unwrap()
    }
    #[test]
    fn null_empty_literal_markers_and_unicode_are_distinct() {
        let p = page(&[
            None,
            Some(""),
            Some("NULL"),
            Some("서울 ΣK café"),
            Some("null"),
        ]);
        let l = FindLimits::default();
        assert!(find(&p, "", false, l).matches.is_empty());
        assert_eq!(
            find(&p, "NULL", true, l).matches,
            vec![CellMatch { row: 2, column: 0 }]
        );
        assert_eq!(find(&p, "null", false, l).matches.len(), 2);
        assert_eq!(
            find(&p, "서울 σk", false, l).selection(),
            Some(CellMatch { row: 3, column: 0 })
        );
        assert!(find(&p, "CAFE", false, l).matches.is_empty());
    }
    #[test]
    fn cell_byte_match_and_utf8_prefix_budgets_are_visible() {
        let p = page(&[Some("서울서울"), Some("서울"), Some("서울")]);
        let l = FindLimits::default();
        let report = find(&p, "서울", true, FindLimits { max_bytes: 5, ..l });
        assert!(report.partial);
        assert_eq!(report.scanned_bytes, 3);
        assert!(report.matches.is_empty());
        let report = find(&p, "서울", true, FindLimits { max_cells: 1, ..l });
        assert!(report.partial);
        assert_eq!(report.matches.len(), 1);
        let report = find(
            &p,
            "서울",
            true,
            FindLimits {
                max_matches: 1,
                ..l
            },
        );
        assert!(report.partial);
        assert_eq!(report.scanned_cells, 1);
        let report = find(
            &p,
            "서울",
            true,
            FindLimits {
                max_cell_bytes: 3,
                ..l
            },
        );
        assert!(report.partial);
        assert_eq!(report.scanned_bytes, 9);
        assert!(report.matches.is_empty());
        assert!(
            !find(
                &page(&[Some("x")]),
                "x",
                true,
                FindLimits {
                    max_matches: 1,
                    ..l
                }
            )
            .partial
        );
    }
    #[test]
    fn navigation_wraps_and_invalid_work_cannot_select() {
        let p = page(&[Some("x"), Some("x")]);
        let mut report = find(&p, "x", true, FindLimits::default());
        report.move_selection(false);
        assert_eq!(report.selection().unwrap().row, 1);
        report.move_selection(true);
        assert_eq!(report.selection().unwrap().row, 0);
        let mut empty = FindReport::default();
        empty.move_selection(false);
        assert!(empty.selection().is_none());
        assert!(matches!(
            search_page(&p, "x", true, FindLimits::default(), || true),
            Err(FindError::Cancelled)
        ));
        assert!(matches!(
            search_page(
                &p,
                &"x".repeat(MAX_QUERY_BYTES + 1),
                true,
                FindLimits::default(),
                || false
            ),
            Err(FindError::QueryTooLong)
        ));
    }
}
