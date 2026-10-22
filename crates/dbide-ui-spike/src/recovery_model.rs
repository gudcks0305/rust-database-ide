//! Editor recovery restores buffers and file metadata only, with the exact dirty baseline.
use crate::workbench_model::DocumentModel;
use dbide_workspace::DocumentSnapshot;
pub fn restore_document(snapshot: DocumentSnapshot) -> (DocumentModel, String) {
    (
        DocumentModel {
            id: snapshot.id,
            title: snapshot.title,
            revision: snapshot.revision,
            saved_source: snapshot.saved_sql,
            path: snapshot.path,
        },
        snapshot.sql,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_preserves_unsaved_multiline_and_maximum_id_without_reading_file() {
        let snapshot = DocumentSnapshot {
            project_root: None,
            id: u64::MAX,
            title: "draft.sql".into(),
            sql: "SELECT 'new\\nvalue';\r\n".into(),
            saved_sql: "SELECT 'old';\r\n".into(),
            revision: 9,
            path: Some("/nonexistent/original.sql".into()),
            profile_id: Some("captured-only".into()),
            execution_target: None,
        };
        let (model, sql) = restore_document(snapshot.clone());
        assert_eq!(model.id, u64::MAX);
        assert_eq!(model.path, snapshot.path);
        assert_eq!(model.saved_source, snapshot.saved_sql);
        assert_eq!(sql, snapshot.sql);
        assert!(model.dirty(&sql));
    }
    #[test]
    fn clean_file_baseline_stays_clean_without_reopening_original() {
        let sql = "-- 한글\r\nSELECT 1;";
        let (model, restored) = restore_document(DocumentSnapshot {
            project_root: None,
            id: 1,
            title: "query.sql".into(),
            sql: sql.into(),
            saved_sql: sql.into(),
            revision: 0,
            path: Some("/missing/query.sql".into()),
            profile_id: None,
            execution_target: None,
        });
        assert!(!model.dirty(&restored));
    }
}
