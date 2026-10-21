//! Opt-in native Codex panel. Parent initializes GPUI/component; this crate never runs SQL.
mod model;
mod render;
mod worker;

pub use dbide_assistant::AssistantConfig;
use dbide_assistant::{
    AssistantTask, CancellationToken, MAX_INSTRUCTION_BYTES, MAX_SCHEMA_BYTES, MAX_SQL_BYTES,
    Suggestion,
};
use dbide_profiles::ConnectionProfile;
use futures_util::StreamExt;
use gpui::{prelude::*, *};
use gpui_component::input::{InputEvent, InputState, TextareaState};
use model::{Generation, ReviewedRequest};
use std::{path::Path, time::Duration};

#[derive(Clone, Debug)]
pub enum AssistantEvent {
    /// Explicit user action only. Parent creates a dirty console for this captured profile.
    OpenSqlDraft {
        profile: Option<ConnectionProfile>,
        sql: String,
    },
}
impl EventEmitter<AssistantEvent> for AssistantPanel {}

/// No startup network or file reads. SQL/schema are captured once and excluded by default.
pub struct AssistantPanel {
    profile: Option<ConnectionProfile>,
    captured_sql: Option<String>,
    captured_schema: Option<String>,
    enabled: bool,
    include_sql: bool,
    include_schema: bool,
    task: AssistantTask,
    endpoint: Entity<InputState>,
    token_path: Entity<InputState>,
    cwd: Entity<InputState>,
    model: Entity<InputState>,
    instruction: Entity<TextareaState>,
    instruction_cache: String,
    preview_text: Entity<TextareaState>,
    response_text: Entity<TextareaState>,
    reviewed: Option<ReviewedRequest>,
    generation: Generation,
    cancel: Option<CancellationToken>,
    progress: String,
    suggestion: Option<Suggestion>,
    draft_sql: Option<String>,
    metadata: Option<dbide_assistant::MetadataProbe>,
    status: String,
    error: Option<String>,
}
impl AssistantPanel {
    pub fn new(
        profile: Option<ConnectionProfile>,
        initial_sql: String,
        initial_schema: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = |value: &str, max: usize, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).validate(move |text, _| {
                    text.len() <= max && !text.chars().any(char::is_control)
                });
                state.set_value(value, window, cx);
                state
            })
        };
        let endpoint = field("ws://127.0.0.1:45457", 4096, window, cx);
        let token_path = field("", 4096, window, cx);
        let cwd = field("", 4096, window, cx);
        let model = field("", 256, window, cx);
        let instruction = cx.new(|cx| TextareaState::new(window, cx));
        for input in [&endpoint, &token_path, &cwd, &model] {
            cx.subscribe(input, |s, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    s.invalidate_preview(cx);
                }
            })
            .detach();
        }
        cx.subscribe_in(&instruction, window, |s, input, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value();
                if value.len() > MAX_INSTRUCTION_BYTES {
                    let restored = s.instruction_cache.clone();
                    input.update(cx, |state, cx| state.set_value(restored, window, cx));
                    s.error = Some("Instruction limit is 64 KiB; excess edit rejected".into());
                } else {
                    s.instruction_cache = value.to_string();
                }
                s.invalidate_preview(cx);
            }
        })
        .detach();
        let sql_oversize = initial_sql.len() > MAX_SQL_BYTES;
        let schema_oversize = initial_schema
            .as_ref()
            .is_some_and(|s| s.len() > MAX_SCHEMA_BYTES);
        Self {
            profile,
            captured_sql: (!sql_oversize && !initial_sql.trim().is_empty()).then_some(initial_sql),
            captured_schema: initial_schema
                .filter(|s| s.len() <= MAX_SCHEMA_BYTES && !s.trim().is_empty()),
            enabled: false,
            include_sql: false,
            include_schema: false,
            task: AssistantTask::GenerateSql,
            endpoint,
            token_path,
            cwd,
            model,
            instruction,
            instruction_cache: String::new(),
            preview_text: cx.new(|cx| TextareaState::new(window, cx)),
            response_text: cx.new(|cx| TextareaState::new(window, cx)),
            reviewed: None,
            generation: Generation::default(),
            cancel: None,
            progress: String::new(),
            suggestion: None,
            draft_sql: None,
            metadata: None,
            status: if sql_oversize || schema_oversize {
                "Disabled · oversized captured SQL/schema omitted, never truncated for sending"
                    .into()
            } else {
                "Disabled · no connection or context sent".into()
            },
            error: None,
        }
    }
    fn invalidate_preview(&mut self, cx: &mut Context<Self>) {
        self.reviewed = None;
        self.metadata = None;
        cx.notify();
    }
    /// Allows the host to prefill an explicitly chosen owned listener; still starts disabled.
    pub fn configure(
        &mut self,
        config: AssistantConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel(cx);
        self.enabled = false;
        for (field, value, max) in [
            (&self.endpoint, config.endpoint, 4096),
            (&self.cwd, config.cwd, 4096),
            (
                &self.token_path,
                config
                    .capability_token_file
                    .and_then(|p| p.to_str().map(str::to_owned))
                    .unwrap_or_default(),
                4096,
            ),
            (&self.model, config.model.unwrap_or_default(), 256),
        ] {
            let value = if value.len() <= max && !value.chars().any(char::is_control) {
                value
            } else {
                String::new()
            };
            field.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        self.reviewed = None;
        self.metadata = None;
        self.status = "Listener configured · enable explicitly before connecting".into();
        cx.notify();
    }
    fn configuration(&self, cx: &App) -> Result<AssistantConfig, String> {
        if !self.enabled {
            return Err("Enable Codex assistance before connecting".into());
        }
        let mut config = AssistantConfig::codex(
            self.endpoint.read(cx).value().to_string(),
            self.cwd.read(cx).value().to_string(),
        );
        config.enabled = true;
        config.timeout = Duration::from_secs(120);
        let token = self.token_path.read(cx).value().to_string();
        if token.is_empty()
            || token.len() > 4096
            || token.chars().any(char::is_control)
            || !Path::new(&token).is_absolute()
        {
            return Err(
                "Enter an absolute private capability-token file path; never paste token contents"
                    .into(),
            );
        }
        config.capability_token_file = Some(token.into());
        let model = self.model.read(cx).value().to_string();
        config.model = (!model.is_empty()).then_some(model);
        dbide_assistant::AssistantClient::new(config.clone()).map_err(|e| e.to_string())?;
        Ok(config)
    }
    fn fail(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        self.status = "Action needs attention".into();
        cx.notify();
    }
    fn preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.generation.busy() {
            return;
        }
        self.reviewed = None;
        let result = self.configuration(cx).and_then(|config| {
            ReviewedRequest::new(
                config,
                self.task,
                self.instruction.read(cx).value().to_string(),
                self.captured_sql.as_deref(),
                self.captured_schema.as_deref(),
                self.include_sql,
                self.include_schema,
            )
        });
        match result {
            Ok(request) => {
                let contents = serde_json::to_string_pretty(&request.context).unwrap_or_default();
                self.preview_text
                    .update(cx, |state, cx| state.set_value(contents, window, cx));
                self.reviewed = Some(request);
                self.error = None;
                self.status="Review exact selected context below · Send reviewed snapshot is a separate action".into();
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }
    fn send(&mut self, cx: &mut Context<Self>) {
        if !self.enabled || self.generation.busy() {
            return;
        }
        let Some(request) = self.reviewed.take() else {
            self.fail("Prepare and review a fresh preview before sending", cx);
            return;
        };
        self.start(
            worker::Job::Generate(Box::new(request), self.profile.clone()),
            cx,
        );
    }
    fn probe(&mut self, cx: &mut Context<Self>) {
        if self.generation.busy() {
            return;
        }
        match self.configuration(cx) {
            Ok(config) => self.start(worker::Job::Probe(config), cx),
            Err(error) => self.fail(error, cx),
        }
    }
    fn start(&mut self, job: worker::Job, cx: &mut Context<Self>) {
        let Some(id) = self.generation.begin() else {
            return;
        };
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        self.error = None;
        self.progress.clear();
        self.suggestion = None;
        self.draft_sql = None;
        self.status = "Connecting to explicit owned Codex listener…".into();
        let mut receiver = worker::spawn(job, cancel);
        cx.spawn(async move |view, cx| {
            while let Some(event) = receiver.next().await {
                let complete = matches!(&event, worker::Event::Complete(_));
                let result = view.update(cx, |s, cx| s.receive(id, event, cx));
                if result.is_err() || complete {
                    return;
                }
            }
            let _ = view.update(cx, |s, cx| {
                s.receive(
                    id,
                    worker::Event::Complete(Err("Codex worker stopped before completion".into())),
                    cx,
                )
            });
        })
        .detach();
        cx.notify();
    }
    fn receive(&mut self, id: u64, event: worker::Event, cx: &mut Context<Self>) {
        if !self.generation.accepts(id) {
            return;
        }
        match event {
            worker::Event::Delta(delta) => {
                model::append_bounded(&mut self.progress, &delta);
                self.status = "Receiving untrusted transient model text…".into();
            }
            worker::Event::Complete(result) => {
                self.generation.finish(id);
                self.cancel = None;
                match result {
                    Ok(worker::Completed::Probe(metadata)) => {
                        self.metadata = Some(metadata);
                        self.status = "Metadata probe complete · no model turn started".into();
                    }
                    Ok(worker::Completed::Suggestion(suggestion, draft_sql)) => {
                        // Structured field only; never infer executable SQL from prose/Markdown.
                        self.draft_sql = draft_sql;
                        self.suggestion = Some(suggestion);
                        self.status="Suggestion complete · untrusted text; review before copying or opening a draft".into();
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.status = "Request stopped · no SQL applied".into();
                    }
                }
            }
        }
        cx.notify();
    }
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.generation.cancel();
        self.reviewed = None;
        self.status =
            "Cancelled locally · server interruption best effort; stale results discarded".into();
        cx.notify();
    }
    fn open_draft(&mut self, cx: &mut Context<Self>) {
        if self.generation.busy() {
            return;
        }
        if let Some(sql) = self.draft_sql.clone() {
            cx.emit(AssistantEvent::OpenSqlDraft {
                profile: self.profile.clone(),
                sql,
            });
            self.status = "Opened SQL draft on captured profile · nothing executed".into();
            cx.notify();
        }
    }
    fn copy_response(&mut self, cx: &mut Context<Self>) {
        if let Some(response) = &self.suggestion {
            cx.write_to_clipboard(ClipboardItem::new_string(response.raw_text.clone()));
            self.status = "Copied untrusted response text".into();
            cx.notify();
        }
    }
}
impl Drop for AssistantPanel {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
    }
}
