use super::*;
use dbide_workspace::{DocumentSnapshot, WorkspaceSnapshot, WorkspaceStore};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    enabled: bool,
}
fn load_preferences(path: &std::path::Path) -> Result<Preferences, String> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Preferences::default()),
        Err(e) => return Err(e.to_string()),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Editor preferences must be a regular file.".into());
    }
    let mut bytes = vec![];
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4096 {
        return Err("Editor preferences exceed the size limit; original preserved.".into());
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| "Editor preferences malformed; original preserved.".into())
}
#[derive(Clone, PartialEq, Eq)]
struct DocumentStamp {
    id: u64,
    revision: u64,
    target_generation: u64,
    saved_pointer: usize,
    saved_length: usize,
    title: String,
    path: Option<PathBuf>,
}
#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    input_changes: u64,
    active: u64,
    documents: Vec<DocumentStamp>,
}
pub(super) struct Recovery {
    store: Option<WorkspaceStore>,
    preferences: Option<PathBuf>,
    pub enabled: bool,
    pub loading: bool,
    pub blocked: bool,
    busy: bool,
    pub preference_busy: bool,
    stamp: Option<Stamp>,
    saved: Option<Stamp>,
    changed: Instant,
    pub close_after: bool,
    pub status: String,
}
impl Default for Recovery {
    fn default() -> Self {
        Self {
            store: None,
            preferences: None,
            enabled: false,
            loading: true,
            blocked: false,
            busy: false,
            preference_busy: false,
            stamp: None,
            saved: None,
            changed: Instant::now(),
            close_after: false,
            status: "Loading editor recovery settings…".into(),
        }
    }
}
impl Workbench {
    fn recovery_stamp(&self) -> Stamp {
        Stamp {
            input_changes: self.input_changes,
            active: self.active_document,
            documents: self
                .documents
                .iter()
                .map(|d| DocumentStamp {
                    id: d.model.id,
                    revision: d.model.revision,
                    target_generation: d.session.route().generation,
                    saved_pointer: d.model.saved_source.as_ptr() as usize,
                    saved_length: d.model.saved_source.len(),
                    title: d.model.title.clone(),
                    path: d.model.path.clone(),
                })
                .collect(),
        }
    }
    fn recovery_snapshot(&self, cx: &App) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            schema_version: dbide_workspace::SCHEMA_VERSION,
            active_document_id: Some(self.active_document),
            documents: self
                .documents
                .iter()
                .map(|d| DocumentSnapshot {
                    id: d.model.id,
                    title: d.model.title.clone(),
                    sql: d.editor.read(cx).value().to_string(),
                    saved_sql: d.model.saved_source.clone(),
                    revision: d.model.revision,
                    path: d.model.path.clone(),
                    profile_id: d.required_profile_id.clone(),
                    execution_target: d.session.stored_target(),
                })
                .collect(),
        }
    }
    pub(super) fn init_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fixture_smoke {
            self.recovery.loading = false;
            self.recovery.blocked = true;
            return;
        }
        let Some(profile_store) = &self.store else {
            self.recovery.loading = false;
            self.recovery.blocked = true;
            self.recovery.status = "Recovery unavailable until profile storage is repaired.".into();
            return;
        };
        let store = WorkspaceStore::new(profile_store.path().with_file_name("workspace.json"));
        let preferences = profile_store
            .path()
            .with_file_name("editor-preferences.json");
        self.recovery.store = Some(store.clone());
        self.recovery.preferences = Some(preferences.clone());
        let initial = self.recovery_stamp();
        cx.spawn_in(window,async move|view,cx|{
            let result=cx.background_executor().spawn(async move{
                let enabled=load_preferences(&preferences)?.enabled;
                // Validate an existing snapshot even while disabled. Never overwrite corrupt data.
                let snapshot=store.load().map_err(|e|e.to_string())?;Ok::<_,String>((enabled,snapshot))
            }).await;
            let _=view.update_in(cx,|s,w,cx|{
                s.recovery.loading=false;
                match result {
                    Ok((enabled,snapshot))=>{
                        s.recovery.enabled=enabled;
                        let had_snapshot=snapshot.as_ref().is_some_and(|s|!s.documents.is_empty());
                        if enabled&&let Some(snapshot)=snapshot {
                            if s.recovery_stamp()!=initial {s.recovery.blocked=true;s.recovery.status="Recovery not restored because this window changed during loading. Existing recovery file preserved; reopen to retry.".into();}
                            else if !snapshot.documents.is_empty(){s.restore_workspace_snapshot(snapshot,w,cx);}
                        }
                        if !s.recovery.blocked {s.recovery.status=if enabled{"SQL recovery enabled · local 0600 file; original SQL files are not changed."}else{"SQL recovery off · enable to retain local drafts across restarts."}.into();s.recovery.stamp=Some(s.recovery_stamp());s.recovery.saved=if had_snapshot{s.recovery.stamp.clone()}else{None};}
                    },
                    Err(error)=>{s.recovery.blocked=true;s.recovery.status=error.clone();s.diagnostic=Some(Diagnostic::new(("SQL recovery unavailable".into(),error,format!("Existing recovery data is preserved. Repair the local files explicitly, then reopen: {} and {}",s.recovery.store.as_ref().map(|p|p.path().display().to_string()).unwrap_or_default(),s.recovery.preferences.as_ref().map(|p|p.display().to_string()).unwrap_or_default()))));},
                }cx.notify();
            });
            loop {cx.background_executor().timer(Duration::from_millis(300)).await;if view.update_in(cx,|s,w,cx|s.recovery_tick(false,w,cx)).is_err(){break;}}
        }).detach();
    }
    fn restore_workspace_snapshot(
        &mut self,
        snapshot: WorkspaceSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.documents.clear();
        for doc in snapshot.documents {
            let required_profile_id = doc.profile_id.clone();
            let mut session = ConsoleSession::new(doc.id);
            if let Some(target) = &doc.execution_target
                && let Some(profile) = self
                    .workspace
                    .profiles
                    .iter()
                    .find(|p| p.id == target.profile_id)
                && let Ok(profile) = target.resolve(profile)
            {
                let route = session.route();
                let _ = session.bind(
                    route,
                    ConnectionRequest {
                        profile,
                        password: String::new(),
                    },
                );
            }
            let (model, sql) = dbide_ui_spike::recovery_model::restore_document(doc);
            let id = model.id;
            let editor = cx.new(|cx| {
                let mut e = EditorState::new(window, cx).language("sql");
                e.set_value(sql, window, cx);
                e
            });
            self._subscriptions
                .push(cx.subscribe(&editor, move |s, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        s.input_changes += 1;
                        if let Some(d) = s.documents.iter_mut().find(|d| d.model.id == id) {
                            d.model.changed();
                        }
                        s.completions.clear();
                        cx.notify();
                    }
                }));
            let database = session
                .target()
                .map(|p| p.database.clone())
                .unwrap_or_default();
            let password_draft = cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Session password")
            });
            let database_draft = cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder("Database name");
                input.set_value(database, window, cx);
                input
            });
            self.documents.push(SqlDocument {
                password_draft,
                database_draft,
                required_profile_id,
                session,
                model,
                editor,
                results: ResultSnapshot::default(),
            });
        }
        let active = snapshot
            .active_document_id
            .filter(|id| self.documents.iter().any(|d| d.model.id == *id))
            .unwrap_or(self.documents[0].model.id);
        self.active_document = active;
        let doc = self
            .documents
            .iter()
            .find(|d| d.model.id == active)
            .unwrap();
        self.editor = doc.editor.clone();
        self.file_name = doc.model.title.clone();
        self.editor
            .update(cx, |editor, cx| editor.focus(window, cx));
        self.next_document = 1;
        self.pages.clear();
        self.summary = None;
        self.result_owner = None;
        self.details = "Recovered SQL drafts · no query was executed".into();
        self.status = format!(
            "Restored {} SQL tabs · no connection opened or SQL executed",
            self.documents.len()
        );
    }
    pub(super) fn toggle_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recovery.blocked && self.recovery.enabled {
            self.recovery.enabled = false;
            self.recovery.status="Recovery disabled for this window after failure. Existing local files remain preserved.".into();
            cx.notify();
            return;
        }
        if self.recovery.loading || self.recovery.blocked || self.recovery.busy {
            return;
        }
        let enabled = !self.recovery.enabled;
        if enabled && let Err(error) = self.recovery_snapshot(cx).validate() {
            self.diagnostic=Some(Diagnostic::new(("SQL recovery cannot be enabled".into(),error.to_string(),"Save or split documents within the recovery limits first. Preference was not changed.".into())));
            cx.notify();
            return;
        }
        let Some(path) = self.recovery.preferences.clone() else {
            return;
        };
        self.recovery.busy = true;
        self.recovery.preference_busy = true;
        cx.spawn_in(window,async move|view,cx|{
            let result=cx.background_executor().spawn(async move{{if let Some(parent)=path.parent(){let mut builder=std::fs::DirBuilder::new();builder.recursive(true);#[cfg(unix)]{use std::os::unix::fs::DirBuilderExt;builder.mode(0o700);}builder.create(parent)?;}dbide_ui_spike::write_atomic(&path,|out|{serde_json::to_writer(out,&Preferences{enabled}).map_err(std::io::Error::other)})}}).await;
            let _=view.update_in(cx,|s,w,cx|{s.recovery.busy=false;s.recovery.preference_busy=false;match result{Ok(())=>{s.recovery.enabled=enabled;s.recovery.saved=None;s.recovery.status=if enabled{"SQL drafts are saved locally in a 0600 file. SQL may contain sensitive values; source .sql files are not auto-saved."}else{"SQL recovery disabled. The prior recovery snapshot remains local and is not restored while disabled."}.into();s.recovery_tick(true,w,cx);},Err(error)=>s.diagnostic=Some(Diagnostic::new(("Recovery setting was not saved".into(),error.to_string(),"Current recovery preference remains unchanged.".into())))}cx.notify();});
        }).detach();
    }
    pub(super) fn flush_recovery_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.abort_table_open(
            "Window close requested before table reading.".into(),
            true,
            cx,
        );
        self.recovery.close_after = true;
        self.recovery_tick(true, window, cx);
    }
    fn recovery_tick(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recovery.enabled || self.recovery.loading || self.recovery.blocked {
            return;
        }
        let stamp = self.recovery_stamp();
        if self.recovery.stamp.as_ref() != Some(&stamp) {
            self.recovery.stamp = Some(stamp.clone());
            self.recovery.changed = Instant::now();
        }
        if self.recovery.busy {
            return;
        }
        if self.recovery.saved.as_ref() == Some(&stamp) {
            if self.recovery.close_after {
                self.finish_window_close(window, cx);
            }
            return;
        }
        if !force
            && !self.recovery.close_after
            && self.recovery.changed.elapsed() < Duration::from_millis(900)
        {
            return;
        }
        let Some(store) = self.recovery.store.clone() else {
            return;
        };
        let snapshot = self.recovery_snapshot(cx);
        self.recovery.busy = true;
        cx.spawn_in(window,async move|view,cx|{
            let result=cx.background_executor().spawn(async move{store.save(&snapshot)}).await;
            let _=view.update_in(cx,|s,w,cx|{s.recovery.busy=false;match result {Ok(())=>{s.recovery.saved=Some(stamp);if s.recovery.close_after{s.recovery_tick(true,w,cx);}},Err(error)=>{s.recovery.close_after=false;s.recovery.blocked=true;s.recovery.status=format!("SQL recovery paused: {error}");s.diagnostic=Some(Diagnostic::new(("SQL drafts could not be recovered".into(),error.to_string(),"Window kept open. Save SQL files explicitly; existing recovery data was not intentionally replaced after validation failure.".into())));}}cx.notify();});
        }).detach();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn recovery_preferences_default_off_and_reject_corrupt_without_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("editor-preferences.json");
        assert!(!load_preferences(&path).unwrap().enabled);
        std::fs::write(&path, b"broken").unwrap();
        assert!(load_preferences(&path).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"broken");
    }
    #[::core::prelude::v1::test]
    fn recovery_preferences_reject_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("editor-preferences.json");
        std::fs::write(&path, vec![b' '; 4097]).unwrap();
        assert!(load_preferences(&path).is_err());
    }
}
