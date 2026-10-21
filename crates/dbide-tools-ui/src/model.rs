use dbide_data::TabularData;
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Schema,
    Objects,
    Migration,
    Relationships,
    Explain,
    Data,
}
impl Tab {
    pub const ALL: [(Self, &'static str); 6] = [
        (Self::Schema, "Schema"),
        (Self::Objects, "Objects"),
        (Self::Migration, "Migration"),
        (Self::Relationships, "Relationships"),
        (Self::Explain, "EXPLAIN"),
        (Self::Data, "Data compare"),
    ];
}
/// Only the current operation can clear busy state or publish its result.
#[derive(Default)]
pub struct OperationState {
    revision: u64,
    active: Option<u64>,
}
impl OperationState {
    pub fn busy(&self) -> bool {
        self.active.is_some()
    }
    pub fn begin(&mut self) -> u64 {
        self.revision = self.revision.wrapping_add(1);
        self.active = Some(self.revision);
        self.revision
    }
    pub fn finish(&mut self, id: u64) -> bool {
        if self.active != Some(id) {
            return false;
        }
        self.active = None;
        true
    }
    pub fn cancel(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.active = None;
    }
}
pub fn scope(text: &str) -> Result<Vec<String>, String> {
    let names = text
        .split(',')
        .map(str::trim)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut seen = BTreeSet::new();
    if names.is_empty()
        || names.len() > 32
        || names.iter().any(|n| {
            n.is_empty()
                || n.len() > 63
                || n.contains('\0')
                || n == "information_schema"
                || n.starts_with("pg_")
                || !seen.insert(n.clone())
        })
    {
        return Err(
            "Choose 1–32 distinct user schemas, comma separated (system schemas excluded)".into(),
        );
    }
    Ok(names)
}
pub fn key_columns(text: &str, data: &TabularData) -> Result<Vec<usize>, String> {
    let mut seen = BTreeSet::new();
    let mut out = vec![];
    for token in text.split(',').map(str::trim) {
        let matches = data
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.name == token)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if matches.len() != 1 || !seen.insert(matches[0]) {
            return Err("Enter distinct, unambiguous column names separated by commas; names must match exactly".into());
        }
        out.push(matches[0]);
    }
    Ok(out)
}
pub fn er_limits() -> dbide_schema::er::ErLimits {
    dbide_schema::er::ErLimits {
        max_tables: 80,
        max_columns_per_table: 24,
        max_edges: 200,
        max_svg_bytes: 2_000_000,
    }
}
pub fn short(text: &str, max: usize) -> String {
    let mut value = text.chars().take(max).collect::<String>();
    if text.chars().count() > max {
        value.push('…');
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_results_cannot_clear_current_loading() {
        let mut s = OperationState::default();
        let old = s.begin();
        s.cancel();
        let new = s.begin();
        assert!(!s.finish(old));
        assert!(s.busy());
        assert!(s.finish(new));
        assert!(!s.busy());
        assert!(!s.finish(new));
    }
    #[test]
    fn errors_and_dialog_cancellation_can_finish_current_operation() {
        let mut s = OperationState::default();
        let id = s.begin();
        assert!(s.finish(id));
        assert!(!s.busy());
        let id = s.begin();
        assert!(s.finish(id));
    }
    #[test]
    fn scope_is_explicit_bounded_and_non_system() {
        assert_eq!(scope("public, sales").unwrap(), vec!["public", "sales"]);
        for input in [
            "",
            "public,public",
            "pg_catalog",
            "information_schema",
            "public,",
        ] {
            assert!(scope(input).is_err(), "{input}");
        }
    }
    #[test]
    fn keys_require_exact_unique_names() {
        let data = TabularData {
            columns: vec![
                dbide_data::Column { name: "id".into() },
                dbide_data::Column {
                    name: "code".into(),
                },
            ],
            rows: vec![],
        };
        assert_eq!(key_columns("code,id", &data).unwrap(), vec![1, 0]);
        for input in ["", "id,id", "missing", "0"] {
            assert!(key_columns(input, &data).is_err());
        }
        let dup = TabularData {
            columns: vec![dbide_data::Column { name: "id".into() }; 2],
            rows: vec![],
        };
        assert!(key_columns("id", &dup).is_err());
    }
    #[test]
    fn duplicate_identity_is_visible_as_error() {
        let data = TabularData {
            columns: vec![dbide_data::Column { name: "id".into() }],
            rows: vec![vec![dbide_data::Cell::Integer(1)]; 2],
        };
        let error = dbide_data::diff_by_key(&data, &data, &[0], Default::default()).unwrap_err();
        assert!(error.to_string().contains("duplicate explicit key"));
    }
}

/// Engine-aware target formatting never exposes credentials.
pub fn target_label(profile: &dbide_profiles::ConnectionProfile) -> String {
    use dbide_profiles::DatabaseEngine;
    match profile.engine {
        DatabaseEngine::Sqlite => format!(
            "{} · SQLite · {}",
            profile.name,
            profile.sqlite_path.as_deref().unwrap_or("No file selected")
        ),
        DatabaseEngine::PostgreSql => format!(
            "{} · PostgreSQL · {}:{} / {}",
            profile.name, profile.host, profile.port, profile.database
        ),
        DatabaseEngine::MySql => format!(
            "{} · MySQL · {}:{} / {}",
            profile.name, profile.host, profile.port, profile.database
        ),
    }
}
#[cfg(test)]
mod target_tests {
    #[test]
    fn sqlite_label_is_file_target_not_pg_host() {
        let profile = dbide_profiles::ConnectionProfile {
            engine: dbide_profiles::DatabaseEngine::Sqlite,
            sqlite_path: Some("/owned/fixture.sqlite".into()),
            host: "wrong-host".into(),
            ..Default::default()
        };
        let label = super::target_label(&profile);
        assert!(label.contains("/owned/fixture.sqlite"));
        assert!(label.contains("SQLite"));
        assert!(!label.contains("wrong-host"));
    }
}
