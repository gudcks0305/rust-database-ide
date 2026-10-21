//! Pure retention and dispatch rules for the single table-reader connection.
use dbide_bridge::OperationId;
use dbide_ui_spike::navigator::{Key, Relation};

pub(crate) const MAX_TABLE_TABS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableIdentity {
    profile: String,
    endpoint: String,
    database: String,
    relation: Relation,
}
impl TableIdentity {
    pub(crate) fn capture(key: &Key, relation: &Relation) -> Self {
        Self {
            profile: key.profile_id.clone(),
            endpoint: key.fingerprint.clone(),
            database: key.database.clone(),
            relation: relation.clone(),
        }
    }
}

pub(crate) fn can_add(count: usize) -> bool {
    count < MAX_TABLE_TABS
}

/// The visual selection is deliberately absent: a report belongs to its
/// captured operation/editor pair even after the user selects another tab.
pub(crate) fn accepts_report<T: Copy + Eq>(
    captured_operation: OperationId,
    active_operation: Option<OperationId>,
    captured_owner: Option<T>,
    existing_owner: Option<T>,
) -> bool {
    active_operation == Some(captured_operation)
        && captured_owner.is_some()
        && captured_owner == existing_owner
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbide_ui_spike::navigator::Scope;

    fn identity() -> (Key, Relation) {
        let relation = Relation {
            schema: "public".into(),
            name: "items".into(),
            kind: "table".into(),
        };
        (
            Key {
                profile_id: "profile-a".into(),
                fingerprint: "endpoint-a".into(),
                generation: 1,
                database: "db-a".into(),
                scope: Scope::Columns(relation.clone()),
            },
            relation,
        )
    }

    #[test]
    fn dedup_ignores_navigator_generation_but_retains_exact_target() {
        let (key, relation) = identity();
        let expected = TableIdentity::capture(&key, &relation);
        let mut refreshed = key.clone();
        refreshed.generation += 1;
        assert_eq!(expected, TableIdentity::capture(&refreshed, &relation));
        for field in ["profile", "endpoint", "database"] {
            let mut changed = key.clone();
            match field {
                "profile" => changed.profile_id.push('x'),
                "endpoint" => changed.fingerprint.push('x'),
                _ => changed.database.push('x'),
            }
            assert_ne!(expected, TableIdentity::capture(&changed, &relation));
        }
        let mut changed = relation.clone();
        changed.schema = "private".into();
        assert_ne!(expected, TableIdentity::capture(&key, &changed));
        changed = relation;
        changed.name = "other".into();
        assert_ne!(expected, TableIdentity::capture(&key, &changed));
    }

    #[test]
    fn retention_is_bounded_without_evicting_a_busy_owner() {
        assert!(can_add(MAX_TABLE_TABS - 1));
        assert!(!can_add(MAX_TABLE_TABS));
        assert!(!can_add(usize::MAX));
    }

    #[test]
    fn selection_stale_operation_and_closed_owner_never_redirect_reports() {
        assert!(accepts_report(
            OperationId(7),
            Some(OperationId(7)),
            Some(1),
            Some(1)
        ));
        assert!(!accepts_report(
            OperationId(7),
            Some(OperationId(8)),
            Some(1),
            Some(1)
        ));
        assert!(!accepts_report(
            OperationId(7),
            Some(OperationId(7)),
            Some(1),
            Some(2)
        ));
        assert!(!accepts_report(
            OperationId(7),
            Some(OperationId(7)),
            Some(1),
            None
        ));
        assert!(!accepts_report::<u64>(
            OperationId(7),
            Some(OperationId(7)),
            None,
            None
        ));
    }
}
