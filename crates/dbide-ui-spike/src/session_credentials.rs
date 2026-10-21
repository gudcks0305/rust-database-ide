//! Ephemeral browser credential cache. Profile IDs alone never authorize password reuse.
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine, StoredExecutionTarget};
use std::collections::HashMap;
/// Intentionally has no Debug/Serialize implementation. Values remain process-local.
#[derive(Default)]
pub struct SessionCredentials {
    values: HashMap<(String, String), String>,
}
impl SessionCredentials {
    fn key(profile: &ConnectionProfile) -> Option<(String, String)> {
        let target = StoredExecutionTarget::capture(
            profile,
            if profile.engine == DatabaseEngine::Sqlite {
                "main"
            } else {
                &profile.database
            },
        )
        .ok()?;
        Some((target.profile_id, target.endpoint_fingerprint))
    }
    /// Prefer explicitly supplied console credentials only for this exact endpoint.
    /// Empty drafts must not mask another matching console's entered password.
    pub fn matching_password(
        profile: &ConnectionProfile,
        requests: impl IntoIterator<Item = ConnectionRequest>,
    ) -> Option<String> {
        let expected = Self::key(profile)?;
        requests.into_iter().find_map(|request| {
            (!request.password.is_empty()
                && Self::key(&request.profile).as_ref() == Some(&expected))
            .then_some(request.password)
        })
    }
    pub fn remember(&mut self, request: &ConnectionRequest) {
        if let Some(key) = Self::key(&request.profile) {
            self.forget_profile(&key.0);
            self.values.insert(key, request.password.clone());
        }
    }
    pub fn get(&self, profile: &ConnectionProfile) -> Option<String> {
        self.values.get(&Self::key(profile)?).cloned()
    }
    pub fn forget_profile(&mut self, id: &str) {
        self.values.retain(|(profile_id, _), _| profile_id != id);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_id_endpoint_user_tls_engine_and_effective_options_cannot_reuse_secret() {
        let mut profile = ConnectionProfile::fixture();
        profile.tls_mode = dbide_profiles::TlsMode::VerifyFull;
        let mut credentials = SessionCredentials::default();
        credentials.remember(&ConnectionRequest {
            profile: profile.clone(),
            password: "fixture-secret-only".into(),
        });
        let mut variants = vec![];
        let mut changed = profile.clone();
        changed.host = "another.invalid".into();
        variants.push(changed);
        let mut changed = profile.clone();
        changed.username = "another_user".into();
        variants.push(changed);
        let mut changed = profile.clone();
        changed.port = 5433;
        variants.push(changed);
        let mut changed = profile.clone();
        changed.tls_mode = dbide_profiles::TlsMode::Disable;
        variants.push(changed);
        let mut changed = profile.clone();
        changed.root_cert_path = Some("/tmp/owned-ca.pem".into());
        variants.push(changed);
        let mut changed = profile.clone();
        changed.read_only = true;
        variants.push(changed);
        let mut changed = profile.clone();
        changed.connect_timeout_secs = 7;
        variants.push(changed);
        let mut changed = profile.clone();
        changed.engine = DatabaseEngine::MySql;
        variants.push(changed);
        for changed in variants {
            assert_eq!(changed.id, profile.id);
            assert!(credentials.get(&changed).is_none());
        }
    }
    #[test]
    fn cosmetic_name_and_default_database_changes_preserve_same_endpoint_credential() {
        let mut profile = ConnectionProfile::fixture();
        let mut credentials = SessionCredentials::default();
        credentials.remember(&ConnectionRequest {
            profile: profile.clone(),
            password: "fixture-secret-only".into(),
        });
        profile.name = "Renamed profile".into();
        profile.database = "another_owned_database".into();
        assert_eq!(
            credentials.get(&profile).as_deref(),
            Some("fixture-secret-only")
        );
        let original_id = profile.id.clone();
        profile.id = "different-profile-id".into();
        assert!(credentials.get(&profile).is_none());
        credentials.forget_profile(&original_id);
        profile.id = original_id;
        assert!(credentials.get(&profile).is_none());
    }
    #[test]
    fn remembering_new_endpoint_releases_old_cache_without_mutating_console_snapshot() {
        let profile = ConnectionProfile::fixture();
        let original = ConnectionRequest {
            profile: profile.clone(),
            password: "original-fixture-only".into(),
        };
        let mut console = crate::console_session::ConsoleSession::new(1);
        console.bind(console.route(), original.clone()).unwrap();
        let mut credentials = SessionCredentials::default();
        credentials.remember(&original);
        let mut next = original.clone();
        next.profile.host = "replacement.invalid".into();
        next.password = "replacement-fixture-only".into();
        credentials.remember(&next);
        assert!(credentials.get(&profile).is_none());
        assert_eq!(
            credentials.get(&next.profile).as_deref(),
            Some("replacement-fixture-only")
        );
        assert_eq!(console.request().unwrap().password, "original-fixture-only");
        assert_eq!(console.target().unwrap().host, profile.host);
    }
    #[test]
    fn browser_can_use_exact_console_password_but_empty_or_changed_endpoint_cannot_mask_it() {
        let profile = ConnectionProfile::fixture();
        let mut other = profile.clone();
        other.host = "other.invalid".into();
        let requests = vec![
            ConnectionRequest {
                profile: other,
                password: "foreign-secret".into(),
            },
            ConnectionRequest {
                profile: profile.clone(),
                password: String::new(),
            },
            ConnectionRequest {
                profile: profile.clone(),
                password: "entered-console-secret".into(),
            },
        ];
        let mut target = profile.clone();
        target.database = "another_database".into();
        assert_eq!(
            SessionCredentials::matching_password(&target, requests).as_deref(),
            Some("entered-console-secret")
        );
        target.username = "another_user".into();
        assert!(
            SessionCredentials::matching_password(
                &target,
                [ConnectionRequest {
                    profile,
                    password: "old-user-secret".into()
                }]
            )
            .is_none()
        );
    }
}
