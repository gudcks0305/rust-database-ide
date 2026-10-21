use dbide_ui_spike::{
    QuerySummary, ResultPage,
    workbench_model::{DocumentModel, ResultOwner},
};
use gpui::Entity;
use gpui_component::input::EditorState;
use std::sync::Arc;

#[derive(Clone)]
pub struct ResultSnapshot {
    pub pages: Vec<Arc<ResultPage>>,
    pub selected_page: usize,
    pub summary: Option<QuerySummary>,
    pub details: String,
    pub owner: Option<ResultOwner>,
}
impl Default for ResultSnapshot {
    fn default() -> Self {
        Self {
            pages: vec![],
            selected_page: 0,
            summary: None,
            details: "No query executed in this tab".into(),
            owner: None,
        }
    }
}
pub struct SqlDocument {
    pub model: DocumentModel,
    pub required_profile_id: Option<String>,
    pub session: dbide_ui_spike::console_session::ConsoleSession,
    pub editor: Entity<EditorState>,
    pub password_draft: Entity<gpui_component::input::InputState>,
    pub database_draft: Entity<gpui_component::input::InputState>,
    pub results: ResultSnapshot,
}
