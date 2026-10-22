//! Read-only history projection. Captured targets never resolve through current profiles.
use dbide_profiles::HistoryEntry;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct HistoryModel {
    pub entries: Vec<HistoryEntry>,
    pub profile_names: BTreeMap<String, String>,
    pub search: String,
    pub profile: Option<String>,
    pub outcome: Option<String>,
    pub visible: Vec<usize>,
    selected: Option<HistoryEntry>,
}
impl HistoryModel {
    pub fn update(&mut self, entries: Vec<HistoryEntry>, names: BTreeMap<String, String>) {
        self.entries = entries;
        self.profile_names = names;
        self.refresh();
    }
    pub fn refresh(&mut self) {
        let query = self.search.trim().to_lowercase();
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let name = self
                    .profile_names
                    .get(&entry.profile_id)
                    .map(String::as_str)
                    .unwrap_or_default();
                let database = captured_database(entry).unwrap_or_default();
                (self
                    .profile
                    .as_ref()
                    .is_none_or(|id| *id == entry.profile_id)
                    && self
                        .outcome
                        .as_ref()
                        .is_none_or(|outcome| *outcome == entry.outcome)
                    && (query.is_empty()
                        || [&entry.sql, &entry.profile_id, name, database]
                            .into_iter()
                            .any(|value| value.to_lowercase().contains(&query))))
                .then_some(index)
            })
            .collect();
        // Preserve reverse insertion order for entries captured in the same millisecond.
        self.visible.sort_by(|a, b| {
            self.entries[*b]
                .executed_at_unix_ms
                .cmp(&self.entries[*a].executed_at_unix_ms)
                .then_with(|| b.cmp(a))
        });
        if !self
            .visible
            .iter()
            .any(|index| self.selected.as_ref() == Some(&self.entries[*index]))
        {
            self.selected = self
                .visible
                .first()
                .map(|index| self.entries[*index].clone());
        }
    }
    pub fn selected(&self) -> Option<&HistoryEntry> {
        self.selected.as_ref()
    }
    pub fn select(&mut self, index: usize) {
        if self.visible.contains(&index) {
            self.selected = Some(self.entries[index].clone());
        }
    }
    pub fn move_selection(&mut self, down: bool) {
        if self.visible.is_empty() {
            return;
        }
        let current = self
            .visible
            .iter()
            .position(|index| self.selected.as_ref() == Some(&self.entries[*index]))
            .unwrap_or(0);
        let next = if down {
            (current + 1).min(self.visible.len() - 1)
        } else {
            current.saturating_sub(1)
        };
        self.select(self.visible[next]);
    }
    pub fn profiles(&self) -> BTreeMap<String, String> {
        self.entries
            .iter()
            .map(|entry| {
                (
                    entry.profile_id.clone(),
                    self.profile_names
                        .get(&entry.profile_id)
                        .cloned()
                        .unwrap_or_else(|| entry.profile_id.clone()),
                )
            })
            .collect()
    }
}
pub fn captured_database(entry: &HistoryEntry) -> Option<&str> {
    entry
        .execution_target
        .as_ref()
        .map(|target| target.database.as_str())
}
pub fn sql_excerpt(sql: &str, limit: usize) -> String {
    let mut chars = sql.chars();
    let mut excerpt: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        excerpt.push('…');
    }
    excerpt
}
/// UTC display independent of machine locale and timezone; SQL remains untouched.
pub fn timestamp(ms: u64) -> String {
    let seconds = ms / 1000;
    let days = (seconds / 86400) as i64;
    let z = days + 719468;
    let era = z / 146097;
    let day_of_era = z - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        seconds % 86400 / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbide_profiles::{ConnectionProfile, StoredExecutionTarget};
    fn entry(sql: &str, time: u64, profile: &str, outcome: &str) -> HistoryEntry {
        HistoryEntry {
            profile_id: profile.into(),
            sql: sql.into(),
            executed_at_unix_ms: time,
            outcome: outcome.into(),
            row_count: Some(3),
            execution_target: None,
        }
    }
    fn model() -> HistoryModel {
        let mut model = HistoryModel::default();
        model.update(
            vec![
                entry("SELECT '서울 ÄBC';\n", 2000, "p1", "Completed"),
                entry("select 2", 1000, "removed", "Failed"),
                entry("select 3", 3000, "p1", "Cancelled"),
            ],
            BTreeMap::from([("p1".into(), "개발 DB".into())]),
        );
        model
    }
    #[test]
    fn unicode_search_profile_and_exact_outcome_scope() {
        let mut model = model();
        model.search = "äbc".into();
        model.refresh();
        assert_eq!(model.visible, vec![0]);
        model.search = "서울".into();
        model.refresh();
        assert_eq!(model.visible, vec![0]);
        model.search = "개발 db".into();
        model.refresh();
        assert_eq!(model.visible, vec![2, 0]);
        model.profile = Some("p1".into());
        model.outcome = Some("Completed".into());
        model.refresh();
        assert_eq!(model.visible, vec![0]);
        model.outcome = Some("Failed".into());
        model.refresh();
        assert!(model.visible.is_empty());
        model.search.clear();
        model.profile = Some("removed".into());
        model.refresh();
        assert_eq!(model.visible, vec![1]);
        assert!(model.profiles().contains_key("removed"));
    }
    #[test]
    fn newest_order_and_stable_selection_survive_snapshot() {
        let mut model = model();
        assert_eq!(model.visible, vec![2, 0, 1]);
        model.select(0);
        let selected = model.selected().cloned();
        let mut entries = model.entries.clone();
        entries.reverse();
        model.update(entries, BTreeMap::new());
        assert_eq!(model.selected(), selected.as_ref());
        model.move_selection(true);
        assert_eq!(model.selected().unwrap().executed_at_unix_ms, 1000);
        model.update(vec![], BTreeMap::new());
        assert!(model.selected().is_none());
        assert!(model.visible.is_empty());
    }
    #[test]
    fn captured_database_and_raw_sql_survive_profile_changes() {
        let profile = ConnectionProfile::default();
        let mut original = entry(
            "  SELECT '한글';\n-- Tail\n",
            0,
            &profile.id,
            "OutcomeUnknown",
        );
        original.execution_target =
            Some(StoredExecutionTarget::capture(&profile, "captured_db").unwrap());
        let mut model = HistoryModel::default();
        model.update(
            vec![original.clone()],
            BTreeMap::from([(profile.id, "renamed".into())]),
        );
        model.search = "CAPTURED_DB".into();
        model.refresh();
        assert_eq!(model.visible.len(), 1);
        assert_eq!(
            captured_database(model.selected().unwrap()),
            Some("captured_db")
        );
        assert_eq!(model.selected().unwrap().sql, original.sql);
        assert_eq!(sql_excerpt(&original.sql, 3), "  S…");
        model.outcome = Some("Rejected".into());
        model.refresh();
        assert!(model.visible.is_empty());
    }
    #[test]
    fn utc_timestamp_handles_epoch_leap_day_and_milliseconds() {
        assert_eq!(timestamp(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(timestamp(1_709_251_199_999), "2024-02-29 23:59:59 UTC");
    }
}
