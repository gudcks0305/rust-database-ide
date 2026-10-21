//! Exclusive, parameterized CRUD batches. Transaction controls never come from callers.
use crate::{
    Active, CommitPolicy, Event, ExecutionReport, Limits, Outcome, ServerError,
    ServerTransactionState as Tx, retain,
};
use futures_util::StreamExt;
use sqlparser::{
    ast::{
        Expr, FromTable, Ident, ObjectName, Statement, TableFactor, TableObject, Value, Visit,
        Visitor,
    },
    dialect::PostgreSqlDialect,
    parser::Parser,
};
use std::{fmt, ops::ControlFlow, time::Duration};
use tokio::sync::Mutex;
use tokio_postgres::{
    Client,
    types::{IsNull, ToSql, Type},
};

pub const MAX_BATCH_STATEMENTS: usize = 256;
pub const MAX_BATCH_PARAMETERS: usize = 4096;
pub const MAX_BATCH_PARAMETER_BYTES: usize = 4 * 1024 * 1024;
/// Conflict is an application failure code; it is never presented as a PostgreSQL SQLSTATE.
pub const BATCH_CONFLICT_CODE: &str = "Conflict";
#[derive(Clone)]
pub struct ParameterizedStatement {
    pub sql: String,
    pub parameters: Vec<Option<String>>,
    pub expected_affected_rows: Option<u64>,
}
impl fmt::Debug for ParameterizedStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParameterizedStatement")
            .field("sql_bytes", &self.sql.len())
            .field("parameter_count", &self.parameters.len())
            .field("expected_affected_rows", &self.expected_affected_rows)
            .finish()
    }
}
/// Identity captured with table metadata, never looked up afresh by apply callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableIdentityGuard {
    pub schema: String,
    pub table: String,
    pub oid: u32,
    pub metadata_fingerprint: Option<String>,
}
impl TableIdentityGuard {
    pub fn validate(&self) -> Result<(), &'static str> {
        for name in [&self.schema, &self.table] {
            if name.is_empty() || name.len() > 63 || name.as_bytes().contains(&0) {
                return Err("Guard identifiers require1..63 UTF-8 bytes and no NUL");
            }
        }
        if self.metadata_fingerprint.as_ref().is_some_and(|value| {
            value.len() != 64
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            return Err("Metadata fingerprint must be64 lowercase SHA256 hex characters");
        }
        if self.oid == 0 {
            return Err("Guard relation OID must be positive");
        }
        Ok(())
    }
}
fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn target_name(statement: &Statement) -> Option<&ObjectName> {
    match statement {
        Statement::Insert(insert) => match &insert.table {
            TableObject::TableName(name) => Some(name),
            _ => None,
        },
        Statement::Update(update) if update.table.joins.is_empty() => {
            match &update.table.relation {
                TableFactor::Table {
                    name, args: None, ..
                } => Some(name),
                _ => None,
            }
        }
        Statement::Delete(delete) if delete.tables.is_empty() => {
            let (FromTable::WithFromKeyword(tables) | FromTable::WithoutKeyword(tables)) =
                &delete.from;
            match tables.as_slice() {
                [table] if table.joins.is_empty() => match &table.relation {
                    TableFactor::Table {
                        name, args: None, ..
                    } => Some(name),
                    _ => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
}
fn resolved_identifier(identifier: &Ident) -> String {
    if identifier.quote_style.is_some() {
        identifier.value.clone()
    } else {
        identifier.value.to_ascii_lowercase()
    }
}
struct GuardTargets<'a>(&'a TableIdentityGuard);
impl Visitor for GuardTargets<'_> {
    type Break = &'static str;
    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<Self::Break> {
        // Query nodes nested in an INSERT source may read other relations; every
        // mutating statement, including writable CTEs, must target this guard.
        if matches!(statement, Statement::Query(_)) {
            return ControlFlow::Continue(());
        }
        let Some(name) = target_name(statement) else {
            return ControlFlow::Break("Guarded batch requires ordinary table-targeted CRUD");
        };
        let [schema, table] = name.0.as_slice() else {
            return ControlFlow::Break("Guarded mutation targets must be schema-qualified");
        };
        let (Some(schema), Some(table)) = (schema.as_ident(), table.as_ident()) else {
            return ControlFlow::Break("Guarded mutation target is not a static relation name");
        };
        if resolved_identifier(schema) != self.0.schema
            || resolved_identifier(table) != self.0.table
        {
            return ControlFlow::Break("Guarded mutation target differs from captured table");
        }
        ControlFlow::Continue(())
    }
}
pub(crate) fn validate_guarded(
    guard: &TableIdentityGuard,
    statements: &[ParameterizedStatement],
    limits: Limits,
) -> Result<(), &'static str> {
    guard.validate()?;
    validate(statements, limits)?;
    for statement in statements {
        let ast = Parser::parse_sql(&PostgreSqlDialect {}, &statement.sql)
            .map_err(|_| "Guarded SQL failed local validation")?;
        if let ControlFlow::Break(error) = ast.visit(&mut GuardTargets(guard)) {
            return Err(error);
        }
    }
    Ok(())
}
struct Placeholders {
    used: Vec<bool>,
}
impl Visitor for Placeholders {
    type Break = &'static str;
    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        if let Expr::Value(value) = expr
            && let Value::Placeholder(name) = &value.value
        {
            let Some(index) = name
                .strip_prefix('$')
                .and_then(|n| n.parse::<usize>().ok())
                .and_then(|n| n.checked_sub(1))
            else {
                return ControlFlow::Break(
                    "Batch parameters must use numbered PostgreSQL placeholders",
                );
            };
            let Some(used) = self.used.get_mut(index) else {
                return ControlFlow::Break("Batch placeholder index exceeds parameter count");
            };
            *used = true;
        }
        ControlFlow::Continue(())
    }
}
pub(crate) fn validate(
    statements: &[ParameterizedStatement],
    limits: Limits,
) -> Result<(), &'static str> {
    if statements.is_empty() || statements.len() > MAX_BATCH_STATEMENTS {
        return Err("Batch requires between1 and256 CRUD statements");
    }
    let mut sql_bytes = 0usize;
    let mut parameter_bytes = 0usize;
    let mut parameter_count = 0usize;
    for statement in statements {
        sql_bytes = sql_bytes.saturating_add(statement.sql.len());
        parameter_count = parameter_count.saturating_add(statement.parameters.len());
        if sql_bytes > limits.sql_bytes || statement.sql.as_bytes().contains(&0) {
            return Err("Batch SQL exceeds byte budget or contains NUL");
        }
        if parameter_count > MAX_BATCH_PARAMETERS {
            return Err("Batch parameter count exceeds4096");
        }
        for value in &statement.parameters {
            if let Some(value) = value {
                parameter_bytes = parameter_bytes.saturating_add(value.len());
                if value.as_bytes().contains(&0) {
                    return Err("Batch text parameter contains NUL");
                }
            }
            if parameter_bytes > MAX_BATCH_PARAMETER_BYTES {
                return Err("Batch parameter payload exceeds4MiB");
            }
        }
        let parsed = Parser::parse_sql(&PostgreSqlDialect {}, &statement.sql)
            .map_err(|_| "Batch SQL failed local PostgreSQL syntax validation")?;
        if !matches!(
            parsed.as_slice(),
            [Statement::Insert(_) | Statement::Update { .. } | Statement::Delete(_)]
        ) {
            return Err("Batch accepts exactly one INSERT, UPDATE, or DELETE per statement");
        }
        let mut placeholders = Placeholders {
            used: vec![false; statement.parameters.len()],
        };
        if let ControlFlow::Break(error) = parsed.visit(&mut placeholders) {
            return Err(error);
        }
        if placeholders.used.iter().any(|used| !*used) {
            return Err("Batch contains unused or missing parameter placeholders");
        }
    }
    Ok(())
}
/// Driver debug logging sees a marker rather than plaintext parameter values.
struct BoundText<'a>(&'a Option<String>);
impl fmt::Debug for BoundText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<bound text>")
    }
}
impl ToSql for BoundText<'_> {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut bytes::BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        self.0.to_sql(ty, out)
    }
    fn accepts(ty: &Type) -> bool {
        *ty == Type::TEXT
    }
    tokio_postgres::types::to_sql_checked!();
}
pub(crate) struct ResultState {
    pub state: Tx,
    pub broken: bool,
}
enum Scope {
    New,
    Savepoint(String),
}
struct BatchClient<'a> {
    client: &'a Client,
    cancel_tls: Option<&'a postgres_native_tls::MakeTlsConnector>,
}
impl std::ops::Deref for BatchClient<'_> {
    type Target = Client;
    fn deref(&self) -> &Client {
        self.client
    }
}
impl BatchClient<'_> {
    async fn cancel_on_timeout(&self) {
        // This backend is retired after an ambiguous client deadline. Request
        // server cancellation before socket close; never reuse/replay afterward.
        let token = self.client.cancel_token();
        let send = async {
            match self.cancel_tls {
                Some(tls) => token.cancel_query(tls.clone()).await,
                None => token.cancel_query(tokio_postgres::NoTls).await,
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(3), send).await;
    }
}
enum ControlError {
    Database(tokio_postgres::Error),
    Unknown,
}
async fn command(client: &BatchClient<'_>, sql: &str, expected: Tx) -> Result<(), ControlError> {
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let stream = client
            .simple_query_raw(sql)
            .await
            .map_err(|_| ControlError::Unknown)?;
        tokio::pin!(stream);
        let mut failure = None;
        while let Some(item) = stream.next().await {
            if let Err(error) = item {
                if error.as_db_error().is_none() {
                    return Err(ControlError::Unknown);
                }
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
        let state = match stream.as_ref().get_ref().transaction_status() {
            Some(tokio_postgres::TransactionStatus::Idle) => Tx::Idle,
            Some(tokio_postgres::TransactionStatus::InTransaction) => Tx::Active,
            Some(tokio_postgres::TransactionStatus::InError) => Tx::Failed,
            None => return Err(ControlError::Unknown),
        };
        if let Some(error) = failure {
            Err(ControlError::Database(error))
        } else if state == expected {
            Ok(())
        } else {
            Err(ControlError::Unknown)
        }
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            client.cancel_on_timeout().await;
            Err(ControlError::Unknown)
        }
    }
}
async fn control(client: &BatchClient<'_>, sql: &str, expected: Tx) -> bool {
    command(client, sql, expected).await.is_ok()
}
async fn guard_query_opt(
    client: &BatchClient<'_>,
    sql: &str,
    parameters: &[&(dyn ToSql + Sync)],
) -> Result<Option<tokio_postgres::Row>, ControlError> {
    match tokio::time::timeout(Duration::from_secs(5), client.query_opt(sql, parameters)).await {
        Ok(Ok(row)) => Ok(row),
        Ok(Err(error)) if error.as_db_error().is_some() => Err(ControlError::Database(error)),
        Ok(Err(_)) => Err(ControlError::Unknown),
        Err(_) => {
            client.cancel_on_timeout().await;
            Err(ControlError::Unknown)
        }
    }
}
async fn guard_query(
    client: &BatchClient<'_>,
    sql: &str,
    parameters: &[&(dyn ToSql + Sync)],
) -> Result<tokio_postgres::Row, ControlError> {
    guard_query_opt(client, sql, parameters)
        .await?
        .ok_or(ControlError::Unknown)
}
struct GuardTimeouts {
    lock: String,
    statement: String,
}
impl GuardTimeouts {
    async fn cap(client: &BatchClient<'_>) -> Result<Self, ControlError> {
        let row=guard_query(client,"SELECT pg_catalog.current_setting('lock_timeout'),pg_catalog.current_setting('statement_timeout'),CASE WHEN l.setting::pg_catalog.int8=0 OR l.setting::pg_catalog.int8>4000 THEN '4000ms' ELSE pg_catalog.current_setting('lock_timeout') END,CASE WHEN s.setting::pg_catalog.int8=0 OR s.setting::pg_catalog.int8>4000 THEN '4000ms' ELSE pg_catalog.current_setting('statement_timeout') END FROM pg_catalog.pg_settings l CROSS JOIN pg_catalog.pg_settings s WHERE l.name='lock_timeout' AND s.name='statement_timeout'",&[]).await?;
        let saved = Self {
            lock: row.get(0),
            statement: row.get(1),
        };
        let lock: String = row.get(2);
        let statement: String = row.get(3);
        guard_query(client,"SELECT pg_catalog.set_config('lock_timeout',$1::text,true),pg_catalog.set_config('statement_timeout',$2::text,true)",&[&lock,&statement]).await?;
        Ok(saved)
    }
    async fn restore(self, client: &BatchClient<'_>) -> Result<(), ControlError> {
        guard_query(client,"SELECT pg_catalog.set_config('lock_timeout',$1::text,true),pg_catalog.set_config('statement_timeout',$2::text,true)",&[&self.lock,&self.statement]).await?;
        Ok(())
    }
}
async fn retired(active: &Mutex<Option<Active>>) -> bool {
    active.lock().await.as_ref().is_none_or(|a| a.retire)
}
fn failure(
    report: &mut ExecutionReport,
    outcome: Outcome,
    state: Option<String>,
    message: String,
    limits: Limits,
) {
    report.outcome = outcome;
    report.terminal_error = Some(ServerError {
        sqlstate: state.clone(),
        message: message.clone(),
    });
    retain(
        report,
        Event::Error {
            sqlstate: state,
            message,
        },
        limits,
    );
}
async fn recover(
    client: &BatchClient<'_>,
    scope: &Scope,
    report: &mut ExecutionReport,
    active: &Mutex<Option<Active>>,
    limits: Limits,
) -> ResultState {
    if retired(active).await {
        return ResultState {
            state: Tx::Unknown,
            broken: true,
        };
    }
    let (sql, state) = match scope {
        Scope::New => ("ROLLBACK".into(), Tx::Idle),
        Scope::Savepoint(name) => (
            format!("ROLLBACK TO SAVEPOINT {name}; RELEASE SAVEPOINT {name}"),
            Tx::Active,
        ),
    };
    if control(client, &sql, state).await {
        ResultState {
            state,
            broken: false,
        }
    } else {
        failure(report,Outcome::OutcomeUnknown,None,"Batch rollback could not be confirmed; connection retired. Review server state before further changes.".into(),limits);
        ResultState {
            state: Tx::Unknown,
            broken: true,
        }
    }
}
#[derive(Clone, Copy)]
enum FailureStage {
    Statement(usize),
    Completion,
    Identity,
}
async fn database_failure(
    client: &BatchClient<'_>,
    scope: &Scope,
    report: &mut ExecutionReport,
    active: &Mutex<Option<Active>>,
    error: tokio_postgres::Error,
    stage: FailureStage,
    limits: Limits,
) -> ResultState {
    let state = error.code().map(|code| code.code().to_owned());
    let outcome = if state.as_deref() == Some("57014") {
        Outcome::Cancelled
    } else if error.as_db_error().is_some() {
        Outcome::Failed
    } else {
        Outcome::OutcomeUnknown
    };
    failure(
        report,
        outcome,
        state,
        match stage {
            FailureStage::Statement(index)=>format!("Batch statement {} failed; no batch commit was acknowledged. PostgreSQL SQLSTATE, when available, identifies the error.",index+1),
            FailureStage::Completion=>"Batch transaction completion failed; no batch commit was acknowledged. PostgreSQL SQLSTATE, when available, identifies the error.".into(),
            FailureStage::Identity=>match error.code().map(|code|code.code()) {
                Some("55P03")=>"Table metadata guard could not acquire its lock before the server timeout; no mutation was dispatched. Retry after other transactions finish.".into(),
                Some("57014")=>"Table metadata guard reached its server statement timeout; no mutation was dispatched. Retry after concurrent work finishes.".into(),
                _=>"Guarded table identity verification failed before mutation; refresh table metadata before applying changes.".into(),
            },
        },
        limits,
    );
    if outcome == Outcome::OutcomeUnknown {
        ResultState {
            state: Tx::Unknown,
            broken: true,
        }
    } else {
        recover(client, scope, report, active, limits).await
    }
}
pub(crate) struct BatchRequest<'a> {
    pub statements: &'a [ParameterizedStatement],
    pub guard: Option<&'a TableIdentityGuard>,
    pub policy: CommitPolicy,
    pub initial: Tx,
    pub cancel_tls: Option<&'a postgres_native_tls::MakeTlsConnector>,
}
pub(crate) async fn run(
    client: &Client,
    request: BatchRequest<'_>,
    report: &mut ExecutionReport,
    active: &Mutex<Option<Active>>,
    limits: Limits,
) -> ResultState {
    let BatchRequest {
        statements,
        guard,
        policy,
        initial,
        cancel_tls,
    } = request;
    let client = &BatchClient { client, cancel_tls };
    if initial == Tx::Failed {
        failure(
            report,
            Outcome::Rejected,
            None,
            "Transaction is failed; Rollback is required before a batch".into(),
            limits,
        );
        return ResultState {
            state: initial,
            broken: false,
        };
    }
    if !matches!(initial, Tx::Idle | Tx::Active) || retired(active).await {
        failure(
            report,
            Outcome::Rejected,
            None,
            "Batch was not dispatched: connection state is unknown or cancellation was requested"
                .into(),
            limits,
        );
        return ResultState {
            state: Tx::Unknown,
            broken: true,
        };
    }
    let scope = if initial == Tx::Idle {
        Scope::New
    } else {
        Scope::Savepoint(format!(
            "dbide_batch_{}_{}",
            report.key.session, report.key.execution
        ))
    };
    let begin = match &scope {
        Scope::New => "BEGIN".into(),
        Scope::Savepoint(name) => format!("SAVEPOINT {name}"),
    };
    if !control(client, &begin, Tx::Active).await {
        failure(report,Outcome::OutcomeUnknown,None,"Could not establish the batch transaction boundary; batch statements were not dispatched".into(),limits);
        return ResultState {
            state: Tx::Unknown,
            broken: true,
        };
    }
    if guard.is_some() {
        match guard_query(
            client,
            "SELECT pg_catalog.current_setting('transaction_isolation')",
            &[],
        )
        .await
        {
            Ok(row) if row.get::<_, String>(0) == "read committed" => {}
            Ok(_) => {
                failure(report,Outcome::Rejected,None,"UnsupportedGuardIsolation: guarded edits require Read Committed; no mutation was dispatched and existing outer work was preserved. Commit/Rollback and start Read Committed before applying.".into(),limits);
                return recover(client, &scope, report, active, limits).await;
            }
            Err(ControlError::Database(error)) => {
                return database_failure(
                    client,
                    &scope,
                    report,
                    active,
                    error,
                    FailureStage::Identity,
                    limits,
                )
                .await;
            }
            Err(ControlError::Unknown) => {
                failure(report,Outcome::OutcomeUnknown,None,"Guard isolation check did not acknowledge completion; no mutation was dispatched".into(),limits);
                return ResultState {
                    state: Tx::Unknown,
                    broken: true,
                };
            }
        }
    }
    let guard_timeouts = if guard.is_some_and(|g| g.metadata_fingerprint.is_some()) {
        match GuardTimeouts::cap(client).await {
            Ok(saved) => Some(saved),
            Err(ControlError::Database(error)) => {
                return database_failure(
                    client,
                    &scope,
                    report,
                    active,
                    error,
                    FailureStage::Identity,
                    limits,
                )
                .await;
            }
            Err(ControlError::Unknown) => {
                failure(
                    report,
                    Outcome::OutcomeUnknown,
                    None,
                    "Guard timeout setup could not be confirmed; no mutation was dispatched".into(),
                    limits,
                );
                return ResultState {
                    state: Tx::Unknown,
                    broken: true,
                };
            }
        }
    } else {
        None
    };
    if let Some(guard) = guard {
        let lock_mode = if guard.metadata_fingerprint.is_some() {
            "SHARE UPDATE EXCLUSIVE"
        } else {
            "ROW EXCLUSIVE"
        };
        let lock = format!(
            "LOCK TABLE {}.{} IN {lock_mode} MODE",
            quote_identifier(&guard.schema),
            quote_identifier(&guard.table)
        );
        match command(client, &lock, Tx::Active).await {
            Ok(()) => {}
            Err(ControlError::Database(error)) => {
                return database_failure(
                    client,
                    &scope,
                    report,
                    active,
                    error,
                    FailureStage::Identity,
                    limits,
                )
                .await;
            }
            Err(ControlError::Unknown) => {
                failure(
                    report,
                    Outcome::OutcomeUnknown,
                    None,
                    "Guarded table lock could not be confirmed; no mutation was dispatched".into(),
                    limits,
                );
                return ResultState {
                    state: Tx::Unknown,
                    broken: true,
                };
            }
        }
        let identity=guard_query_opt(client,"SELECT c.oid,c.relkind::text FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1::pg_catalog.text AND c.relname=$2::pg_catalog.text",&[&guard.schema,&guard.table]).await;
        match identity {
            Ok(Some(row))
                if row.get::<_, u32>(0) == guard.oid && row.get::<_, String>(1) == "r" => {}
            Ok(_) => {
                failure(report,Outcome::Failed,None,"TableIdentityChanged: captured relation OID or ordinary table kind no longer matches; refresh metadata before applying changes".into(),limits);
                return recover(client, &scope, report, active, limits).await;
            }
            Err(ControlError::Database(error)) => {
                return database_failure(
                    client,
                    &scope,
                    report,
                    active,
                    error,
                    FailureStage::Identity,
                    limits,
                )
                .await;
            }
            Err(ControlError::Unknown) => {
                failure(report,Outcome::OutcomeUnknown,None,"Guard relation identity lookup did not acknowledge completion; no mutation was dispatched".into(),limits);
                return ResultState {
                    state: Tx::Unknown,
                    broken: true,
                };
            }
        }
    }
    if let Some(guard) = guard
        && let Some(expected) = &guard.metadata_fingerprint
    {
        let metadata = guard_query(client, &crate::table_metadata_query(guard.oid), &[]).await;
        match metadata {
            Ok(row) => {
                let canonical: Option<String> = row.get(0);
                if canonical.as_ref().is_none_or(|value| {
                    value.len() > crate::MAX_TABLE_METADATA_BYTES
                        || crate::table_metadata_fingerprint(value) != *expected
                }) {
                    failure(report,Outcome::Failed,None,"TableMetadataChanged: captured table metadata no longer matches or exceeds guard budget; refresh metadata before applying changes".into(),limits);
                    return recover(client, &scope, report, active, limits).await;
                }
            }
            Err(ControlError::Database(error)) => {
                return database_failure(
                    client,
                    &scope,
                    report,
                    active,
                    error,
                    FailureStage::Identity,
                    limits,
                )
                .await;
            }
            Err(ControlError::Unknown) => {
                failure(report,Outcome::OutcomeUnknown,None,"Guard metadata query did not acknowledge completion; no mutation was dispatched".into(),limits);
                return ResultState {
                    state: Tx::Unknown,
                    broken: true,
                };
            }
        }
    }
    if let Some(saved) = guard_timeouts {
        match saved.restore(client).await {
            Ok(()) => {}
            Err(ControlError::Database(error)) => {
                return database_failure(
                    client,
                    &scope,
                    report,
                    active,
                    error,
                    FailureStage::Identity,
                    limits,
                )
                .await;
            }
            Err(ControlError::Unknown) => {
                failure(
                    report,
                    Outcome::OutcomeUnknown,
                    None,
                    "Guard timeout restoration was not acknowledged; no mutation was dispatched"
                        .into(),
                    limits,
                );
                return ResultState {
                    state: Tx::Unknown,
                    broken: true,
                };
            }
        }
    }
    // All local validation has already completed. Server prepare belongs INSIDE
    // this scope, so a missing relation/column cannot abort unrelated prior work.
    let mut prepared = Vec::with_capacity(statements.len());
    for (index, statement) in statements.iter().enumerate() {
        if retired(active).await {
            failure(
                report,
                Outcome::Rejected,
                None,
                "Batch cancelled before mutation dispatch; connection retired".into(),
                limits,
            );
            return ResultState {
                state: Tx::Unknown,
                broken: true,
            };
        }
        match client
            .prepare_typed(
                &statement.sql,
                &vec![Type::TEXT; statement.parameters.len()],
            )
            .await
        {
            Ok(statement) => prepared.push(statement),
            Err(error) => {
                return database_failure(
                    client,
                    &scope,
                    report,
                    active,
                    error,
                    FailureStage::Statement(index),
                    limits,
                )
                .await;
            }
        }
    }
    for (index, (statement, prepared)) in statements.iter().zip(&prepared).enumerate() {
        if retired(active).await {
            failure(report,Outcome::OutcomeUnknown,None,"Batch cancellation requested; connection retired without a batch commit acknowledgement".into(),limits);
            return ResultState {
                state: Tx::Unknown,
                broken: true,
            };
        }
        let bound = statement
            .parameters
            .iter()
            .map(BoundText)
            .collect::<Vec<_>>();
        let parameters = bound
            .iter()
            .map(|value| value as &(dyn ToSql + Sync))
            .collect::<Vec<_>>();
        match client.execute(prepared, &parameters).await {
            Ok(actual) => {
                retain(
                    report,
                    Event::CommandComplete {
                        affected_rows: actual,
                    },
                    limits,
                );
                if statement
                    .expected_affected_rows
                    .is_some_and(|expected| expected != actual)
                {
                    failure(
                        report,
                        Outcome::Failed,
                        None,
                        format!(
                            "{BATCH_CONFLICT_CODE}: statement{} expected{} affected rows, received{actual}; batch row changes rolled back",
                            index + 1,
                            statement.expected_affected_rows.unwrap()
                        ),
                        limits,
                    );
                    return recover(client, &scope, report, active, limits).await;
                }
            }
            Err(error) => {
                return database_failure(
                    client,
                    &scope,
                    report,
                    active,
                    error,
                    FailureStage::Statement(index),
                    limits,
                )
                .await;
            }
        }
    }
    if retired(active).await {
        failure(
            report,
            Outcome::OutcomeUnknown,
            None,
            "Batch cancelled before commit acknowledgement; connection retired".into(),
            limits,
        );
        return ResultState {
            state: Tx::Unknown,
            broken: true,
        };
    }
    let (finish, state) = match &scope {
        Scope::New if policy == CommitPolicy::Auto => (Some("COMMIT".into()), Tx::Idle),
        Scope::New => (None, Tx::Active),
        Scope::Savepoint(name) => (Some(format!("RELEASE SAVEPOINT {name}")), Tx::Active),
    };
    if let Some(sql) = finish {
        match command(client, &sql, state).await {
            Ok(()) => {}
            Err(ControlError::Database(error)) => {
                return database_failure(
                    client,
                    &scope,
                    report,
                    active,
                    error,
                    FailureStage::Completion,
                    limits,
                )
                .await;
            }
            Err(ControlError::Unknown) => {
                failure(report,Outcome::OutcomeUnknown,None,"Batch transaction completion was not acknowledged; connection retired. SQL was not replayed.".into(),limits);
                return ResultState {
                    state: Tx::Unknown,
                    broken: true,
                };
            }
        }
    }
    report.outcome = Outcome::Completed;
    ResultState {
        state,
        broken: false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn statement(sql: &str, parameters: Vec<Option<String>>) -> ParameterizedStatement {
        ParameterizedStatement {
            sql: sql.into(),
            parameters,
            expected_affected_rows: None,
        }
    }
    #[test]
    fn identity_guard_checks_exact_targets_and_utf8_identifier_bounds() {
        let limits = Limits::default();
        let guard = TableIdentityGuard {
            schema: "public".into(),
            table: "items".into(),
            oid: 42,
            metadata_fingerprint: None,
        };
        for sql in [
            "INSERT INTO public.items(v) VALUES($1)",
            "UPDATE public.items SET v=$1",
            "DELETE FROM public.items WHERE v=$1",
        ] {
            assert!(
                validate_guarded(
                    &guard,
                    &[statement(sql, vec![Some("value".into())])],
                    limits
                )
                .is_ok(),
                "{sql}"
            );
        }
        for sql in [
            "INSERT INTO items(v) VALUES($1)",
            "INSERT INTO other.items(v) VALUES($1)",
            "UPDATE public.other SET v=$1",
            "WITH changed AS (DELETE FROM public.other RETURNING v) INSERT INTO public.items(v) SELECT $1 FROM changed",
        ] {
            assert!(
                validate_guarded(
                    &guard,
                    &[statement(sql, vec![Some("value".into())])],
                    limits
                )
                .is_err(),
                "{sql}"
            );
        }
        for name in [
            "".into(),
            "a".repeat(64),
            "한".repeat(22),
            "nul\0name".into(),
        ] {
            let invalid = TableIdentityGuard {
                table: name,
                ..guard.clone()
            };
            assert!(invalid.validate().is_err());
        }
        assert!(
            TableIdentityGuard {
                table: "한".repeat(21),
                ..guard.clone()
            }
            .validate()
            .is_ok()
        );
        assert!(
            TableIdentityGuard {
                oid: 0,
                ..guard.clone()
            }
            .validate()
            .is_err()
        );
        for value in ["".into(), "g".repeat(64), "A".repeat(64), "0".repeat(63)] {
            assert!(
                TableIdentityGuard {
                    metadata_fingerprint: Some(value),
                    ..guard.clone()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            TableIdentityGuard {
                metadata_fingerprint: Some("0".repeat(64)),
                ..guard.clone()
            }
            .validate()
            .is_ok()
        );
        let quoted = TableIdentityGuard {
            schema: "한글 schema".into(),
            table: "quoted\"; DROP TABLE unrelated;--".into(),
            oid: 42,
            metadata_fingerprint: None,
        };
        let sql = format!(
            "INSERT INTO {}.{}(v) VALUES($1)",
            quote_identifier(&quoted.schema),
            quote_identifier(&quoted.table)
        );
        assert!(validate_guarded(&quoted, &[statement(&sql, vec![None])], limits).is_ok());
    }
    #[test]
    fn validation_is_complete_before_admission() {
        let limits = Limits::default();
        assert!(
            validate(
                &[statement("INSERT INTO t(v) VALUES($1::text)", vec![None])],
                limits
            )
            .is_ok()
        );
        for sql in [
            "COMMIT",
            "SET ROLE other",
            "SELECT 1",
            "INSERT INTO t(v) VALUES($1); COMMIT",
            "UPDATE t SET v=$2",
        ] {
            assert!(
                validate(&[statement(sql, vec![Some("a".into())])], limits).is_err(),
                "{sql}"
            );
        }
        assert!(
            validate(
                &[statement("DELETE FROM t", vec![Some("unused".into())])],
                limits
            )
            .is_err()
        );
        assert!(
            validate(
                &[statement(
                    "INSERT INTO t(v) VALUES($1)",
                    vec![Some("bad\0text".into())]
                )],
                limits
            )
            .is_err()
        );
        assert!(
            validate(
                &[statement(
                    "INSERT INTO t(v) VALUES($1)",
                    vec![Some("x".repeat(MAX_BATCH_PARAMETER_BYTES + 1))]
                )],
                limits
            )
            .is_err()
        );
        assert!(validate(&[], limits).is_err());
        let secret = "never-log-this-value";
        assert!(
            !format!(
                "{:?}",
                statement("INSERT INTO t(v) VALUES($1)", vec![Some(secret.into())])
            )
            .contains(secret)
        );
        assert!(!format!("{:?}", BoundText(&Some(secret.into()))).contains(secret));
    }
}
