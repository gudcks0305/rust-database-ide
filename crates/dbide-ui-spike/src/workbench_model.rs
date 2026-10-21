//! UI-independent identity and revision guards used by document and result views.
#[derive(Clone, Debug)]
pub struct DocumentModel {
    pub id: u64,
    pub title: String,
    pub revision: u64,
    pub saved_source: String,
    pub path: Option<std::path::PathBuf>,
}
impl DocumentModel {
    pub fn new(id: u64, title: String, source: String) -> Self {
        Self {
            id,
            title,
            revision: 0,
            saved_source: source,
            path: None,
        }
    }
    pub fn changed(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
    pub fn dirty(&self, source: &str) -> bool {
        self.saved_source != source
    }
    pub fn saved(&mut self, source: String, title: String) {
        self.saved_source = source;
        self.title = title;
    }
}
#[derive(Clone, Debug)]
pub struct ResultOwner {
    pub document_id: u64,
    pub document_title: String,
    pub profile_id: String,
    pub profile_name: String,
    pub target: String,
}
impl ResultOwner {
    pub fn belongs_to(&self, document_id: u64) -> bool {
        self.document_id == document_id
    }
    pub fn label(&self) -> String {
        format!(
            "{} [tab {}] · {} · {}",
            self.document_title, self.document_id, self.profile_name, self.target
        )
    }
}
pub struct OwnedRenamePreview {
    pub document_id: u64,
    pub preview: dbide_sql::RenamePreview,
}
impl OwnedRenamePreview {
    pub fn apply(
        &self,
        document_id: u64,
        sql: &str,
        revision: u64,
    ) -> Result<String, dbide_sql::SqlError> {
        if self.document_id != document_id {
            return Err(dbide_sql::SqlError(
                "Rename preview belongs to another document".into(),
            ));
        }
        self.preview.apply(sql, revision)
    }
}
/// Inject the credential store so tests exercise the explicit remember choice
/// without ever invoking native Keychain services.
pub fn remember_password(
    store: &dyn dbide_credentials::CredentialStore,
    profile_id: &str,
    password: &str,
    remember: bool,
) -> Result<(), dbide_credentials::CredentialError> {
    if remember {
        store.save(
            profile_id,
            &dbide_credentials::Secret::new(password.as_bytes().to_vec()),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbide_credentials::{CredentialStore, InMemoryCredentialStore};
    #[test]
    fn save_completion_uses_exact_snapshot_and_does_not_hide_later_edits() {
        let mut doc = DocumentModel::new(1, "query.sql".into(), "SELECT 1".into());
        doc.changed();
        doc.saved("SELECT 2".into(), "saved.sql".into());
        assert!(doc.dirty("SELECT 3"));
        assert!(!doc.dirty("SELECT 2"));
    }
    #[test]
    fn equal_titles_do_not_make_results_belong_to_other_tabs() {
        let owner = ResultOwner {
            document_id: 1,
            document_title: "untitled.sql".into(),
            profile_id: "p".into(),
            profile_name: "Local".into(),
            target: "localhost:5432/db".into(),
        };
        assert!(owner.belongs_to(1));
        assert!(!owner.belongs_to(2));
    }
    #[test]
    fn rename_requires_same_document_revision_and_source_hash() {
        let sql = "WITH sample AS (SELECT 1 AS id) SELECT sample.id FROM sample";
        let cursor = sql.find("sample").unwrap();
        let preview = dbide_sql::rename(
            sql,
            cursor,
            "renamed",
            dbide_sql::Dialect::PostgreSql,
            &dbide_sql::Catalog::default(),
            4,
        )
        .unwrap();
        let preview = OwnedRenamePreview {
            document_id: 1,
            preview,
        };
        assert!(preview.apply(1, sql, 4).is_ok());
        assert!(preview.apply(2, sql, 4).is_err());
        assert!(preview.apply(1, sql, 5).is_err());
        assert!(
            preview
                .apply(1, &sql.replace("SELECT 1", "SELECT 2"), 4)
                .is_err()
        );
    }
    #[test]
    fn unchecked_remember_never_writes_or_removes_existing_secret() {
        let store = InMemoryCredentialStore::default();
        remember_password(&store, "one", "session-only", false).unwrap();
        assert!(store.load("one").unwrap().is_none());
        remember_password(&store, "one", "remembered", true).unwrap();
        remember_password(&store, "one", "new-session", false).unwrap();
        assert_eq!(
            store.load("one").unwrap().unwrap().expose_str().unwrap(),
            "remembered"
        );
        assert!(store.load("two").unwrap().is_none());
    }
}

/// Keep the old active identity until the exact worker acknowledgement arrives.
pub fn complete_configuration<T>(
    expected: &mut Option<dbide_bridge::OperationId>,
    pending: &mut Option<T>,
    active: &mut Option<T>,
    received: dbide_bridge::OperationId,
) -> bool {
    if *expected != Some(received) || pending.is_none() {
        return false;
    }
    *active = pending.take();
    *expected = None;
    true
}
#[cfg(test)]
mod configuration_tests {
    use super::*;
    use dbide_bridge::OperationId;
    #[test]
    fn pending_profile_never_becomes_active_from_a_stale_ack() {
        let mut active = Some("database-A");
        let mut pending = Some("database-B");
        let mut operation = Some(OperationId(8));
        assert!(!complete_configuration(
            &mut operation,
            &mut pending,
            &mut active,
            OperationId(7)
        ));
        assert_eq!(active, Some("database-A"));
        assert_eq!(pending, Some("database-B"));
        assert!(complete_configuration(
            &mut operation,
            &mut pending,
            &mut active,
            OperationId(8)
        ));
        assert_eq!(active, Some("database-B"));
        assert!(pending.is_none());
        assert!(operation.is_none());
    }
}

pub fn engine_name(engine: dbide_profiles::DatabaseEngine) -> &'static str {
    match engine {
        dbide_profiles::DatabaseEngine::PostgreSql => "PostgreSQL",
        dbide_profiles::DatabaseEngine::Sqlite => "SQLite",
        dbide_profiles::DatabaseEngine::MySql => "MySQL",
    }
}
pub fn engine_dialect(engine: dbide_profiles::DatabaseEngine) -> dbide_sql::Dialect {
    match engine {
        dbide_profiles::DatabaseEngine::PostgreSql => dbide_sql::Dialect::PostgreSql,
        dbide_profiles::DatabaseEngine::Sqlite => dbide_sql::Dialect::SQLite,
        dbide_profiles::DatabaseEngine::MySql => dbide_sql::Dialect::MySql,
    }
}
pub fn profile_target(profile: &dbide_profiles::ConnectionProfile) -> String {
    if profile.engine == dbide_profiles::DatabaseEngine::Sqlite {
        format!(
            "SQLite · {}",
            profile
                .sqlite_path
                .as_deref()
                .unwrap_or("<no file selected>")
        )
    } else {
        format!(
            "{} · {}:{}/{}",
            engine_name(profile.engine),
            profile.host,
            profile.port,
            profile.database
        )
    }
}
pub fn connection_target(target: &dbide_bridge::ConnectionTarget) -> String {
    if target.port == 0 && target.host == "SQLite" {
        format!("SQLite · {}", target.database)
    } else {
        format!("{}:{}/{}", target.host, target.port, target.database)
    }
}
pub fn table_query(engine: dbide_profiles::DatabaseEngine, schema: &str, table: &str) -> String {
    let quote = if engine == dbide_profiles::DatabaseEngine::MySql {
        '`'
    } else {
        '"'
    };
    let ident = |name: &str| {
        format!(
            "{quote}{}{quote}",
            name.replace(quote, &format!("{quote}{quote}"))
        )
    };
    format!(
        "SELECT * FROM {}.{} LIMIT 100;",
        ident(schema),
        ident(table)
    )
}
/// Tracks user edits separately from programmatic engine-default changes.
/// Switching through SQLite retains the last pristine network default.
#[derive(Debug)]
pub struct PortDraft {
    pristine: bool,
    default_value: String,
    expected_change: Option<String>,
}
impl PortDraft {
    pub fn new(engine: dbide_profiles::DatabaseEngine, value: &str) -> Self {
        let default = match engine {
            dbide_profiles::DatabaseEngine::MySql => "3306",
            _ => "5432",
        };
        Self {
            pristine: value == default,
            default_value: default.into(),
            expected_change: None,
        }
    }
    pub fn observe(&mut self, value: &str) {
        if self.expected_change.as_deref() == Some(value) {
            self.expected_change = None;
        } else {
            self.pristine = false;
        }
    }
    pub fn switch(
        &mut self,
        engine: dbide_profiles::DatabaseEngine,
        current: &str,
    ) -> Option<String> {
        if engine == dbide_profiles::DatabaseEngine::Sqlite
            || !self.pristine
            || current != self.default_value
        {
            return None;
        }
        let next = if engine == dbide_profiles::DatabaseEngine::MySql {
            "3306"
        } else {
            "5432"
        };
        self.default_value = next.into();
        if current == next {
            return None;
        }
        self.expected_change = Some(next.into());
        Some(next.into())
    }
}
/// Pure form projection: hidden SQLite authentication/network fields cannot
/// reject a valid file profile or accidentally attach a saved password.
pub fn request_from_fields(
    original: &dbide_profiles::ConnectionProfile,
    engine: dbide_profiles::DatabaseEngine,
    fields: &[String],
    tls: dbide_profiles::TlsMode,
    create: bool,
    read_only: bool,
) -> Result<dbide_profiles::ConnectionRequest, String> {
    if fields.len() < 9 {
        return Err("Incomplete connection form".into());
    }
    let mut profile = original.clone();
    profile.engine = engine;
    profile.name = fields[0].clone();
    profile.read_only = read_only;
    profile.connect_timeout_secs = fields[6]
        .parse()
        .map_err(|_| "Timeout must be a whole number from 1 to 300 seconds".to_string())?;
    let password = if engine == dbide_profiles::DatabaseEngine::Sqlite {
        profile.sqlite_path = Some(fields[8].clone());
        profile.sqlite_create = create && !read_only;
        String::new()
    } else {
        profile.host = fields[1].clone();
        profile.port = fields[2]
            .parse::<u16>()
            .map_err(|_| "Port must be a number between 1 and 65535".to_string())?;
        profile.database = fields[3].clone();
        profile.username = fields[4].clone();
        profile.tls_mode = tls;
        profile.root_cert_path = (!fields[7].is_empty()).then(|| fields[7].clone());
        profile.sqlite_path = None;
        profile.sqlite_create = false;
        fields[5].clone()
    };
    profile.validate().map_err(|e| e.to_string())?;
    Ok(dbide_profiles::ConnectionRequest { profile, password })
}
#[cfg(test)]
mod engine_tests {
    use super::*;
    use dbide_profiles::{ConnectionProfile, DatabaseEngine as E, TlsMode};
    #[test]
    fn pristine_ports_follow_engine_defaults_but_user_edits_never_do() {
        let mut draft = PortDraft::new(E::PostgreSql, "5432");
        assert_eq!(draft.switch(E::Sqlite, "5432"), None);
        assert_eq!(draft.switch(E::MySql, "5432"), Some("3306".into()));
        draft.observe("3306");
        assert_eq!(draft.switch(E::PostgreSql, "3306"), Some("5432".into()));
        draft.observe("5432");
        draft.observe("6543");
        draft.observe("5432");
        assert_eq!(draft.switch(E::MySql, "5432"), None);
        assert_eq!(
            PortDraft::new(E::PostgreSql, "55439").switch(E::MySql, "55439"),
            None
        );
    }
    #[test]
    fn dialect_and_identifier_quoting_follow_actual_engine() {
        assert_eq!(
            engine_dialect(E::PostgreSql),
            dbide_sql::Dialect::PostgreSql
        );
        assert_eq!(engine_dialect(E::Sqlite), dbide_sql::Dialect::SQLite);
        assert_eq!(engine_dialect(E::MySql), dbide_sql::Dialect::MySql);
        assert_eq!(
            table_query(E::MySql, "a`b", "t`x"),
            "SELECT * FROM `a``b`.`t``x` LIMIT 100;"
        );
        assert_eq!(
            table_query(E::Sqlite, "main", "t\"x"),
            "SELECT * FROM \"main\".\"t\"\"x\" LIMIT 100;"
        );
    }
    #[test]
    fn sqlite_projection_ignores_hidden_invalid_network_fields_and_password() {
        let original = ConnectionProfile::new();
        let fields = [
            "Local file",
            "",
            "bad-port",
            "",
            "",
            "secret",
            "10",
            "",
            "/tmp/dbide-fixture.sqlite",
        ]
        .map(String::from);
        let request = request_from_fields(
            &original,
            E::Sqlite,
            &fields,
            TlsMode::VerifyFull,
            false,
            true,
        )
        .unwrap();
        assert_eq!(
            request.profile.sqlite_path.as_deref(),
            Some("/tmp/dbide-fixture.sqlite")
        );
        assert!(!request.profile.sqlite_create);
        assert!(request.profile.read_only);
        assert!(request.password.is_empty());
        assert_eq!(request.profile.id, original.id);
        let roundtrip: ConnectionProfile =
            serde_json::from_str(&serde_json::to_string(&request.profile).unwrap()).unwrap();
        assert_eq!(roundtrip, request.profile);
        assert!(profile_target(&request.profile).starts_with("SQLite · /tmp/"));
        assert!(!profile_target(&request.profile).contains(":5432"));
    }
    #[test]
    fn mysql_projection_preserves_typed_network_and_tls_fields() {
        let original = ConnectionProfile::new();
        let fields = [
            "My server",
            "localhost",
            "3307",
            "app",
            "alice",
            "session-secret",
            "9",
            "/tmp/ca.pem",
            "",
        ]
        .map(String::from);
        let request =
            request_from_fields(&original, E::MySql, &fields, TlsMode::Require, false, true)
                .unwrap();
        assert_eq!(request.profile.engine, E::MySql);
        assert_eq!(request.profile.port, 3307);
        assert_eq!(request.profile.tls_mode, TlsMode::Require);
        assert_eq!(
            request.profile.root_cert_path.as_deref(),
            Some("/tmp/ca.pem")
        );
        assert_eq!(request.password, "session-secret");
        assert!(request.profile.read_only);
        let roundtrip: ConnectionProfile =
            serde_json::from_str(&serde_json::to_string(&request.profile).unwrap()).unwrap();
        assert_eq!(roundtrip, request.profile);
    }
}

/// Unknown on a live connection may conceal a manual transaction. A disconnected
/// startup has no transaction to discard. Never infer Failed from a SQL error.
pub fn transaction_may_be_open(
    state: dbide_bridge::ServerTransactionState,
    connected: bool,
) -> bool {
    matches!(
        state,
        dbide_bridge::ServerTransactionState::Active | dbide_bridge::ServerTransactionState::Failed
    ) || (connected && state == dbide_bridge::ServerTransactionState::Unknown)
}
#[cfg(test)]
mod transaction_tests {
    use super::*;
    use dbide_bridge::ServerTransactionState as T;
    #[test]
    fn unknown_live_session_requires_resolution_but_disconnected_startup_does_not() {
        assert!(transaction_may_be_open(T::Unknown, true));
        assert!(!transaction_may_be_open(T::Unknown, false));
        assert!(transaction_may_be_open(T::Active, true));
        assert!(transaction_may_be_open(T::Failed, true));
        assert!(!transaction_may_be_open(T::Idle, true));
    }
}

/// History tabs keep their originating profile requirement even after sidebar selection changes.
pub fn profile_binding_matches(required: Option<&str>, selected: Option<&str>) -> bool {
    required.is_none_or(|id| selected == Some(id))
}
#[cfg(test)]
mod history_tests {
    use super::*;
    #[test]
    fn historical_sql_cannot_run_under_a_different_or_missing_profile() {
        assert!(profile_binding_matches(None, Some("b")));
        assert!(profile_binding_matches(Some("a"), Some("a")));
        assert!(!profile_binding_matches(Some("a"), Some("b")));
        assert!(!profile_binding_matches(Some("missing"), None));
    }
}
