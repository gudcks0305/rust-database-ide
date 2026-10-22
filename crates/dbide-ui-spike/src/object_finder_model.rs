//! Generation gate shared by the saved-object finder UI and isolated tests.
use dbide_catalog_store::{SearchMatch, SearchPage};
#[derive(Default)]
pub struct FinderModel {
    pub generation: u64,
    pub page: Option<SearchPage>,
    pub selected: usize,
    pub error: Option<String>,
    pub searching: bool,
}
impl FinderModel {
    pub fn begin(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.page = None;
        self.selected = 0;
        self.error = None;
        self.searching = true;
        self.generation
    }
    pub fn complete(&mut self, generation: u64, result: Result<SearchPage, String>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.searching = false;
        self.page = None;
        self.error = None;
        match result {
            Ok(page) => self.page = Some(page),
            Err(error) => self.error = Some(error),
        }
        true
    }
    pub fn move_selection(&mut self, down: bool) {
        let length = self.page.as_ref().map_or(0, |page| page.matches.len());
        self.selected = if length == 0 {
            0
        } else if down {
            (self.selected + 1).min(length - 1)
        } else {
            self.selected.saturating_sub(1)
        };
    }
    pub fn selection(&self) -> Option<&SearchMatch> {
        if self.searching {
            return None;
        }
        self.page.as_ref()?.matches.get(self.selected)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use dbide_catalog_store::{Relation, SearchProfile};
    fn page() -> SearchPage {
        SearchPage {
            matches: vec![SearchMatch {
                profile: SearchProfile {
                    profile_id: "p".into(),
                    fingerprint: "f".into(),
                    profile_name: "fixture".into(),
                    engine: "SQLite".into(),
                    database: None,
                },
                database: "main".into(),
                relation: Relation {
                    schema: "main".into(),
                    name: "cached".into(),
                    kind: "table".into(),
                },
                captured_unix_ms: 1,
            }],
            ..Default::default()
        }
    }
    #[test]
    fn query_change_immediately_invalidates_selection_and_late_response() {
        let mut model = FinderModel::default();
        let old = model.begin();
        assert!(model.complete(old, Ok(page())));
        assert!(model.selection().is_some());
        let current = model.begin();
        assert!(model.selection().is_none());
        assert!(!model.complete(old, Ok(page())));
        assert!(model.searching);
        assert!(model.selection().is_none());
        assert!(model.complete(current, Ok(page())));
        assert!(model.selection().is_some());
    }
    #[test]
    fn selection_clamps_empty_and_edges_and_stale_errors_are_ignored() {
        let mut model = FinderModel::default();
        model.move_selection(true);
        assert_eq!(model.selected, 0);
        let old = model.begin();
        let current = model.begin();
        assert!(!model.complete(old, Err("old".into())));
        assert!(model.error.is_none());
        model.complete(current, Ok(page()));
        model.move_selection(true);
        assert_eq!(model.selected, 0);
        model.move_selection(false);
        assert_eq!(model.selected, 0);
        let current = model.begin();
        model.complete(current, Err("cache unavailable".into()));
        assert!(model.selection().is_none());
        assert_eq!(model.error.as_deref(), Some("cache unavailable"));
    }
}
