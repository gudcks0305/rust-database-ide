use dbide_assistant::{
    AssistantClient, AssistantConfig, AssistantContext, AssistantTask, MAX_INSTRUCTION_BYTES,
    MAX_SCHEMA_BYTES, MAX_SQL_BYTES, PayloadPreview,
};
use dbide_profiles::{ConnectionProfile, DatabaseEngine};
use sqlparser::{
    dialect::{GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect},
    parser::Parser,
};

pub const MAX_DISPLAY_BYTES: usize = 1024 * 1024;
#[derive(Clone)]
pub struct ReviewedRequest {
    pub config: AssistantConfig,
    pub task: AssistantTask,
    pub context: AssistantContext,
    pub summary: PayloadPreview,
}
impl ReviewedRequest {
    pub fn new(
        config: AssistantConfig,
        task: AssistantTask,
        instruction: String,
        captured_sql: Option<&str>,
        captured_schema: Option<&str>,
        include_sql: bool,
        include_schema: bool,
    ) -> Result<Self, String> {
        if !config.enabled {
            return Err("Enable Codex assistance before preparing a request".into());
        }
        if instruction.len() > MAX_INSTRUCTION_BYTES {
            return Err("Instruction exceeds 64 KiB".into());
        }
        let sql = selected(captured_sql, include_sql, MAX_SQL_BYTES, "SQL")?;
        let schema = selected(captured_schema, include_schema, MAX_SCHEMA_BYTES, "schema")?;
        let context = AssistantContext {
            instruction,
            sql,
            schema,
        };
        let client = AssistantClient::new(config.clone()).map_err(|e| e.to_string())?;
        let summary = client
            .build_preview(task, &context)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            config,
            task,
            context,
            summary,
        })
    }
}
fn selected(
    value: Option<&str>,
    include: bool,
    max: usize,
    name: &str,
) -> Result<Option<String>, String> {
    if !include {
        return Ok(None);
    }
    let value = value.filter(|v| !v.trim().is_empty()).ok_or_else(|| {
        format!("No captured {name} available; choose another task or reopen with context")
    })?;
    if value.len() > max {
        return Err(format!("Captured {name} exceeds its byte limit"));
    }
    Ok(Some(value.into()))
}
/// Syntactic validation only. It does not establish correctness, safety or permission.
pub fn sql_draft(sql: Option<&str>, profile: Option<&ConnectionProfile>) -> Result<String, String> {
    let sql = sql.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
        "Response has no structured SQL suggestion; text remains available to copy".to_string()
    })?;
    if sql.len() > MAX_SQL_BYTES {
        return Err("SQL suggestion exceeds 256 KiB".into());
    }
    let dialect: &dyn sqlparser::dialect::Dialect = match profile.map(|p| p.engine) {
        Some(DatabaseEngine::PostgreSql) => &PostgreSqlDialect {},
        Some(DatabaseEngine::Sqlite) => &SQLiteDialect {},
        Some(DatabaseEngine::MySql) => &MySqlDialect {},
        None => &GenericDialect {},
    };
    let statements = Parser::parse_sql(dialect, sql).map_err(|_| "SQL suggestion did not parse for the captured dialect; copy response text for manual review".to_string())?;
    if statements.is_empty() {
        return Err("SQL suggestion contains no complete statement".into());
    }
    Ok(sql.into())
}
#[derive(Default)]
pub struct Generation {
    value: u64,
    busy: bool,
}
impl Generation {
    pub fn begin(&mut self) -> Option<u64> {
        if self.busy {
            return None;
        }
        self.value = self.value.wrapping_add(1);
        self.busy = true;
        Some(self.value)
    }
    pub fn accepts(&self, id: u64) -> bool {
        self.busy && self.value == id
    }
    pub fn finish(&mut self, id: u64) -> bool {
        if !self.accepts(id) {
            return false;
        }
        self.busy = false;
        true
    }
    pub fn cancel(&mut self) {
        self.value = self.value.wrapping_add(1);
        self.busy = false;
    }
    pub fn busy(&self) -> bool {
        self.busy
    }
}
pub fn append_bounded(target: &mut String, incoming: &str) {
    let mut end = incoming
        .len()
        .min(MAX_DISPLAY_BYTES.saturating_sub(target.len()));
    while !incoming.is_char_boundary(end) {
        end -= 1;
    }
    target.push_str(&incoming[..end]);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> AssistantConfig {
        let mut c = AssistantConfig::codex("ws://127.0.0.1:45457", "/isolated/scratch");
        c.enabled = true;
        c
    }
    #[test]
    fn excluded_context_never_enters_outbound_snapshot() {
        let request = ReviewedRequest::new(
            config(),
            AssistantTask::GenerateSql,
            "Count records".into(),
            Some("SECRET SQL"),
            Some("SECRET SCHEMA"),
            false,
            false,
        )
        .unwrap();
        assert!(request.context.sql.is_none());
        assert!(request.context.schema.is_none());
        assert!(request.summary.sql_bytes.is_none());
        assert!(request.summary.schema_bytes.is_none());
        let payload = serde_json::to_string(&request.context).unwrap();
        assert!(!payload.contains("SECRET"));
    }
    #[test]
    fn explicit_include_preview_and_task_requirements() {
        assert!(
            ReviewedRequest::new(
                config(),
                AssistantTask::Explain,
                String::new(),
                Some("SELECT 1"),
                None,
                false,
                false
            )
            .is_err()
        );
        let request = ReviewedRequest::new(
            config(),
            AssistantTask::Explain,
            String::new(),
            Some("SELECT 1"),
            Some("t(id int)"),
            true,
            true,
        )
        .unwrap();
        assert_eq!(request.summary.sql_bytes, Some(8));
        assert_eq!(request.summary.schema_bytes, Some(9));
        assert!(
            ReviewedRequest::new(
                config(),
                AssistantTask::GenerateSql,
                "x".into(),
                None,
                None,
                true,
                false
            )
            .is_err()
        );
        let mut disabled = config();
        disabled.enabled = false;
        assert!(
            ReviewedRequest::new(
                disabled,
                AssistantTask::GenerateSql,
                "x".into(),
                None,
                None,
                false,
                false
            )
            .is_err()
        );
    }
    #[test]
    fn no_network_preview_and_size_limits() {
        assert!(
            ReviewedRequest::new(
                config(),
                AssistantTask::GenerateSql,
                "x".repeat(MAX_INSTRUCTION_BYTES + 1),
                None,
                None,
                false,
                false
            )
            .is_err()
        );
        assert!(
            ReviewedRequest::new(
                config(),
                AssistantTask::GenerateSql,
                "x".into(),
                Some(&"x".repeat(MAX_SQL_BYTES + 1)),
                None,
                true,
                false
            )
            .is_err()
        );
        let mut remote = config();
        remote.endpoint = "ws://example.com:45457".into();
        assert!(
            ReviewedRequest::new(
                remote,
                AssistantTask::GenerateSql,
                "x".into(),
                None,
                None,
                false,
                false
            )
            .is_err()
        );
    }
    #[test]
    fn sql_is_structured_validated_and_remains_untrusted() {
        assert_eq!(sql_draft(Some("SELECT 1;"), None).unwrap(), "SELECT 1;");
        assert!(sql_draft(None, None).is_err());
        assert!(sql_draft(Some("```sql\nSELECT 1;\n```"), None).is_err());
        assert!(sql_draft(Some("-- comment only"), None).is_err());
        // Parser success is not an execution/safety decision; destructive text is still just a draft.
        assert!(sql_draft(Some("DROP TABLE users;"), None).is_ok());
    }
    #[test]
    fn cancelled_and_stale_results_cannot_publish() {
        let mut generation = Generation::default();
        let old = generation.begin().unwrap();
        assert!(generation.begin().is_none());
        generation.cancel();
        let current = generation.begin().unwrap();
        assert!(!generation.accepts(old));
        assert!(!generation.finish(old));
        assert!(generation.finish(current));
        assert!(!generation.finish(current));
    }
    #[test]
    fn streamed_display_is_utf8_bounded() {
        let mut result = "x".repeat(MAX_DISPLAY_BYTES - 1);
        append_bounded(&mut result, "한글");
        assert_eq!(result.len(), MAX_DISPLAY_BYTES - 1);
        append_bounded(&mut result, "a");
        append_bounded(&mut result, "more");
        assert_eq!(result.len(), MAX_DISPLAY_BYTES);
    }
}
