use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RowDifference {
    Insert {
        key: Vec<Cell>,
        after: Vec<Cell>,
    },
    Delete {
        key: Vec<Cell>,
        before: Vec<Cell>,
    },
    Change {
        key: Vec<Cell>,
        before: Vec<Cell>,
        after: Vec<Cell>,
        changed_columns: Vec<usize>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataDiff {
    pub key_columns: Vec<usize>,
    pub compared_before: usize,
    pub compared_after: usize,
    pub unchanged: usize,
    pub differences: Vec<RowDifference>,
}
/// Exact kind/value comparison, including decimal lexemes. No positional or guessed identity.
/// Each side must have unique, non-NULL explicit keys. Both complete inputs must fit limits.
/// Deletions/changes preserve before order; insertions preserve after order.
pub fn diff_by_key(
    before: &TabularData,
    after: &TabularData,
    key_columns: &[usize],
    limits: Limits,
) -> Result<DataDiff> {
    before.validate(limits)?;
    after.validate(limits)?;
    if before.columns != after.columns {
        return Err(Error::Invalid("diff schemas differ".into()));
    }
    validate_key_columns(key_columns, before.columns.len())?;
    let before_keys = index_keys(&before.rows, key_columns)?;
    let after_keys = index_keys(&after.rows, key_columns)?;
    let mut differences = Vec::new();
    let mut unchanged = 0;
    for row in &before.rows {
        let key = row_key(row, key_columns)?;
        match after_keys.get(&key) {
            None => differences.push(RowDifference::Delete {
                key,
                before: row.clone(),
            }),
            Some(&index) => {
                let new = &after.rows[index];
                if row == new {
                    unchanged += 1;
                } else {
                    let changed_columns = row
                        .iter()
                        .zip(new)
                        .enumerate()
                        .filter_map(|(i, (a, b))| (a != b).then_some(i))
                        .collect();
                    differences.push(RowDifference::Change {
                        key,
                        before: row.clone(),
                        after: new.clone(),
                        changed_columns,
                    });
                }
            }
        }
    }
    for row in &after.rows {
        let key = row_key(row, key_columns)?;
        if !before_keys.contains_key(&key) {
            differences.push(RowDifference::Insert {
                key,
                after: row.clone(),
            });
        }
    }
    Ok(DataDiff {
        key_columns: key_columns.to_vec(),
        compared_before: before.rows.len(),
        compared_after: after.rows.len(),
        unchanged,
        differences,
    })
}
fn index_keys(rows: &[Vec<Cell>], columns: &[usize]) -> Result<HashMap<Vec<Cell>, usize>> {
    let mut keys = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        if keys.insert(row_key(row, columns)?, index).is_some() {
            return Err(Error::Invalid(
                "duplicate explicit key: ambiguous row identity".into(),
            ));
        }
    }
    Ok(keys)
}
pub(crate) fn validate_key_columns(columns: &[usize], width: usize) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    if columns.is_empty() || columns.iter().any(|&i| i >= width || !seen.insert(i)) {
        return Err(Error::Invalid(
            "identity requires nonempty distinct valid column indices".into(),
        ));
    }
    Ok(())
}
fn row_key(row: &[Cell], columns: &[usize]) -> Result<Vec<Cell>> {
    if columns.iter().any(|&i| row[i] == Cell::Null) {
        return Err(Error::Invalid("NULL key has ambiguous row identity".into()));
    }
    Ok(columns.iter().map(|&i| row[i].clone()).collect())
}
