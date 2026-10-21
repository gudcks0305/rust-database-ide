//! One console owns one lazy physical session. Routes remain immutable during an operation.
use crate::workbench_model::ResultOwner;
use dbide_bridge::{
    BridgeEvent, CancelTarget, CommitPolicy, DatabaseBridge, OperationId, SchemaObject,
    ServerTransactionState,
};
use dbide_pg_spike::{ExecutionKey, ExecutionReport, Outcome};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine, StoredExecutionTarget};

pub const MAX_CONSOLE_WORKERS: usize = 8;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConsoleRoute {
    pub console_id: u64,
    pub generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    Disconnected,
    Connecting,
    Connected,
    Executing,
    Cancelling,
    Disconnecting,
}
impl SessionPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting…",
            Self::Connected => "Connected",
            Self::Executing => "Executing…",
            Self::Cancelling => "Cancelling…",
            Self::Disconnecting => "Disconnecting…",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    Connection,
    Query,
    Metadata,
}
#[derive(Clone, Debug)]
pub struct SessionDiagnostic {
    pub title: String,
    pub message: String,
    pub hint: String,
    pub kind: DiagnosticKind,
}
pub enum ConsoleEffect {
    Finished {
        owner: ResultOwner,
        sql: Option<String>,
        report: ExecutionReport,
        target: Option<StoredExecutionTarget>,
    },
}
enum Intent {
    Databases,
    Connect,
    Run { sql: String, owner: ResultOwner },
}
/// No Debug/Serialize implementation: session-only credentials never enter recovery/logs.
pub struct ConsoleSession {
    route: ConsoleRoute,
    request: Option<ConnectionRequest>,
    bridge: Option<DatabaseBridge>,
    configured: bool,
    config_operation: Option<OperationId>,
    connection_operation: Option<OperationId>,
    policy_operation: Option<OperationId>,
    cancel_operation: Option<OperationId>,
    disconnect_operation: Option<OperationId>,
    schema_operation: Option<OperationId>,
    database_operation: Option<OperationId>,
    pub databases: Vec<dbide_bridge::DatabaseInfo>,
    pub databases_truncated: bool,
    intent: Option<Intent>,
    pending_owner: Option<ResultOwner>,
    pending_sql: Option<String>,
    pending_target: Option<StoredExecutionTarget>,
    was_connected_before_operation: bool,
    pub phase: SessionPhase,
    pub commit_policy: CommitPolicy,
    pub transaction_state: ServerTransactionState,
    pub active_operation: Option<OperationId>,
    pub active_execution: Option<ExecutionKey>,
    pub schema: Vec<SchemaObject>,
    pub schema_truncated: bool,
    pub diagnostic: Option<SessionDiagnostic>,
    pub status: String,
}
impl ConsoleSession {
    pub fn new(console_id: u64) -> Self {
        Self {
            route: ConsoleRoute {
                console_id,
                generation: 1,
            },
            request: None,
            bridge: None,
            configured: false,
            config_operation: None,
            connection_operation: None,
            policy_operation: None,
            cancel_operation: None,
            disconnect_operation: None,
            schema_operation: None,
            database_operation: None,
            databases: vec![],
            databases_truncated: false,
            intent: None,
            pending_owner: None,
            pending_sql: None,
            pending_target: None,
            was_connected_before_operation: false,
            phase: SessionPhase::Disconnected,
            commit_policy: CommitPolicy::Auto,
            transaction_state: ServerTransactionState::Unknown,
            active_operation: None,
            active_execution: None,
            schema: vec![],
            schema_truncated: false,
            diagnostic: None,
            status: "Select a connection and database for this console".into(),
        }
    }
    pub fn route(&self) -> ConsoleRoute {
        self.route
    }
    pub fn target(&self) -> Option<&ConnectionProfile> {
        self.request.as_ref().map(|r| &r.profile)
    }
    pub fn stored_target(&self) -> Option<StoredExecutionTarget> {
        let profile = self.target()?;
        StoredExecutionTarget::capture(
            profile,
            if profile.engine == DatabaseEngine::Sqlite {
                "main"
            } else {
                &profile.database
            },
        )
        .ok()
    }
    pub fn request(&self) -> Option<ConnectionRequest> {
        self.request.clone()
    }
    pub fn has_worker(&self) -> bool {
        self.bridge.is_some()
    }
    pub fn busy(&self) -> bool {
        self.config_operation.is_some()
            || self.policy_operation.is_some()
            || self.active_operation.is_some()
            || self.connection_operation.is_some()
            || self.disconnect_operation.is_some()
            || self.intent.is_some()
            || self.database_operation.is_some()
            || self.schema_operation.is_some()
    }
    pub fn may_have_transaction(&self) -> bool {
        crate::workbench_model::transaction_may_be_open(
            self.transaction_state,
            self.phase == SessionPhase::Connected,
        )
    }
    pub fn can_change_target(&self) -> bool {
        !self.busy() && !self.may_have_transaction()
    }
    pub fn bind(&mut self, route: ConsoleRoute, request: ConnectionRequest) -> Result<(), String> {
        self.check_route(route)?;
        request.profile.validate().map_err(|e| e.to_string())?;
        if !self.can_change_target() {
            return Err("Commit/Rollback or explicitly Disconnect this console before changing its target. Other consoles are unchanged.".into());
        }
        let next = self
            .route
            .generation
            .checked_add(1)
            .ok_or("Console generation exhausted; open a new console")?;
        self.bridge = None;
        self.configured = false;
        self.route.generation = next;
        self.request = Some(request);
        self.commit_policy = CommitPolicy::Auto;
        self.phase = SessionPhase::Disconnected;
        self.transaction_state = ServerTransactionState::Unknown;
        self.schema.clear();
        self.databases.clear();
        self.database_operation = None;
        self.schema_truncated = false;
        self.schema_operation = None;
        self.diagnostic = None;
        self.status = "Target pinned to this console · Auto commit · not connected".into();
        Ok(())
    }
    fn check_route(&self, route: ConsoleRoute) -> Result<(), String> {
        if self.route != route {
            Err("Console context changed; action was not submitted".into())
        } else {
            Ok(())
        }
    }
    fn start(
        &mut self,
        route: ConsoleRoute,
        intent: Intent,
        slot_available: bool,
    ) -> Result<(), String> {
        self.check_route(route)?;
        if self.busy() {
            return Err("This console already has a pending operation".into());
        }
        let request = self
            .request
            .clone()
            .ok_or("Select this console's connection and database first")?;
        if self.bridge.is_none() {
            if !slot_available {
                return Err(format!(
                    "At most {MAX_CONSOLE_WORKERS} console sessions can be live. Disconnect a console explicitly; idle sessions are never evicted."
                ));
            }
            self.bridge = Some(DatabaseBridge::new().map_err(|e| e.to_string())?);
            self.configured = false;
        }
        self.diagnostic = None;
        if !self.configured {
            let operation = self
                .bridge
                .as_ref()
                .unwrap()
                .configure(request)
                .map_err(|e| format!("Configure was not submitted: {e:?}"))?;
            self.config_operation = Some(operation);
            self.intent = Some(intent);
            self.phase = SessionPhase::Connecting;
            self.status = "Preparing this console's captured target…".into();
            Ok(())
        } else {
            self.dispatch(intent)
        }
    }
    fn dispatch(&mut self, intent: Intent) -> Result<(), String> {
        let bridge = self
            .bridge
            .as_ref()
            .ok_or("Console worker is unavailable")?;
        match intent {
            Intent::Databases => {
                self.database_operation = Some(
                    bridge
                        .list_databases()
                        .map_err(|e| format!("Database discovery was not submitted: {e:?}"))?,
                );
                self.phase = if self.phase == SessionPhase::Connecting {
                    SessionPhase::Disconnected
                } else {
                    self.phase
                };
                self.status = "Loading advertised databases (separate metadata probe)…".into();
            }
            Intent::Connect => {
                let operation = bridge
                    .connect()
                    .map_err(|e| format!("Connect was not submitted: {e:?}"))?;
                self.connection_operation = Some(operation);
                self.phase = SessionPhase::Connecting;
                self.status = "Connecting this console…".into();
            }
            Intent::Run { sql, owner } => {
                self.was_connected_before_operation = self.phase == SessionPhase::Connected;
                let operation = bridge
                    .execute(sql.clone())
                    .map_err(|e| format!("Query was not submitted: {e:?}"))?;
                self.active_operation = Some(operation);
                self.pending_target = self.stored_target();
                self.pending_owner = Some(owner);
                self.pending_sql = Some(sql);
                self.phase = SessionPhase::Executing;
                self.status = "Query submitted on this console only".into();
            }
        }
        Ok(())
    }
    pub fn discover_databases(
        &mut self,
        route: ConsoleRoute,
        slot_available: bool,
    ) -> Result<(), String> {
        self.start(route, Intent::Databases, slot_available)
    }
    pub fn connect(&mut self, route: ConsoleRoute, slot_available: bool) -> Result<(), String> {
        self.check_route(route)?;
        if self.phase == SessionPhase::Connected {
            return Ok(());
        }
        self.start(route, Intent::Connect, slot_available)
    }
    pub fn execute(
        &mut self,
        route: ConsoleRoute,
        sql: String,
        owner: ResultOwner,
        slot_available: bool,
    ) -> Result<(), String> {
        if sql.trim().is_empty() {
            return Err("SQL is empty".into());
        }
        if owner.document_id != self.route.console_id {
            return Err("Query owner does not match this console".into());
        }
        if self.target().is_none_or(|profile| {
            profile.id != owner.profile_id
                || crate::workbench_model::profile_target(profile) != owner.target
        }) {
            return Err("Query owner target does not match this console".into());
        }
        self.start(route, Intent::Run { sql, owner }, slot_available)
    }
    pub fn set_policy(&mut self, route: ConsoleRoute, policy: CommitPolicy) -> Result<(), String> {
        self.check_route(route)?;
        if self.busy() || self.phase != SessionPhase::Connected || self.may_have_transaction() {
            return Err("Resolve this console's transaction before changing commit policy".into());
        }
        let operation = self
            .bridge
            .as_ref()
            .ok_or("Connect this console first")?
            .set_commit_policy(policy)
            .map_err(|e| format!("Policy was not submitted: {e:?}"))?;
        self.policy_operation = Some(operation);
        Ok(())
    }
    pub fn transaction(
        &mut self,
        route: ConsoleRoute,
        commit: bool,
        owner: ResultOwner,
    ) -> Result<(), String> {
        self.check_route(route)?;
        if owner.document_id != self.route.console_id {
            return Err("Transaction owner does not match this console".into());
        }
        if self.busy()
            || self.phase != SessionPhase::Connected
            || !self.may_have_transaction()
            || (commit && self.transaction_state != ServerTransactionState::Active)
        {
            return Err("Transaction control is unavailable in this console state".into());
        }
        let bridge = self
            .bridge
            .as_ref()
            .ok_or("Existing console session required")?;
        let operation = if commit {
            bridge.commit()
        } else {
            bridge.rollback()
        }
        .map_err(|e| format!("Transaction control was not submitted: {e:?}"))?;
        self.was_connected_before_operation = self.phase == SessionPhase::Connected;
        self.active_operation = Some(operation);
        self.pending_target = self.stored_target();
        self.pending_owner = Some(owner);
        self.pending_sql = None;
        self.phase = SessionPhase::Executing;
        Ok(())
    }
    pub fn cancel_target(&self) -> Option<CancelTarget> {
        self.active_execution
            .map(CancelTarget::Execution)
            .or_else(|| {
                self.active_operation
                    .or(self.connection_operation)
                    .map(CancelTarget::Operation)
            })
    }
    pub fn cancel(&mut self, route: ConsoleRoute, target: CancelTarget) -> Result<(), String> {
        self.check_route(route)?;
        let current = match target {
            CancelTarget::Execution(key) => self.active_execution == Some(key),
            CancelTarget::Operation(op) => {
                self.active_operation == Some(op) || self.connection_operation == Some(op)
            }
        };
        if !current {
            return Err("Cancellation target is stale; nothing was cancelled".into());
        }
        let operation = self
            .bridge
            .as_ref()
            .ok_or("Session is disconnected")?
            .cancel(target)
            .map_err(|e| format!("Cancel was not submitted: {e:?}"))?;
        self.cancel_operation = Some(operation);
        self.phase = SessionPhase::Cancelling;
        self.status = "Cancellation requested for this console execution".into();
        Ok(())
    }
    pub fn disconnect(&mut self, route: ConsoleRoute) -> Result<(), String> {
        self.check_route(route)?;
        self.intent = None;
        if let Some(bridge) = &self.bridge {
            let operation = bridge
                .disconnect()
                .map_err(|e| format!("Disconnect was not submitted: {e:?}"))?;
            self.disconnect_operation = Some(operation);
            self.phase = SessionPhase::Disconnecting;
            self.status = "Disconnecting this console without committing".into();
        } else {
            self.phase = SessionPhase::Disconnected;
            self.transaction_state = ServerTransactionState::Unknown;
        }
        Ok(())
    }
    fn fail(&mut self, title: &str, message: String, kind: DiagnosticKind) {
        self.diagnostic=Some(SessionDiagnostic{title:title.into(),message,hint:match kind{DiagnosticKind::Connection=>"Check this console's pinned target and session password; retry explicitly.",DiagnosticKind::Query=>"Review SQL; Rollback resolves Failed/Unknown live transactions. No SQL is automatically replayed.",DiagnosticKind::Metadata=>"Refresh metadata explicitly. SQL execution state is unchanged."}.into(),kind});
    }
    fn refresh_schema(&mut self) {
        if self.schema_operation.is_none()
            && let Some(bridge) = &self.bridge
        {
            self.schema_operation = bridge.inspect_schema().ok();
        }
    }
    pub fn poll_one(&mut self) -> Option<Vec<ConsoleEffect>> {
        let event = self.bridge.as_mut()?.try_next()?;
        Some(self.handle_event(self.route, event))
    }
    /// An envelope route is mandatory even when operation counters happen to be equal.
    pub fn handle_event(&mut self, route: ConsoleRoute, event: BridgeEvent) -> Vec<ConsoleEffect> {
        if route != self.route {
            return vec![];
        }
        let mut effects = vec![];
        match event {
            BridgeEvent::Configured { operation } if self.config_operation == Some(operation) => {
                self.config_operation = None;
                self.configured = true;
                if let Some(intent) = self.intent.take()
                    && let Err(error) = self.dispatch(intent)
                {
                    self.phase = SessionPhase::Disconnected;
                    self.fail("Operation not submitted", error, DiagnosticKind::Connection);
                }
            }
            BridgeEvent::Connecting {
                operation, attempt, ..
            } if self.active_operation == Some(operation)
                || self.connection_operation == Some(operation) =>
            {
                self.status = format!("Connecting this console · attempt {attempt}");
            }
            BridgeEvent::Connected { operation }
                if self.active_operation == Some(operation)
                    || self.connection_operation == Some(operation) =>
            {
                if self.connection_operation == Some(operation) {
                    self.connection_operation = None;
                    self.phase = SessionPhase::Connected;
                    if let Some(bridge) = &self.bridge {
                        self.policy_operation = bridge.set_commit_policy(self.commit_policy).ok();
                    }
                }
                self.status = "Console connected".into();
            }
            BridgeEvent::Executing {
                operation,
                execution,
            } if self.active_operation == Some(operation) => {
                self.active_execution = Some(execution);
                if self.disconnect_operation.is_none() {
                    self.phase = SessionPhase::Executing;
                }
                self.status = format!("Console query {} running", execution.execution);
            }
            BridgeEvent::Cancelling { execution, .. }
                if self.active_execution == Some(execution) =>
            {
                self.phase = SessionPhase::Cancelling;
            }
            BridgeEvent::Finished { operation, report }
                if self.active_operation == Some(operation)
                    && self.active_execution.is_none_or(|key| key == report.key) =>
            {
                self.active_operation = None;
                self.active_execution = None;
                self.cancel_operation = None;
                self.commit_policy = report.commit_policy;
                self.transaction_state = report.transaction_state;
                self.phase = if self.disconnect_operation.is_some() {
                    SessionPhase::Disconnecting
                } else if report.connection_retired {
                    SessionPhase::Disconnected
                } else {
                    SessionPhase::Connected
                };
                self.status = format!(
                    "Last operation: {:?} · transaction {:?}",
                    report.outcome, report.transaction_state
                );
                if let Some(error) = &report.terminal_error {
                    self.fail(
                        if report.outcome == Outcome::Cancelled {
                            "Query cancelled"
                        } else {
                            "Query failed"
                        },
                        error.message.clone(),
                        DiagnosticKind::Query,
                    );
                } else if report.outcome == Outcome::Completed {
                    self.diagnostic = None;
                }
                if let Some(owner) = self.pending_owner.take() {
                    effects.push(ConsoleEffect::Finished {
                        owner,
                        sql: self.pending_sql.take(),
                        report,
                        target: self.pending_target.take(),
                    });
                }
                if self.phase == SessionPhase::Connected && self.schema.is_empty() {
                    self.refresh_schema();
                }
            }
            BridgeEvent::CommitPolicyChanged {
                operation,
                policy,
                transaction_state,
            } if self.policy_operation == Some(operation) => {
                self.policy_operation = None;
                self.commit_policy = policy;
                self.transaction_state = transaction_state;
                self.status = format!("Console {:?} · transaction {:?}", policy, transaction_state);
                if self.schema.is_empty() {
                    self.refresh_schema();
                }
            }
            BridgeEvent::DatabasesLoaded {
                operation,
                databases,
                truncated,
            } if self.database_operation == Some(operation) => {
                self.database_operation = None;
                self.databases = databases;
                self.databases_truncated = truncated;
                self.status = format!(
                    "{} advertised databases{}",
                    self.databases.len(),
                    if truncated {
                        " · subset limit reached"
                    } else {
                        ""
                    }
                );
            }
            BridgeEvent::SchemaLoaded {
                operation,
                objects,
                truncated,
            } if self.schema_operation == Some(operation) => {
                self.schema_operation = None;
                self.schema = objects;
                self.schema_truncated = truncated;
            }
            BridgeEvent::ConnectionFailed {
                operation,
                diagnostic,
            } => {
                if self.database_operation == Some(operation) {
                    self.database_operation = None;
                    self.fail(
                        "Database list unavailable",
                        format!("{} · {}", diagnostic.message, diagnostic.hint),
                        DiagnosticKind::Metadata,
                    );
                } else if self.schema_operation == Some(operation) {
                    self.schema_operation = None;
                    self.fail(
                        "Metadata unavailable",
                        format!("{} · {}", diagnostic.message, diagnostic.hint),
                        DiagnosticKind::Metadata,
                    );
                } else if self.config_operation == Some(operation)
                    || self.connection_operation == Some(operation)
                    || self.active_operation == Some(operation)
                {
                    self.config_operation = None;
                    self.connection_operation = None;
                    self.active_operation = None;
                    self.active_execution = None;
                    self.intent = None;
                    self.pending_sql = None;
                    self.pending_owner = None;
                    self.phase = SessionPhase::Disconnected;
                    self.transaction_state = ServerTransactionState::Unknown;
                    self.fail(
                        "Connection failed",
                        format!("{} · {}", diagnostic.message, diagnostic.hint),
                        DiagnosticKind::Connection,
                    );
                }
            }
            BridgeEvent::Disconnected { operation }
                if self.disconnect_operation == operation
                    && self.disconnect_operation.is_some() =>
            {
                self.disconnect_operation = None;
                self.config_operation = None;
                self.connection_operation = None;
                self.policy_operation = None;
                self.schema_operation = None;
                self.database_operation = None;
                self.active_operation = None;
                self.active_execution = None;
                self.cancel_operation = None;
                self.intent = None;
                self.pending_owner = None;
                self.pending_sql = None;
                self.bridge = None;
                self.configured = false;
                self.phase = SessionPhase::Disconnected;
                self.transaction_state = ServerTransactionState::Unknown;
                self.status = "Console disconnected · no transaction committed".into();
            }
            BridgeEvent::Error {
                operation: Some(operation),
                message,
            } => {
                if self.database_operation == Some(operation) {
                    self.database_operation = None;
                    self.fail(
                        "Database list unavailable",
                        message,
                        DiagnosticKind::Metadata,
                    );
                } else if self.schema_operation == Some(operation) {
                    self.schema_operation = None;
                    self.fail("Metadata unavailable", message, DiagnosticKind::Metadata);
                } else if self.cancel_operation == Some(operation) {
                    self.cancel_operation = None;
                } else if self.policy_operation == Some(operation) {
                    self.policy_operation = None;
                    self.fail("Policy unchanged", message, DiagnosticKind::Query);
                } else if self.active_operation == Some(operation)
                    || self.connection_operation == Some(operation)
                    || self.config_operation == Some(operation)
                {
                    let may_be_alive = self.active_operation == Some(operation)
                        && (self.was_connected_before_operation || self.active_execution.is_some());
                    self.active_operation = None;
                    self.connection_operation = None;
                    self.config_operation = None;
                    self.active_execution = None;
                    self.intent = None;
                    self.pending_owner = None;
                    self.pending_sql = None;
                    self.phase = if may_be_alive {
                        SessionPhase::Connected
                    } else {
                        SessionPhase::Disconnected
                    };
                    self.transaction_state = ServerTransactionState::Unknown;
                    self.fail("Console operation failed", message, DiagnosticKind::Query);
                }
            }
            _ => {}
        }
        effects
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> ConnectionRequest {
        ConnectionRequest {
            profile: ConnectionProfile::fixture(),
            password: "not-persisted".into(),
        }
    }
    #[test]
    fn bindings_are_snapshots_and_other_console_transactions_are_independent() {
        let mut a = ConsoleSession::new(1);
        let mut b = ConsoleSession::new(2);
        a.bind(a.route(), request()).unwrap();
        b.bind(b.route(), request()).unwrap();
        a.phase = SessionPhase::Connected;
        a.transaction_state = ServerTransactionState::Active;
        let old = a.route();
        let mut other = request();
        other.profile.database = "other".into();
        assert!(a.bind(old, other.clone()).is_err());
        b.bind(b.route(), other).unwrap();
        assert_eq!(a.target().unwrap().database, "dbide_fixture");
        assert_eq!(b.target().unwrap().database, "other");
        assert_eq!(a.transaction_state, ServerTransactionState::Active);
    }
    #[test]
    fn stale_console_generation_operation_and_cancellation_never_cross_sessions() {
        let mut a = ConsoleSession::new(1);
        let mut b = ConsoleSession::new(2);
        a.active_operation = Some(OperationId(7));
        b.active_operation = Some(OperationId(7));
        a.phase = SessionPhase::Executing;
        b.phase = SessionPhase::Executing;
        let event = BridgeEvent::Executing {
            operation: OperationId(7),
            execution: ExecutionKey {
                session: 1,
                generation: 1,
                execution: 4,
            },
        };
        b.handle_event(a.route(), event);
        assert!(b.active_execution.is_none());
        a.handle_event(
            ConsoleRoute {
                console_id: 1,
                generation: 0,
            },
            BridgeEvent::Connected {
                operation: OperationId(7),
            },
        );
        assert_eq!(a.phase, SessionPhase::Executing);
        assert!(
            a.cancel(b.route(), CancelTarget::Operation(OperationId(7)))
                .is_err()
        );
    }
    #[test]
    fn generation_exhaustion_and_unknown_live_transactions_fail_closed() {
        let mut s = ConsoleSession::new(u64::MAX);
        s.route.generation = u64::MAX;
        assert!(s.bind(s.route(), request()).is_err());
        s.route.generation = 1;
        s.phase = SessionPhase::Connected;
        s.transaction_state = ServerTransactionState::Unknown;
        assert!(!s.can_change_target());
        s.phase = SessionPhase::Disconnected;
        assert!(s.can_change_target());
    }
    #[test]
    fn worker_limit_never_evicts_or_allocates_on_target_selection() {
        let mut s = ConsoleSession::new(1);
        s.bind(s.route(), request()).unwrap();
        assert!(!s.has_worker());
        assert!(s.connect(s.route(), false).is_err());
        assert!(!s.has_worker());
        assert_eq!(s.phase, SessionPhase::Disconnected);
    }
    #[test]
    fn rebind_explicitly_resets_manual_policy_to_new_worker_auto_default() {
        let mut session = ConsoleSession::new(1);
        session.bind(session.route(), request()).unwrap();
        session.commit_policy = CommitPolicy::Manual;
        session.bind(session.route(), request()).unwrap();
        assert_eq!(session.commit_policy, CommitPolicy::Auto);
        assert!(!session.has_worker());
        assert!(session.status.contains("Auto"));
    }
    #[test]
    fn transaction_owner_and_connected_route_are_checked_before_dispatch() {
        let mut session = ConsoleSession::new(1);
        session.phase = SessionPhase::Connected;
        session.transaction_state = ServerTransactionState::Active;
        let wrong = ConsoleRoute {
            console_id: 2,
            generation: 1,
        };
        assert!(session.connect(wrong, true).is_err());
        let owner = ResultOwner {
            document_id: 2,
            document_title: "other".into(),
            profile_id: "other".into(),
            profile_name: "other".into(),
            target: "other".into(),
        };
        assert!(
            session
                .transaction(session.route(), false, owner)
                .unwrap_err()
                .contains("owner")
        );
        assert_eq!(session.transaction_state, ServerTransactionState::Active);
    }
    #[test]
    fn unstructured_failure_never_unlocks_a_potentially_live_transaction() {
        let mut session = ConsoleSession::new(1);
        session.phase = SessionPhase::Executing;
        session.was_connected_before_operation = true;
        session.transaction_state = ServerTransactionState::Active;
        session.active_operation = Some(OperationId(7));
        session.handle_event(
            session.route(),
            BridgeEvent::Error {
                operation: Some(OperationId(7)),
                message: "rejected".into(),
            },
        );
        assert_eq!(session.phase, SessionPhase::Connected);
        assert_eq!(session.transaction_state, ServerTransactionState::Unknown);
        assert!(!session.can_change_target());
    }
}
