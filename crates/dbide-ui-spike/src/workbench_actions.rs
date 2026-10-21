use super::*;
use dbide_credentials::CredentialStore;

impl Workbench {
    pub(super) fn current_owner(&self) -> Option<ResultOwner> {
        let profile = if self.fixture_smoke {
            &self.configured.as_ref()?.profile
        } else {
            self.console().target()?
        };
        let document = self
            .documents
            .iter()
            .find(|d| d.model.id == self.active_document)?;
        Some(ResultOwner {
            document_id: document.model.id,
            document_title: document.model.title.clone(),
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            target: dbide_ui_spike::workbench_model::profile_target(profile),
        })
    }
    fn snapshot(&self) -> ResultSnapshot {
        ResultSnapshot {
            pages: self.pages.clone(),
            selected_page: self.selected_page,
            summary: self.summary.clone(),
            details: self.details.clone(),
            owner: self.result_owner.clone(),
        }
    }
    pub(super) fn switch_document(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.active_document == id {
            return;
        }
        let snapshot = self.snapshot();
        if let Some(old) = self
            .documents
            .iter_mut()
            .find(|d| d.model.id == self.active_document)
        {
            old.results = snapshot;
        }
        let Some(next) = self.documents.iter().find(|d| d.model.id == id) else {
            return;
        };
        self.active_document = id;
        self.editor = next.editor.clone();
        self.file_name = next.model.title.clone();
        let results = next.results.clone();
        self.restore_results(results, cx);
        self.completions.clear();
        self.sql_diagnostics.clear();
        cx.notify();
    }
    pub(super) fn restore_results(&mut self, results: ResultSnapshot, cx: &mut Context<Self>) {
        self.pages = results.pages;
        self.selected_page = results.selected_page;
        self.summary = results.summary;
        self.details = results.details;
        self.result_owner = results.owner;
        self.set_mode(false, cx);
    }
    pub(super) fn show_sql_tab_limit(&mut self, cx: &mut Context<Self>) {
        let body = if self.locale == Locale::Ko {
            (
                "SQL 탭 한도에 도달했습니다",
                "SQL 탭은 최대 32개까지 열 수 있습니다. 기존 콘솔의 연결·데이터베이스·SQL은 바꾸지 않았습니다.",
                "SQL 탭 하나를 닫은 후 다시 시도하세요. SQL은 실행하지 않았습니다.",
            )
        } else {
            (
                "SQL tab limit reached",
                "At most 32 SQL tabs can be open. The existing console target and SQL are unchanged.",
                "Close a SQL tab and retry. No SQL was executed.",
            )
        };
        self.diagnostic = Some(Diagnostic {
            body: (body.0.into(), body.1.into(), body.2.into()),
            context: DiagnosticContext::Capacity,
            owner: None,
        });
        cx.notify();
    }
    pub(super) fn new_document(
        &mut self,
        source: String,
        title: String,
        saved: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.documents.len() >= 32 {
            self.show_sql_tab_limit(cx);
            return;
        }
        if self
            .diagnostic
            .as_ref()
            .is_some_and(|d| d.context == DiagnosticContext::Capacity)
        {
            self.diagnostic = None;
        }
        while self
            .documents
            .iter()
            .any(|d| d.model.id == self.next_document)
        {
            self.next_document = self.next_document.checked_add(1).unwrap_or(1);
        }
        let id = self.next_document;
        self.next_document = self.next_document.checked_add(1).unwrap_or(1);
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx).language("sql");
            state.set_value(source.clone(), window, cx);
            state
        });
        self._subscriptions
            .push(cx.subscribe(&editor, move |s, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    s.input_changes += 1;
                    s.completions.clear();
                    if let Some(doc) = s.documents.iter_mut().find(|d| d.model.id == id) {
                        doc.model.changed();
                    }
                    cx.notify();
                }
            }));
        let mut model = DocumentModel::new(
            id,
            title,
            if saved { source.clone() } else { String::new() },
        );
        if !saved && !source.is_empty() {
            model.changed();
        }
        let mut session = ConsoleSession::new(id);
        if let Some(request) = self
            .documents
            .iter()
            .find(|d| d.model.id == self.active_document)
            .and_then(|d| d.session.request())
        {
            let _ = session.bind(session.route(), request);
        }
        let request = session.request();
        let password_draft = cx.new(|cx| {
            let mut input = InputState::new(window, cx)
                .masked(true)
                .placeholder("Session password");
            input.set_value(
                request
                    .as_ref()
                    .map(|r| r.password.clone())
                    .unwrap_or_default(),
                window,
                cx,
            );
            input
        });
        let database_draft = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Database name");
            input.set_value(
                request
                    .as_ref()
                    .map(|r| r.profile.database.clone())
                    .unwrap_or_default(),
                window,
                cx,
            );
            input
        });
        self.documents.push(SqlDocument {
            password_draft,
            database_draft,
            session,
            required_profile_id: None,
            model,
            editor,
            results: ResultSnapshot::default(),
        });
        self.switch_document(id, cx);
        self.editor.update(cx, |e, cx| e.focus(window, cx));
    }
    pub(super) fn close_document(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .documents
            .iter()
            .find(|d| d.model.id == id)
            .is_some_and(|d| d.session.busy() || d.session.may_have_transaction())
        {
            self.status =
                "Finish/Cancel or explicitly Disconnect this console before closing its tab."
                    .into();
            cx.notify();
            return;
        }
        if self
            .pending_owner
            .as_ref()
            .is_some_and(|o| o.belongs_to(id))
            && self.active_operation.is_some()
        {
            self.status = "Cancel or finish this tab's execution before closing it".into();
            cx.notify();
            return;
        }
        let Some(doc) = self.documents.iter().find(|d| d.model.id == id) else {
            return;
        };
        if !doc.model.dirty(&doc.editor.read(cx).value()) {
            self.remove_document(id, window, cx);
            return;
        }
        let prompt = window.prompt(
            PromptLevel::Warning,
            tr(self.locale, Message::UnsavedChanges),
            Some(&doc.model.title),
            &[
                tr(self.locale, Message::Save),
                tr(self.locale, Message::KeepEditing),
                tr(self.locale, Message::DiscardChanges),
            ],
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            let Ok(choice) = prompt.await else {
                return;
            };
            let _ = view.update_in(cx, |s, w, cx| match choice {
                0 => s.save_document(id, true, w, cx),
                2 => s.remove_document(id, w, cx),
                _ => {}
            });
        })
        .detach();
    }
    fn remove_document(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.documents.iter().position(|d| d.model.id == id) else {
            return;
        };
        if id == self.active_document
            && let Some(next) = self
                .documents
                .iter()
                .find(|d| d.model.id != id)
                .map(|d| d.model.id)
        {
            self.switch_document(next, cx);
        }
        self.documents.remove(index);
        if self.documents.is_empty() {
            self.new_document(String::new(), "untitled.sql".into(), true, window, cx);
        }
        cx.notify();
    }
    pub(super) fn open_sql(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open .sql files in new tabs".into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                let _ = view.update_in(cx, |s, w, cx| s.open_sql_paths(paths, w, cx));
            }
        })
        .detach();
    }
    pub(super) fn drop_sql_files(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_sql_paths(paths.paths().to_vec(), window, cx);
    }
    pub(super) fn open_sql_paths(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ignored = paths.len().saturating_sub(16);
        self.status = "Opening dropped/selected SQL files · existing tabs preserved".into();
        cx.notify();
        cx.spawn_in(window,async move|view,cx|{
            let mut feedback=vec![];let mut opened=0usize;let mut focused=0usize;let mut capacity_rejected=false;
            if ignored>0 {feedback.push(format!("Skipped {ignored} additional paths: at most16 files per open/drop"));}
            for path in paths.into_iter().take(16) {
                let label=path.display().to_string();
                let result=cx.background_executor().spawn(async move{dbide_ui_spike::sql_files::read_sql_file(&path)}).await;
                let _=view.update_in(cx,|s,w,cx|{
                    match result {
                        Ok(file)=>{
                            if let Some(id)=s.documents.iter().find(|d|d.model.path.as_ref()==Some(&file.path)).map(|d|d.model.id) {s.switch_document(id,cx);focused+=1;}
                            else if s.documents.len()>=32 {capacity_rejected=true;feedback.push(format!("{label}: maximum32 open SQL tabs reached"));}
                            else {s.new_document(file.sql,file.title,true,w,cx);if let Some(doc)=s.documents.iter_mut().find(|d|d.model.id==s.active_document){doc.model.path=Some(file.path);}opened+=1;}
                        },Err(error)=>feedback.push(error.to_string()),
                    }cx.notify();
                });
            }
            let _=view.update_in(cx,|s,_,cx|{
                s.status=format!("SQL files · opened {opened} · focused {focused} existing · {} rejected/skipped · no SQL executed",feedback.len());
                if !feedback.is_empty(){s.diagnostic=Some(Diagnostic::new(("Some SQL files were not opened".into(),feedback.join("\n"),"Drop regular UTF-8 .sql files up to64 KiB each. Symlinks resolve to the canonical regular file; existing dirty tabs are focused unchanged.".into())));}
                if capacity_rejected {s.show_sql_tab_limit(cx);if let Some(diagnostic)=&mut s.diagnostic{diagnostic.body.1.push('\n');diagnostic.body.1.push_str(&feedback.join("\n"));}}
                cx.notify();
            });
        }).detach();
    }
    pub(super) fn save_file(&mut self, csv: bool, window: &mut Window, cx: &mut Context<Self>) {
        if csv {
            self.export_result(false, window, cx)
        } else {
            self.save_document(self.active_document, false, window, cx)
        }
    }
    pub(super) fn save_document(
        &mut self,
        id: u64,
        close_after: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = self
            .documents
            .iter()
            .find(|d| d.model.id == id)
            .and_then(|d| d.model.path.clone())
        {
            self.save_document_to(id, close_after, path, window, cx);
        } else {
            self.save_document_as(id, close_after, window, cx);
        }
    }
    pub(super) fn save_document_as(
        &mut self,
        id: u64,
        close_after: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.documents.iter().find(|d| d.model.id == id) else {
            return;
        };
        let receiver = cx.prompt_for_new_path(
            doc.model
                .path
                .as_deref()
                .and_then(std::path::Path::parent)
                .unwrap_or_else(|| std::path::Path::new(".")),
            Some(&doc.model.title),
        );
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(path))) = receiver.await {
                let _ = view.update_in(cx, |s, w, cx| {
                    s.save_document_to(id, close_after, path, w, cx)
                });
            }
        })
        .detach();
    }
    fn save_document_to(
        &mut self,
        id: u64,
        close_after: bool,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.documents.iter().find(|d| d.model.id == id) else {
            return;
        };
        let sql = doc.editor.read(cx).value().to_string();
        let written = sql.clone();
        cx.spawn_in(window,async move|view,cx|{
            let name=path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let result=cx.background_executor().spawn(async move{dbide_ui_spike::write_atomic(&path,|out|std::io::Write::write_all(out,sql.as_bytes())).map(|_|std::fs::canonicalize(&path).unwrap_or(path))}).await;
            let _=view.update_in(cx,|s,w,cx|{match result {
                Ok(path)=>if let Some(doc)=s.documents.iter_mut().find(|d|d.model.id==id){doc.model.saved(written,name.clone());doc.model.path=Some(path);let clean=!doc.model.dirty(&doc.editor.read(cx).value());if s.active_document==id{s.file_name=name;}
                    if close_after&&clean{s.remove_document(id,w,cx);}s.status=if clean{"SQL file saved"}else{"Saved snapshot; newer edits remain unsaved"}.into();},
                Err(e)=>s.diagnostic=Some(Diagnostic::new(("Could not save SQL file".into(),e.to_string(),"A failed write preserves the existing destination. Unsaved tab text remains available.".into())))
            }cx.notify();});
        }).detach();
    }
    pub(super) fn accept_report(
        &mut self,
        report: dbide_pg_spike::ExecutionReport,
        cx: &mut Context<Self>,
    ) {
        self.active_execution = None;
        self.active_operation = None;
        self.commit_policy = report.commit_policy;
        let (pages, summary) = split_report(report);
        self.transaction_state = summary.transaction_state;
        self.connection = if self.connection == ConnectionState::Disconnecting {
            ConnectionState::Disconnecting
        } else if summary.connection_retired {
            ConnectionState::Disconnected
        } else {
            ConnectionState::Connected
        };
        let owner = self.pending_owner.take();
        let label = owner
            .as_ref()
            .map(ResultOwner::label)
            .unwrap_or_else(|| "SQL execution".into());
        self.status = format!(
            "Last operation · {} · {:?} · {} rows · {:.1} ms{}",
            label,
            summary.outcome,
            summary.retained_rows,
            summary.elapsed_micros as f64 / 1000.,
            if summary.truncated {
                " · truncated"
            } else {
                ""
            }
        );
        if summary.connection_retired {
            self.status
                .push_str(" · connection retired; next Run reconnects");
        }
        let details = format!(
            "{} · transaction at completion {:?} · {} bytes{}{}",
            label,
            summary.transaction_state,
            summary.retained_bytes,
            summary
                .terminal_error
                .as_ref()
                .map(|e| format!("\nError: {e}"))
                .unwrap_or_default(),
            if summary.notices.is_empty() {
                String::new()
            } else {
                format!("\n{}", summary.notices.join("\n"))
            }
        );
        if let (Some(sql), Some(owner)) = (self.pending_sql.take(), owner.as_ref()) {
            let entry = HistoryEntry {
                execution_target: None,
                profile_id: owner.profile_id.clone(),
                sql,
                executed_at_unix_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                outcome: format!("{:?}", summary.outcome),
                row_count: Some(summary.retained_rows as u64),
            };
            let _ = self.workspace.record_history(entry.clone());
            if self.workspace.persist_history {
                self.persist_profile_mutation(ProfileMutation::History(entry), cx);
            }
        }
        if summary.outcome == dbide_pg_spike::Outcome::Completed
            && self.diagnostic.as_ref().is_some_and(|d| {
                d.context == DiagnosticContext::Query
                    && d.owner
                        .as_ref()
                        .zip(owner.as_ref())
                        .is_some_and(|(a, b)| a.profile_id == b.profile_id && a.target == b.target)
            })
        {
            self.diagnostic = None;
        }
        if let Some(error) = &summary.terminal_error {
            self.diagnostic = Some(Diagnostic::new((
                format!(
                    "{} · {label}",
                    if summary.outcome == dbide_pg_spike::Outcome::Cancelled {
                        "Query cancelled"
                    } else {
                        "Query failed"
                    }
                ),
                error.clone(),
                if summary.connection_retired {
                    "Connection retired. Run reconnects before submitting the next query."
                } else {
                    "Review SQL and server permissions; a failed transaction requires Rollback."
                }
                .into(),
            )));
        }
        if summary.terminal_error.is_some()
            && let Some(diagnostic) = &mut self.diagnostic
        {
            diagnostic.context = DiagnosticContext::Query;
            diagnostic.owner = owner.clone();
        }
        let snapshot = ResultSnapshot {
            pages: pages.into_iter().map(Rc::new).collect(),
            selected_page: 0,
            summary: Some(summary),
            details,
            owner: owner.clone(),
        };
        if let Some(owner) = &owner {
            if let Some(doc) = self
                .documents
                .iter_mut()
                .find(|d| owner.belongs_to(d.model.id))
            {
                doc.results = snapshot.clone();
            }
            if owner.belongs_to(self.active_document) {
                self.restore_results(snapshot, cx);
            }
        }
        cx.notify();
    }
    fn catalog(&self) -> dbide_sql::Catalog {
        dbide_sql::Catalog {
            tables: self
                .console()
                .schema
                .iter()
                .map(|o| dbide_sql::CatalogTable {
                    schema: o.schema.clone(),
                    name: o.name.clone(),
                    columns: o.columns.iter().map(|c| c.name.clone()).collect(),
                })
                .collect(),
        }
    }
    pub(super) fn current_revision(&self) -> u64 {
        self.documents
            .iter()
            .find(|d| d.model.id == self.active_document)
            .map_or(0, |d| d.model.revision)
    }
    pub(super) fn format_sql_action(
        &mut self,
        _: &FormatSql,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sql = self.editor.read(cx).value().to_string();
        match dbide_sql::format_sql(&sql, self.dialect()) {
            Ok(formatted) => {
                self.editor
                    .update(cx, |e, cx| e.replace_all(formatted, window, cx));
                self.status = "SQL formatted · Undo restores the original source".into();
            }
            Err(e) => self.diagnostic = Some(Diagnostic::new((
                "Format unavailable · source preserved".into(),
                e.to_string(),
                "The formatter refuses commented or unsupported input instead of discarding text."
                    .into(),
            ))),
        }
        cx.notify();
    }
    pub(super) fn run_current_action(
        &mut self,
        _: &RunCurrent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = self.editor.read(cx);
        let sql = editor.value().to_string();
        let cursor = editor.cursor();
        match dbide_sql::statement_at_cursor(&sql, cursor, self.dialect()) {
            Some(range) => self.submit_sql(sql[range].to_owned(), cx),
            None => {
                self.diagnostic=Some(Diagnostic::new(("Current statement is unsupported".into(),"No safely executable statement boundary at this cursor. No SQL was submitted.".into(),"Move the cursor to a supported statement or explicitly choose Run script after reviewing the entire script.".into())));
                cx.notify();
            }
        }
    }
    pub(super) fn complete_sql_action(
        &mut self,
        _: &CompleteSql,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = self.editor.read(cx);
        let sql = editor.value().to_string();
        self.completions =
            dbide_sql::completions(&sql, editor.cursor(), self.dialect(), &self.catalog());
        self.completion_source = Some((self.active_document, self.current_revision(), sql));
        self.show_sql_tools = false;
        self.transfer_preview = None;
        self.status = format!(
            "{} completion candidates · catalog contains {} tables",
            self.completions.len(),
            self.console().schema.len()
        );
        cx.notify();
    }
    pub(super) fn apply_completion(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, revision, source)) = &self.completion_source else {
            return;
        };
        if *id != self.active_document
            || *revision != self.current_revision()
            || *source != self.editor.read(cx).value().as_str()
        {
            self.status = "Completion snapshot is stale. Request completion again.".into();
            self.completions.clear();
            cx.notify();
            return;
        }
        let Some(item) = self.completions.get(index) else {
            return;
        };
        let mut sql = source.clone();
        if sql.get(item.replace.clone()).is_none() {
            return;
        }
        let end = item.replace.start + item.insert_text.len();
        sql.replace_range(item.replace.clone(), &item.insert_text);
        self.editor.update(cx, |e, cx| {
            e.replace_all(sql, window, cx);
            let position = e.text().offset_to_position(end);
            e.set_cursor_position(position, window, cx);
        });
        self.completions.clear();
        cx.notify();
    }
    pub(super) fn analyze_sql(&mut self, cx: &mut Context<Self>) {
        self.sql_diagnostics = dbide_sql::diagnostics(
            &self.editor.read(cx).value(),
            self.dialect(),
            &self.catalog(),
        );
        self.show_sql_tools = true;
        self.transfer_preview = None;
        self.status = format!(
            "{} SQL diagnostics · {} catalog tables",
            self.sql_diagnostics.len(),
            self.console().schema.len()
        );
        cx.notify();
    }
    pub(super) fn preview_rename(&mut self, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        let sql = editor.value().to_string();
        let name = self.rename_name.read(cx).value();
        match dbide_sql::rename(
            &sql,
            editor.cursor(),
            &name,
            self.dialect(),
            &self.catalog(),
            self.current_revision(),
        ) {
            Ok(preview) => {
                self.status = format!(
                    "Rename preview · {} bound edits · no database DDL",
                    preview.edits.len()
                );
                self.rename_preview = Some(OwnedRenamePreview {
                    document_id: self.active_document,
                    preview,
                });
            }
            Err(e) => {
                self.rename_preview = None;
                self.diagnostic=Some(Diagnostic::new(("Rename unavailable".into(),e.to_string(),"Place the cursor on a uniquely resolved alias or CTE; refresh the catalog when needed.".into())));
            }
        }
        self.show_sql_tools = true;
        cx.notify();
    }
    pub(super) fn apply_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preview) = &self.rename_preview else {
            return;
        };
        match preview.apply(
            self.active_document,
            &self.editor.read(cx).value(),
            self.current_revision(),
        ) {
            Ok(sql) => {
                self.editor
                    .update(cx, |e, cx| e.replace_all(sql, window, cx));
                self.rename_preview = None;
                self.status = "Document-local rename applied · Undo available".into();
            }
            Err(e) => {
                self.diagnostic = Some(Diagnostic::new((
                    "Rename preview is stale".into(),
                    e.to_string(),
                    "Generate a new preview for the current tab and revision.".into(),
                )))
            }
        }
        cx.notify();
    }
    pub(super) fn change_policy(&mut self, policy: CommitPolicy, cx: &mut Context<Self>) {
        if self.table_editor.is_none() && !self.fixture_smoke {
            self.console_policy(policy, cx);
            return;
        }
        if self.busy()
            || self.transaction_may_be_open()
            || self.connection != ConnectionState::Connected
        {
            return;
        }
        if let Some(bridge) = &self.bridge {
            match bridge.set_commit_policy(policy) {
                Ok(op) => self.policy_operation = Some(op),
                Err(e) => self.status = format!("Policy request failed: {e:?}"),
            }
        }
        cx.notify();
    }
    pub(super) fn finish_transaction(&mut self, commit: bool, cx: &mut Context<Self>) {
        if self.table_editor.is_none() && !self.fixture_smoke {
            self.console_transaction(commit, cx);
            return;
        }
        if self.busy()
            || self.connection != ConnectionState::Connected
            || !self.transaction_may_be_open()
            || (commit && self.transaction_state != ServerTransactionState::Active)
        {
            return;
        }
        if let Some(bridge) = &self.bridge {
            let result = if commit {
                bridge.commit()
            } else {
                bridge.rollback()
            };
            match result {
                Ok(op) => {
                    self.active_operation = Some(op);
                    self.pending_owner = self.current_owner();
                    self.pending_sql = None;
                    self.connection = ConnectionState::Executing;
                    self.status = if commit {
                        "Committing…"
                    } else {
                        "Rolling back…"
                    }
                    .into();
                }
                Err(e) => self.status = format!("Transaction request failed: {e:?}"),
            }
        }
        cx.notify();
    }
    pub(super) fn save_credential(&mut self, id: String, password: String, cx: &mut Context<Self>) {
        self.credential_busy = true;
        cx.spawn(async move|view,cx|{
            let result=cx.background_executor().spawn(async move {dbide_ui_spike::workbench_model::remember_password(&dbide_credentials::MacKeychainStore,&id,&password,true)}).await;
            let _=view.update(cx,|s,cx|{s.credential_busy=false;if let Err(e)=result{s.diagnostic=Some(Diagnostic::new(("Password could not be remembered".into(),e.to_string(),"Session password remains available. You can continue or retry the explicit remember action.".into())));}cx.notify();});
        }).detach();
    }
    pub(super) fn load_credential(
        &mut self,
        forget: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.credential_busy {
            return;
        }
        let Some(form) = &self.form else {
            return;
        };
        let id = form.original.id.clone();
        let expected = id.clone();
        self.credential_busy = true;
        cx.spawn_in(window,async move|view,cx|{
            let result=cx.background_executor().spawn(async move {if forget{dbide_credentials::MacKeychainStore.remove(&id).map(|_|None)}else{dbide_credentials::MacKeychainStore.load(&id)}}).await;
            let _=view.update_in(cx,|s,w,cx|{
                s.credential_busy=false;
                if s.form.as_ref().is_none_or(|f|f.original.id!=expected){cx.notify();return;}
                match result {
                    Ok(Some(secret))=>match secret.expose_str(){Ok(password)=>{if let Some(form)=&s.form{form.fields[5].update(cx,|input,cx|input.set_value(password.to_owned(),w,cx));}s.form_error=Some("Password loaded for this exact profile. Save to apply it to the session.".into());},Err(_)=>s.form_error=Some("Stored credential is not UTF-8. Enter the password manually.".into())},
                    Ok(None)=>s.form_error=Some(if forget{"Keychain entry removed. Current session password is retained."}else{"No remembered password for this profile. Enter it below."}.into()),
                    Err(e)=>s.form_error=Some(format!("{e}. Enter the password manually to continue.")),
                }cx.notify();
            });
        }).detach();
    }
    pub(super) fn export_result(
        &mut self,
        json: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = self.pages.get(self.selected_page).cloned() else {
            return;
        };
        if self.logical_mode || page.command_only {
            return;
        }
        let receiver = cx.prompt_for_new_path(
            std::path::Path::new("."),
            Some(if json { "results.json" } else { "results.csv" }),
        );
        cx.spawn_in(window,async move|view,cx|{
            let Ok(Ok(Some(path)))=receiver.await else{return;};
            let result=cx.background_executor().spawn(async move{
                let data=dbide_ui_spike::result_data(&page);let limits=dbide_data::Limits::default();
                dbide_ui_spike::write_atomic(&path,|out|{
                    let result=if json{dbide_data::write_json(out,&data,limits)}else{dbide_data::write_csv(out,&data.columns,&data.rows,&dbide_data::CsvOptions::default(),limits)};
                    result.map_err(std::io::Error::other)
                })
            }).await;
            let _=view.update_in(cx,|s,_,cx|{match result{Ok(())=>s.status=if json{"Typed JSON snapshot exported"}else{"CSV snapshot exported · NULL marker is \\N; literal marker collisions are rejected"}.into(),Err(e)=>s.diagnostic=Some(Diagnostic::new(("Export failed · destination preserved".into(),e.to_string(),"CSV literal NULL-marker collisions are refused. JSON preserves all text values and NULL without marker ambiguity.".into())))}cx.notify();});
        }).detach();
    }
    pub(super) fn import_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Preview CSV or typed JSON — no database writes".into()),
        });
        cx.spawn_in(window,async move|view,cx|{
            let Ok(Ok(Some(paths)))=receiver.await else{return;};let Some(path)=paths.first().cloned()else{return;};
            let result=cx.background_executor().spawn(async move{
                let file=std::fs::File::open(&path).map_err(dbide_data::Error::from)?;
                let limits=dbide_data::Limits{max_rows:1000,max_bytes:4*1024*1024};
                if path.extension().is_some_and(|e|e.eq_ignore_ascii_case("json")){dbide_data::read_json(file,limits).map(|data|dbide_data::DataPreview{data,truncated:false})}else{dbide_data::read_csv(file,&dbide_data::CsvOptions::default(),None,limits,dbide_data::ReadMode::Preview)}
            }).await;
            let _=view.update_in(cx,|s,_,cx|{match result{Ok(preview)=>{s.transfer_preview=Some(preview);s.show_sql_tools=false;s.show_history=false;s.status="Import preview only · no SQL submitted or database writes performed".into();},Err(e)=>s.diagnostic=Some(Diagnostic::new(("Import preview failed".into(),e.to_string(),"CSV uses a header and explicit \\N NULL marker. JSON must use the typed dbide-data format. Preview limit: 1,000 rows / 4 MiB.".into())))}cx.notify();});
        }).detach();
    }
}

impl Workbench {
    pub(super) fn quit_action(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
        self.quit_after_close = true;
        if self.request_window_close(window, cx) {
            cx.quit();
        } else if !self.close_prompt_open && !self.recovery.close_after {
            self.quit_after_close = false;
        }
    }
    pub(super) fn finish_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.abort_table_open("Window closed before table reading.".into(), true, cx);
        if self.quit_after_close {
            cx.quit();
        } else {
            window.remove_window();
        }
    }
    pub(super) fn request_window_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.profile_write_busy
            || !self.profile_writes.is_empty()
            || self.recovery.loading
            || self.recovery.preference_busy
        {
            self.status = "Wait for local profile/recovery storage before closing.".into();
            cx.notify();
            return false;
        }
        if self.recovery.enabled && self.recovery.blocked {
            self.diagnostic = Some(Diagnostic::new((
                "Recovery must be resolved before closing".into(),
                self.recovery.status.clone(),
                "Save SQL files explicitly or disable recovery after resolving the storage error."
                    .into(),
            )));
            cx.notify();
            return false;
        }
        if self
            .table_editor
            .as_ref()
            .is_some_and(|e| e.read(cx).is_busy())
        {
            self.status =
                "Wait for the table operation or cancel its execution before closing".into();
            cx.notify();
            return false;
        }
        let changed = if self.recovery.enabled {
            None
        } else {
            self.documents
                .iter()
                .find(|d| d.model.dirty(&d.editor.read(cx).value()))
                .map(|d| d.model.id)
        };
        let staged_rows = self
            .table_editor
            .as_ref()
            .is_some_and(|e| e.read(cx).has_unsaved(cx));
        if changed.is_none()
            && !staged_rows
            && !self.transaction_may_be_open()
            && self.active_operation.is_none()
            && !self
                .documents
                .iter()
                .any(|d| d.session.busy() || d.session.may_have_transaction())
        {
            if self.recovery.enabled {
                self.flush_recovery_and_close(window, cx);
                return false;
            }
            self.abort_table_open("Window closed before table reading.".into(), true, cx);
            return true;
        }
        if self.close_prompt_open || window.has_active_prompt() {
            return false;
        }
        self.close_prompt_open = true;
        let detail = if self.recovery.enabled {
            self.ui_label("SQL drafts will be retained locally without modifying source files. Open transactions are disconnected without committing. No staged row edit is applied by closing.")
        } else {
            self.ui_label("Unsaved SQL and staged table edits will be discarded. Open transactions are disconnected without committing. No staged row edit is applied by closing.")
        };
        let answers = if changed.is_some() {
            vec![
                tr(self.locale, Message::Save),
                tr(self.locale, Message::KeepEditing),
                self.ui_label("Discard and close"),
            ]
        } else {
            vec![
                tr(self.locale, Message::KeepEditing),
                self.ui_label("Discard and close"),
            ]
        };
        let discard = answers.len() - 1;
        let receiver = window.prompt(
            PromptLevel::Warning,
            self.ui_label("Close this database workspace?"),
            Some(detail),
            &answers,
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            let choice = receiver.await.unwrap_or(1);
            let _ = view.update_in(cx, |s, w, cx| {
                s.close_prompt_open = false;
                match choice {
                    answer if answer == discard => {
                        if s.recovery.enabled {
                            s.flush_recovery_and_close(w, cx);
                        } else {
                            s.finish_window_close(w, cx);
                        }
                    }
                    0 if changed.is_some() => {
                        if let Some(id) = changed {
                            s.save_document(id, false, w, cx);
                        }
                    }
                    _ => {
                        s.quit_after_close = false;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        false
    }
}
impl Workbench {
    pub(super) fn engine(&self) -> DatabaseEngine {
        self.console()
            .target()
            .map(|p| p.engine)
            .or_else(|| self.configured.as_ref().map(|r| r.profile.engine))
            .unwrap_or_default()
    }
    pub(super) fn dialect(&self) -> dbide_sql::Dialect {
        dbide_ui_spike::workbench_model::engine_dialect(self.engine())
    }
    pub(super) fn open_tools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_table_open.is_some() {
            return;
        }
        let request = self.console().request();
        self.tools_label = request
            .as_ref()
            .map(|r| {
                format!(
                    "{} · {}",
                    r.profile.name,
                    dbide_ui_spike::workbench_model::profile_target(&r.profile)
                )
            })
            .unwrap_or_else(|| "No profile captured · offline tools".into());
        let sql = self.editor.read(cx).value().to_string();
        let panel = cx.new(|cx| {
            dbide_tools_ui::ToolsPanel::new_with_locale(request, sql, self.locale, window, cx)
        });
        self._subscriptions.push(cx.subscribe_in(&panel,window,|s,source,event,w,cx|{
            if s.tools.as_ref().is_none_or(|current|current.entity_id()!=source.entity_id()){return;}
            let dbide_tools_ui::ToolsEvent::OpenSqlDraft{profile,sql}=event;
            s.tools=None;
            if s.console().target().is_some_and(|p|p==profile){s.new_document(sql.clone(),"object-draft.sql".into(),false,w,cx);s.status="Captured-profile SQL draft opened · review before explicit Run".into();}
            else{s.diagnostic=Some(Diagnostic::new(("SQL draft profile changed".into(),"The captured table/index/view draft was not opened under another connection.".into(),"Select the original profile, reopen Tools, and generate a new draft. Nothing was executed.".into())));}
            cx.notify();
        }));
        self.tools = Some(panel);
        cx.notify();
    }
    pub(super) fn choose_sqlite_path(
        &mut self,
        new_path: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &self.form else {
            return;
        };
        if form.engine != DatabaseEngine::Sqlite {
            return;
        }
        let id = form.original.id.clone();
        cx.spawn_in(window,async move|view,cx|{
            let result=if new_path {
                let receiver=cx.update(|_,cx|cx.prompt_for_new_path(std::path::Path::new("."),Some("database.sqlite")));
                match receiver {Ok(receiver)=>receiver.await.ok().and_then(Result::ok).flatten(),Err(_)=>None}
            }else{
                let receiver=cx.update(|_,cx|cx.prompt_for_paths(PathPromptOptions{files:true,directories:false,multiple:false,prompt:Some("Choose an existing SQLite database".into())}));
                match receiver {Ok(receiver)=>receiver.await.ok().and_then(Result::ok).flatten().and_then(|paths|paths.into_iter().next()),Err(_)=>None}
            };
            let Some(path)=result else{return;};
            let _=view.update_in(cx,|s,w,cx|{
                let Some(form)=&mut s.form else{return;};if form.original.id!=id||form.engine!=DatabaseEngine::Sqlite{return;}
                match path.to_str(){Some(path)=>{form.fields[8].update(cx,|input,cx|input.set_value(path.to_owned(),w,cx));s.form_error=if new_path{Some("New path selected. Explicitly enable file creation if Connect should create a missing database.".into())}else{None};},None=>s.form_error=Some("This database path is not valid UTF-8. Choose a UTF-8 path.".into())}cx.notify();
            });
        }).detach();
    }
}
impl Workbench {
    pub(super) fn new_sql_action(
        &mut self,
        _: &NewSqlFile,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_document(String::new(), "untitled.sql".into(), true, w, cx);
    }
    pub(super) fn open_sql_action(
        &mut self,
        _: &OpenSqlFile,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_sql(w, cx);
    }
    pub(super) fn save_sql_action(
        &mut self,
        _: &SaveSqlFile,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_document(self.active_document, false, w, cx);
    }
    pub(super) fn save_sql_as_action(
        &mut self,
        _: &SaveSqlAs,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_document_as(self.active_document, false, w, cx);
    }
    pub(super) fn close_sql_action(
        &mut self,
        _: &CloseSqlTab,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preferences.take().is_some() {
            cx.notify();
            return;
        }
        if self.table_editor.is_some() {
            self.close_table_editor(w, cx);
            return;
        }
        if self.tools.is_some() {
            self.tools = None;
            cx.notify();
            return;
        }
        self.close_document(self.active_document, w, cx);
    }
}
