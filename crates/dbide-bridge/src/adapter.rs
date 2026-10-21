//! Private engine dispatch; actor admission, exact cancellation and terminal leases stay shared.
use super::*;
use dbide_driver_api::{DatabaseEngine, Limits};

pub(super) enum AdapterSession {
    PostgreSql(Box<dbide_pg_spike::Session>),
    Sqlite(dbide_sqlite::Session),
    MySql(dbide_mysql::Session),
}
pub(super) struct AdapterExecution {
    pub key: ExecutionKey,
    finish: Finish,
}
impl AdapterExecution {
    pub async fn finish(self) -> Result<ExecutionReport, &'static str> {
        self.finish.await
    }
}
macro_rules! execution {
    ($result:expr) => {{
        let execution = $result?;
        Ok(AdapterExecution {
            key: execution.key,
            finish: Box::pin(execution.finish()),
        })
    }};
}
pub(super) enum CancelResult {
    Accepted,
    NotRunning,
}
pub(super) fn target(request: &ConnectionRequest) -> ConnectionTarget {
    if request.profile.engine == DatabaseEngine::Sqlite {
        let path = request
            .profile
            .sqlite_path
            .as_deref()
            .unwrap_or("<missing file>");
        ConnectionTarget {
            host: "SQLite".into(),
            port: 0,
            database: if path.len() <= 4096 && !path.chars().any(char::is_control) {
                path.into()
            } else {
                "<invalid file>".into()
            },
        }
    } else {
        ConnectionTarget::from(&request.profile)
    }
}
fn diagnostic(request: &ConnectionRequest, kind: ConnectionErrorKind) -> ConnectionDiagnostic {
    let (message, hint) = match kind {
        ConnectionErrorKind::Configuration => (
            "Connection profile is invalid",
            "Check engine, connection fields, timeout and TLS settings.",
        ),
        ConnectionErrorKind::Timeout => (
            "Database connection timed out",
            "Check database availability and connection timeout.",
        ),
        ConnectionErrorKind::Authentication => (
            "Database authentication failed",
            "Check username, session password and server authentication rules.",
        ),
        ConnectionErrorKind::DatabaseMissing => (
            "Requested database or file does not exist",
            "Choose an existing database or explicitly enable SQLite file creation.",
        ),
        ConnectionErrorKind::Tls => (
            "TLS connection or certificate verification failed",
            "Check server TLS, hostname and trusted root certificate.",
        ),
        _ => (
            "Database connection failed",
            "Check database availability, file permissions, network and connection settings.",
        ),
    };
    ConnectionDiagnostic {
        kind,
        target: Box::new(target(request)),
        message,
        hint,
        sqlstate: None,
        os_code: None,
    }
}
impl AdapterSession {
    pub async fn connect_request(
        request: &ConnectionRequest,
        limits: Limits,
    ) -> Result<Self, ConnectionDiagnostic> {
        if request.profile.validate().is_err() {
            return Err(diagnostic(request, ConnectionErrorKind::Configuration));
        }
        let connect = async {
            match request.profile.engine {
                DatabaseEngine::PostgreSql => {
                    dbide_pg_spike::Session::connect_request(request, limits)
                        .await
                        .map(|session| Self::PostgreSql(Box::new(session)))
                }
                DatabaseEngine::Sqlite => dbide_sqlite::Session::connect_request(request, limits)
                    .await
                    .map(Self::Sqlite)
                    .map_err(|error| {
                        let kind = match error.kind {
                            dbide_sqlite::ConnectionErrorKind::Configuration => {
                                ConnectionErrorKind::Configuration
                            }
                            dbide_sqlite::ConnectionErrorKind::NotFound => {
                                ConnectionErrorKind::DatabaseMissing
                            }
                            _ => ConnectionErrorKind::Network,
                        };
                        diagnostic(request, kind)
                    }),
                DatabaseEngine::MySql => dbide_mysql::Session::connect_request(request, limits)
                    .await
                    .map(Self::MySql)
                    .map_err(|error| {
                        let kind = match error {
                            dbide_mysql::ConnectError::Configuration => {
                                ConnectionErrorKind::Configuration
                            }
                            dbide_mysql::ConnectError::Timeout => ConnectionErrorKind::Timeout,
                            dbide_mysql::ConnectError::Authentication => {
                                ConnectionErrorKind::Authentication
                            }
                            dbide_mysql::ConnectError::Database => {
                                ConnectionErrorKind::DatabaseMissing
                            }
                            dbide_mysql::ConnectError::Tls => ConnectionErrorKind::Tls,
                            dbide_mysql::ConnectError::Transport => ConnectionErrorKind::Network,
                        };
                        diagnostic(request, kind)
                    }),
            }
        };
        tokio::time::timeout(
            Duration::from_secs(request.profile.connect_timeout_secs),
            connect,
        )
        .await
        .unwrap_or_else(|_| Err(diagnostic(request, ConnectionErrorKind::Timeout)))
    }
    pub fn transaction_state(&self) -> ServerTransactionState {
        match self {
            Self::PostgreSql(s) => s.transaction_state(),
            Self::Sqlite(s) => s.transaction_state(),
            Self::MySql(s) => s.transaction_state(),
        }
    }
    pub fn is_closed(&self) -> bool {
        match self {
            Self::PostgreSql(s) => s.is_closed(),
            Self::Sqlite(s) => s.is_closed(),
            Self::MySql(s) => s.is_closed(),
        }
    }
    pub fn close(&self) {
        match self {
            Self::PostgreSql(s) => s.close(),
            Self::Sqlite(s) => s.close(),
            Self::MySql(s) => s.close(),
        }
    }
    pub fn submit(
        &self,
        sql: &str,
        policy: CommitPolicy,
    ) -> Result<AdapterExecution, &'static str> {
        match self {
            Self::PostgreSql(s) => execution!(s.submit(sql, policy)),
            Self::Sqlite(s) => execution!(s.submit(sql, policy)),
            Self::MySql(s) => execution!(s.submit(sql, policy)),
        }
    }
    pub fn submit_commit(&self, policy: CommitPolicy) -> Result<AdapterExecution, &'static str> {
        match self {
            Self::PostgreSql(s) => execution!(s.submit_commit(policy)),
            Self::Sqlite(s) => execution!(s.submit_commit(policy)),
            Self::MySql(s) => execution!(s.submit_commit(policy)),
        }
    }
    pub fn submit_rollback(&self, policy: CommitPolicy) -> Result<AdapterExecution, &'static str> {
        match self {
            Self::PostgreSql(s) => execution!(s.submit_rollback(policy)),
            Self::Sqlite(s) => execution!(s.submit_rollback(policy)),
            Self::MySql(s) => execution!(s.submit_rollback(policy)),
        }
    }
    pub fn submit_batch(
        &self,
        statements: Vec<ParameterizedStatement>,
        policy: CommitPolicy,
    ) -> Result<AdapterExecution, &'static str> {
        match self {
            Self::PostgreSql(s) => execution!(s.submit_batch(statements, policy)),
            _ => Err("Parameterized editing batches are currently supported only for PostgreSQL"),
        }
    }
    pub fn submit_guarded_batch(
        &self,
        guard: TableIdentityGuard,
        statements: Vec<ParameterizedStatement>,
        policy: CommitPolicy,
    ) -> Result<AdapterExecution, &'static str> {
        match self {
            Self::PostgreSql(s) => execution!(s.submit_guarded_batch(guard, statements, policy)),
            _ => Err("Parameterized editing batches are currently supported only for PostgreSQL"),
        }
    }
    pub async fn execute(
        &self,
        sql: &str,
        policy: CommitPolicy,
    ) -> Result<ExecutionReport, &'static str> {
        self.submit(sql, policy)?.finish().await
    }
    pub async fn is_running(&self, key: ExecutionKey) -> bool {
        match self {
            Self::PostgreSql(s) => s.is_running(key).await,
            Self::Sqlite(s) => s.is_running(key).await,
            Self::MySql(s) => s.is_running(key).await,
        }
    }
    pub async fn cancel(&self, key: ExecutionKey) -> CancelResult {
        let running = match self {
            Self::PostgreSql(s) => !matches!(
                s.cancel(key).await,
                dbide_pg_spike::CancelResult::NotRunning
            ),
            Self::Sqlite(s) => matches!(s.cancel(key).await, dbide_sqlite::CancelResult::Requested),
            Self::MySql(s) => !matches!(s.cancel(key).await, dbide_mysql::CancelResult::NotRunning),
        };
        if running {
            CancelResult::Accepted
        } else {
            CancelResult::NotRunning
        }
    }
    pub fn databases_sql(&self) -> &'static str {
        match self {
            Self::PostgreSql(_) => {
                "SELECT datname,CASE WHEN has_database_privilege(oid,'CONNECT') THEN 'true' ELSE 'false' END,CASE WHEN datname=current_database() THEN 'true' ELSE 'false' END FROM pg_catalog.pg_database WHERE datallowconn AND NOT datistemplate ORDER BY datname LIMIT 257"
            }
            // SHOW visibility is catalog-advertised access, not a successful USE/connection test.
            Self::MySql(_) => "SELECT DATABASE(); SHOW DATABASES",
            // Attachments are physical-session state, so a probe advertises only its configured main file.
            Self::Sqlite(_) => "SELECT 'main','true','true'",
        }
    }
    fn literal(&self, value: &str) -> String {
        match self {
            Self::PostgreSql(_) => {
                format!("E'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
            }
            Self::Sqlite(_) => format!("'{}'", value.replace('\'', "''")),
            Self::MySql(_) => {
                use std::fmt::Write;
                let mut literal = String::from("CONVERT(X'");
                for byte in value.as_bytes() {
                    write!(&mut literal, "{byte:02x}").expect("writing String succeeds");
                }
                literal.push_str("' USING utf8mb4)");
                literal
            }
        }
    }
    pub fn catalog_sql(&self, request: &CatalogRequest) -> String {
        match request {
            CatalogRequest::Schemas=>match self {
                Self::PostgreSql(_)=>"SELECT nspname FROM pg_catalog.pg_namespace WHERE left(nspname,3)<>'pg_' AND nspname<>'information_schema' AND has_schema_privilege(oid,'USAGE') ORDER BY nspname LIMIT 257".into(),
                Self::MySql(_)=>"SELECT DATABASE() WHERE DATABASE() IS NOT NULL LIMIT 257".into(),
                Self::Sqlite(_)=>"SELECT 'main' LIMIT 257".into(),
            },
            CatalogRequest::Objects(schema)=>{
                let schema=self.literal(schema);
                match self {
                    Self::PostgreSql(_)=>format!("SELECT n.nspname,c.relname,c.relkind::text FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname={schema} AND c.relkind IN ('r','p','v','m','f') ORDER BY c.relname LIMIT 2049"),
                    Self::MySql(_)=>format!("SELECT TABLE_SCHEMA,TABLE_NAME,CASE WHEN TABLE_TYPE='VIEW' THEN 'v' ELSE 'r' END FROM information_schema.TABLES WHERE TABLE_SCHEMA=DATABASE() AND TABLE_SCHEMA={schema} ORDER BY TABLE_NAME LIMIT 2049"),
                    Self::Sqlite(_)=>format!("SELECT 'main',name,CASE type WHEN 'view' THEN 'v' ELSE 'r' END FROM main.sqlite_schema WHERE {schema}='main' AND type IN ('table','view') AND lower(substr(name,1,7))<>'sqlite_' ORDER BY name LIMIT 2049"),
                }
            }
            CatalogRequest::Columns(relation)=>{
                let schema=self.literal(&relation.schema);let name=self.literal(&relation.name);
                let predicate=match self {
                    Self::PostgreSql(_)=>format!("AND n.nspname={schema} AND c.relname={name} "),
                    Self::MySql(_)=>format!("AND t.TABLE_SCHEMA={schema} AND t.TABLE_NAME={name} "),
                    Self::Sqlite(_)=>format!("AND {schema}='main' AND s.name={name} "),
                };
                let (query,order)=self.metadata_sql().rsplit_once("ORDER BY").expect("static catalog query has final ordering");
                format!("{query} {predicate} ORDER BY {order}")
            }
        }
    }
    pub fn metadata_sql(&self) -> &'static str {
        match self {
            Self::PostgreSql(_) => {
                r#"SELECT n.nspname, c.relname, c.relkind::text, a.attname,
                pg_catalog.format_type(a.atttypid,a.atttypmod),
                CASE WHEN a.attnotnull THEN 'false' ELSE 'true' END,
                (SELECT k.ordinality::text FROM pg_catalog.pg_constraint pk
                 CROSS JOIN LATERAL unnest(pk.conkey) WITH ORDINALITY AS k(attnum,ordinality)
                 WHERE pk.conrelid=c.oid AND pk.contype='p' AND k.attnum=a.attnum LIMIT 1),
                CASE WHEN EXISTS(SELECT 1 FROM pg_catalog.pg_constraint fk
                    WHERE fk.conrelid=c.oid AND fk.contype='f' AND a.attnum=ANY(fk.conkey)) THEN 'true' ELSE 'false' END,
                pg_catalog.pg_get_expr(d.adbin,d.adrelid),
                CASE a.attidentity WHEN 'a' THEN 'identity always' WHEN 'd' THEN 'identity by default'
                    ELSE CASE a.attgenerated WHEN 's' THEN 'generated stored' WHEN 'v' THEN 'generated virtual' END END
                FROM pg_catalog.pg_class c
                JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
                LEFT JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped
                LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum
                WHERE c.relkind IN ('r','p','v','m','f') AND n.nspname NOT IN ('pg_catalog','information_schema')
                  AND n.nspname NOT LIKE 'pg_toast%'
                ORDER BY n.nspname,c.relname,a.attnum LIMIT 2049"#
            }
            Self::Sqlite(_) => {
                r#"SELECT 'main', s.name, CASE s.type WHEN 'view' THEN 'v' ELSE 'r' END,
                p.name, p.type,
                CASE WHEN p."notnull"=1 OR (p.pk>0 AND upper(trim(p.type))='INTEGER'
                    AND NOT EXISTS(SELECT 1 FROM pragma_index_list(s.name,'main') pi WHERE pi.origin='pk'))
                    THEN 'false' ELSE 'true' END,
                CASE WHEN p.pk>0 THEN CAST(p.pk AS TEXT) END,
                CASE WHEN EXISTS(SELECT 1 FROM pragma_foreign_key_list(s.name,'main') fk WHERE fk."from"=p.name COLLATE NOCASE)
                    THEN 'true' ELSE 'false' END,
                p.dflt_value,
                CASE p.hidden WHEN 1 THEN 'hidden' WHEN 2 THEN 'generated virtual' WHEN 3 THEN 'generated stored'
                    ELSE CASE WHEN p.pk>0 AND upper(trim(p.type))='INTEGER'
                        AND NOT EXISTS(SELECT 1 FROM pragma_index_list(s.name,'main') pi WHERE pi.origin='pk')
                        THEN 'rowid alias' END END
                FROM main.sqlite_schema AS s LEFT JOIN pragma_table_xinfo(s.name,'main') AS p ON 1=1
                WHERE s.type IN ('table','view') AND lower(substr(s.name,1,7))<>'sqlite_'
                ORDER BY s.name,p.cid LIMIT 2049"#
            }
            Self::MySql(_) => {
                r#"SELECT t.TABLE_SCHEMA,t.TABLE_NAME,CASE WHEN t.TABLE_TYPE='VIEW' THEN 'v' ELSE 'r' END,
                c.COLUMN_NAME,c.COLUMN_TYPE,CASE WHEN c.IS_NULLABLE='YES' THEN 'true' ELSE 'false' END,
                (SELECT CAST(k.ORDINAL_POSITION AS CHAR) FROM information_schema.KEY_COLUMN_USAGE k
                 WHERE k.TABLE_SCHEMA=t.TABLE_SCHEMA AND k.TABLE_NAME=t.TABLE_NAME
                   AND k.COLUMN_NAME=c.COLUMN_NAME AND k.CONSTRAINT_NAME='PRIMARY' LIMIT 1),
                CASE WHEN EXISTS(SELECT 1 FROM information_schema.KEY_COLUMN_USAGE fk
                    WHERE fk.TABLE_SCHEMA=t.TABLE_SCHEMA AND fk.TABLE_NAME=t.TABLE_NAME
                      AND fk.COLUMN_NAME=c.COLUMN_NAME AND fk.REFERENCED_TABLE_NAME IS NOT NULL)
                    THEN 'true' ELSE 'false' END,
                c.COLUMN_DEFAULT,NULLIF(c.EXTRA,'')
                FROM information_schema.TABLES t LEFT JOIN information_schema.COLUMNS c
                  ON c.TABLE_SCHEMA=t.TABLE_SCHEMA AND c.TABLE_NAME=t.TABLE_NAME
                WHERE t.TABLE_SCHEMA=DATABASE()
                ORDER BY t.TABLE_SCHEMA,t.TABLE_NAME,c.ORDINAL_POSITION LIMIT 2049"#
            }
        }
    }
}
