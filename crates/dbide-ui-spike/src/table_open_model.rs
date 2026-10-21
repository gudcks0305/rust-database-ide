//! Captured intent for the isolated legacy PostgreSQL table reader.
//! No console, profile-store mutation, SQL dispatch, credential persistence or replay.
use crate::navigator::{Key, Relation, Scope};
use dbide_bridge::{OperationId, ServerTransactionState};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine, StoredExecutionTarget};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenPhase {
    Unsubmitted,
    Configuring(OperationId),
    Connecting(OperationId),
    Observing(OperationId),
    Ready,
    Closed,
}
pub struct PendingTableOpen {
    key: Key,
    request: ConnectionRequest,
    relation: Relation,
    phase: OpenPhase,
}
impl PendingTableOpen {
    pub fn new(key: Key, request: ConnectionRequest, relation: Relation) -> Result<Self, String> {
        if request.profile.engine != DatabaseEngine::PostgreSql
            || relation.kind != "table"
            || key.scope != Scope::Columns(relation.clone())
        {
            return Err("The paged reader requires an exact PostgreSQL table branch.".into());
        }
        let target = StoredExecutionTarget::capture(&request.profile, &request.profile.database)
            .map_err(|e| e.to_string())?;
        if key.profile_id != target.profile_id
            || key.fingerprint != target.endpoint_fingerprint
            || key.database != target.database
        {
            return Err("The table branch and captured connection target differ.".into());
        }
        Ok(Self {
            key,
            request,
            relation,
            phase: OpenPhase::Unsubmitted,
        })
    }
    pub fn key(&self) -> &Key {
        &self.key
    }
    pub fn request(&self) -> &ConnectionRequest {
        &self.request
    }
    pub fn relation(&self) -> &Relation {
        &self.relation
    }
    pub fn phase(&self) -> OpenPhase {
        self.phase
    }
    pub fn configure(&mut self, operation: OperationId) -> bool {
        if self.phase != OpenPhase::Unsubmitted {
            return false;
        }
        self.phase = OpenPhase::Configuring(operation);
        true
    }
    pub fn waiting_configured(&self, operation: OperationId) -> bool {
        self.phase == OpenPhase::Configuring(operation)
    }
    pub fn connect(&mut self, configured: OperationId, operation: OperationId) -> bool {
        if !self.waiting_configured(configured) {
            return false;
        }
        self.phase = OpenPhase::Connecting(operation);
        true
    }
    pub fn waiting_connected(&self, operation: OperationId) -> bool {
        self.phase == OpenPhase::Connecting(operation)
    }
    pub fn observe(&mut self, connected: OperationId, operation: OperationId) -> bool {
        if !self.waiting_connected(connected) {
            return false;
        }
        self.phase = OpenPhase::Observing(operation);
        true
    }
    pub fn observed(
        &mut self,
        operation: OperationId,
        state: ServerTransactionState,
    ) -> Result<bool, String> {
        if self.phase != OpenPhase::Observing(operation) {
            return Ok(false);
        }
        if state != ServerTransactionState::Idle {
            self.phase = OpenPhase::Closed;
            return Err("The table connection did not confirm Idle; resolve its transaction or disconnect before retrying.".into());
        }
        self.phase = OpenPhase::Ready;
        Ok(true)
    }
    pub fn owns(&self, operation: OperationId) -> bool {
        matches!(self.phase,OpenPhase::Configuring(expected)|OpenPhase::Connecting(expected)|OpenPhase::Observing(expected) if expected==operation)
    }
    pub fn close(&mut self) {
        self.phase = OpenPhase::Closed;
    }
    pub fn ready_for(
        &self,
        branch_current: bool,
        profile: Option<&ConnectionProfile>,
        connected: bool,
        busy: bool,
        state: ServerTransactionState,
    ) -> bool {
        self.phase == OpenPhase::Ready
            && branch_current
            && profile == Some(&self.request.profile)
            && connected
            && !busy
            && state == ServerTransactionState::Idle
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn pending() -> PendingTableOpen {
        let profile = ConnectionProfile::fixture();
        let target = StoredExecutionTarget::capture(&profile, &profile.database).unwrap();
        let relation = Relation {
            schema: "public".into(),
            name: "items".into(),
            kind: "table".into(),
        };
        let key = Key {
            profile_id: profile.id.clone(),
            fingerprint: target.endpoint_fingerprint,
            generation: 7,
            database: profile.database.clone(),
            scope: Scope::Columns(relation.clone()),
        };
        PendingTableOpen::new(
            key,
            ConnectionRequest {
                profile,
                password: "not-serialized".into(),
            },
            relation,
        )
        .unwrap()
    }
    #[test]
    fn only_matching_configure_connect_and_policy_ack_can_open() {
        let mut p = pending();
        assert!(p.configure(OperationId(10)));
        assert!(!p.connect(OperationId(9), OperationId(11)));
        assert_eq!(p.phase(), OpenPhase::Configuring(OperationId(10)));
        assert!(p.connect(OperationId(10), OperationId(11)));
        assert!(!p.observe(OperationId(10), OperationId(12)));
        assert!(p.observe(OperationId(11), OperationId(12)));
        assert!(
            !p.observed(OperationId(11), ServerTransactionState::Idle)
                .unwrap()
        );
        assert!(!p.ready_for(
            true,
            Some(&p.request.profile),
            true,
            false,
            ServerTransactionState::Idle
        ));
        assert!(
            p.observed(OperationId(12), ServerTransactionState::Idle)
                .unwrap()
        );
        assert!(p.ready_for(
            true,
            Some(&p.request.profile),
            true,
            false,
            ServerTransactionState::Idle
        ));
    }
    #[test]
    fn stale_branch_wrong_database_busy_or_disconnected_cannot_open() {
        let mut p = pending();
        p.configure(OperationId(1));
        p.connect(OperationId(1), OperationId(2));
        p.observe(OperationId(2), OperationId(3));
        p.observed(OperationId(3), ServerTransactionState::Idle)
            .unwrap();
        let mut wrong = p.request.profile.clone();
        wrong.database = "another_database".into();
        for (current, profile, connected, busy, state) in [
            (
                false,
                &p.request.profile,
                true,
                false,
                ServerTransactionState::Idle,
            ),
            (true, &wrong, true, false, ServerTransactionState::Idle),
            (
                true,
                &p.request.profile,
                false,
                false,
                ServerTransactionState::Idle,
            ),
            (
                true,
                &p.request.profile,
                true,
                true,
                ServerTransactionState::Idle,
            ),
            (
                true,
                &p.request.profile,
                true,
                false,
                ServerTransactionState::Unknown,
            ),
        ] {
            assert!(!p.ready_for(current, Some(profile), connected, busy, state));
        }
    }
    #[test]
    fn cancellation_and_non_idle_state_never_replay_or_late_open() {
        for state in [
            ServerTransactionState::Active,
            ServerTransactionState::Failed,
            ServerTransactionState::Unknown,
        ] {
            let mut p = pending();
            p.configure(OperationId(1));
            p.connect(OperationId(1), OperationId(2));
            p.observe(OperationId(2), OperationId(3));
            assert!(p.observed(OperationId(3), state).is_err());
            assert!(
                !p.observed(OperationId(3), ServerTransactionState::Idle)
                    .unwrap()
            );
            assert_eq!(p.phase(), OpenPhase::Closed);
        }
        let mut p = pending();
        p.configure(OperationId(1));
        p.close();
        assert!(!p.connect(OperationId(1), OperationId(2)));
        assert!(!p.owns(OperationId(1)));
    }
    #[test]
    fn capture_rejects_endpoint_database_relation_and_engine_substitution() {
        let p = pending();
        let mut request = p.request.clone();
        request.profile.host = "other.invalid".into();
        assert!(PendingTableOpen::new(p.key.clone(), request, p.relation.clone()).is_err());
        let mut key = p.key.clone();
        key.database = "another".into();
        assert!(PendingTableOpen::new(key, p.request.clone(), p.relation.clone()).is_err());
        let mut relation = p.relation.clone();
        relation.name = "other".into();
        assert!(PendingTableOpen::new(p.key.clone(), p.request.clone(), relation).is_err());
        let mut request = p.request.clone();
        request.profile.engine = DatabaseEngine::MySql;
        assert!(PendingTableOpen::new(p.key.clone(), request, p.relation.clone()).is_err());
    }
}
