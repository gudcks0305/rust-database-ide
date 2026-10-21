use super::*;
#[derive(Clone)]
pub(super) enum ProfileMutation {
    Upsert(ConnectionProfile),
    Remove(String),
    Select(String),
    PersistHistory(bool, Vec<HistoryEntry>),
    History(HistoryEntry),
}
impl ProfileMutation {
    fn apply(self, state: &mut WorkspaceState) -> Result<(), dbide_profiles::StoreError> {
        match self {
            Self::Upsert(profile) => state.upsert(profile),
            Self::Remove(id) => {
                state.remove(&id);
                Ok(())
            }
            Self::Select(id) => state.select(Some(&id)),
            Self::PersistHistory(enabled, history) => {
                state.persist_history = enabled;
                if enabled {
                    for entry in history {
                        if !state.history.contains(&entry) {
                            state.record_history(entry)?;
                        }
                    }
                } else {
                    state.history.clear();
                }
                Ok(())
            }
            Self::History(entry) => {
                if state.persist_history {
                    state.record_history(entry)?;
                }
                Ok(())
            }
        }
    }
}
pub(super) struct ProfileWrite {
    pub mutation: ProfileMutation,
    pub activate: Option<(ConnectionRequest, bool, bool)>,
}
impl Workbench {
    pub(super) fn persist_profile_mutation(
        &mut self,
        mutation: ProfileMutation,
        cx: &mut Context<Self>,
    ) {
        self.profile_writes.push_back(ProfileWrite {
            mutation,
            activate: None,
        });
        self.start_profile_write(cx);
    }
    pub(super) fn start_profile_write(&mut self, cx: &mut Context<Self>) {
        if self.profile_write_busy {
            return;
        }
        let Some(write) = self.profile_writes.pop_front() else {
            return;
        };
        let Some(store) = self.store.clone() else {
            self.profile_writes.clear();
            self.form_error = Some(
                "Profile storage unavailable. Resolve the local store error before saving.".into(),
            );
            cx.notify();
            return;
        };
        self.profile_write_busy = true;
        cx.spawn(async move|view,cx|{
            let mutation=write.mutation;
            let result=cx.background_executor().spawn(async move{store.update(|state|mutation.apply(state))}).await;
            let _=view.update(cx,|s,cx|{
                s.profile_write_busy=false;
                match result {
                    Ok(mut fresh)=>{
                        // Keep session-only SQL in memory, and apply queued local mutations over the
                        // fresh cross-process state. Never send that whole overlay back to disk.
                        for entry in &s.workspace.history{if !fresh.history.contains(entry){fresh.history.push(entry.clone());}}
                        if fresh.history.len()>dbide_profiles::MAX_HISTORY{fresh.history.drain(..fresh.history.len()-dbide_profiles::MAX_HISTORY);}
                        for queued in &s.profile_writes{match &queued.mutation {ProfileMutation::PersistHistory(enabled,_)=>fresh.persist_history = *enabled,ProfileMutation::History(_)=>{},mutation=>{let _=mutation.clone().apply(&mut fresh);}}}
                        s.workspace=fresh;
                        if let Some((request,connect,remember))=write.activate {
                            if remember&&request.profile.engine!=DatabaseEngine::Sqlite{s.save_credential(request.profile.id.clone(),request.password.clone(),cx);}
                            if s.configure(request,cx){s.connect_after_configuration=connect;s.form=None;s.form_error=None;s.status="Profile saved · password remains session-only unless explicitly remembered".into();}
                            else{s.form_error=Some("Profile saved. Resolve the current transaction or Disconnect before applying these settings.".into());}
                        }
                    },
                    Err(error)=>{
                        s.form_error=Some(format!("Profile change was not saved: {error}"));
                        s.diagnostic=Some(Diagnostic::new(("Profile storage failed".into(),error.to_string(),"The existing profile file was preserved. Resolve the error and retry the explicit change.".into())));
                    },
                }
                s.start_profile_write(cx);cx.notify();
            });
        }).detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn bounded_gui_update_preserves_profiles_added_by_another_writer() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(dir.path().join("profiles.json")).unwrap();
        let first = ConnectionProfile::fixture();
        store.update(|s| s.upsert(first.clone())).unwrap();
        let _stale = store.load().unwrap();
        let mut external = ConnectionProfile::new();
        external.name = "CLI-added".into();
        store.update(|s| s.upsert(external.clone())).unwrap();
        let mut edited = first.clone();
        edited.name = "GUI-edited".into();
        let fresh = store
            .update(|s| ProfileMutation::Upsert(edited.clone()).apply(s))
            .unwrap();
        assert_eq!(fresh.profiles.len(), 2);
        assert!(fresh.profiles.contains(&external));
        assert!(fresh.profiles.contains(&edited));
        let fresh = store
            .update(|s| ProfileMutation::Select(first.id.clone()).apply(s))
            .unwrap();
        assert!(fresh.profiles.contains(&external));
        let fresh = store
            .update(|s| ProfileMutation::Remove(first.id.clone()).apply(s))
            .unwrap();
        assert_eq!(fresh.profiles, vec![external]);
    }
    #[::core::prelude::v1::test]
    fn session_history_mutation_never_turns_on_persistence() {
        let entry = HistoryEntry {
            profile_id: "owned".into(),
            sql: "SELECT 1".into(),
            executed_at_unix_ms: 1,
            outcome: "Completed".into(),
            row_count: Some(1),
        };
        let mut state = WorkspaceState::default();
        ProfileMutation::History(entry.clone())
            .apply(&mut state)
            .unwrap();
        assert!(state.history.is_empty());
        assert!(!state.persist_history);
        ProfileMutation::PersistHistory(true, vec![entry.clone()])
            .apply(&mut state)
            .unwrap();
        assert_eq!(state.history, vec![entry]);
        ProfileMutation::PersistHistory(false, vec![])
            .apply(&mut state)
            .unwrap();
        assert!(state.history.is_empty());
        assert!(!state.persist_history);
    }
}
