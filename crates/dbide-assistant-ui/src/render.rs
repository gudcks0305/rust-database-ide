use super::*;
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, Textarea},
};

fn hint(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .text_color(rgb(0x9ba8bd))
        .child(text.into())
}
fn card(title: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .border_1()
        .border_color(rgb(0x343d4c))
        .rounded_md()
        .bg(rgb(0x202733))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title.into()))
}
impl AssistantPanel {
    fn button(&self, id: &'static str, label: &'static str) -> Button {
        Button::new(id)
            .label(label)
            .small()
            .disabled(self.generation.busy())
    }
}
impl Render for AssistantPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.generation.busy();
        let mut config=card("Owned Codex app-server · no API key")
            .child(hint("Supply the endpoint, private capability-token file path and isolated scratch directory from your owned launcher. This panel never starts a server."))
            .child(hint("Loopback WebSocket endpoint"))
            .child(Input::new(&self.endpoint).small().disabled(busy).aria_label("Owned numeric loopback WebSocket endpoint"))
            .child(hint("Capability-token file path · contents never displayed"))
            .child(Input::new(&self.token_path).small().disabled(busy).aria_label("Absolute capability-token file path, not token contents"))
            .child(hint("Isolated scratch directory · never a project/database directory"))
            .child(Input::new(&self.cwd).small().disabled(busy).aria_label("Absolute isolated scratch directory"))
            .child(hint("Model id · leave blank to use app-server configured model"))
            .child(Input::new(&self.model).small().disabled(busy).aria_label("Optional explicit Codex model id"))
            .child(div().flex().gap_2().flex_wrap()
                .child(Button::new("enable-codex").label(if self.enabled {"Disable Codex"}else{"Enable Codex"}).small().when(self.enabled,|b|b.primary()).on_click(cx.listener(|s,_,_,cx|{if s.enabled{s.cancel(cx);}s.enabled= !s.enabled;s.reviewed=None;s.metadata=None;s.status=if s.enabled{"Enabled locally · no connection until Probe or Send".into()}else{"Disabled · no connection or context sent".into()};cx.notify();})))
                .child(self.button("probe-codex","Probe auth + model catalog").disabled(busy||!self.enabled).on_click(cx.listener(|s,_,_,cx|s.probe(cx)))));
        if let Some(metadata) = &self.metadata {
            config=config.child(hint(format!("Authenticated: {} ({}) · model page complete: {} · shell disabled: {} · hooks disabled: {} · enabled MCP/plugins: {}/{} · docs/memories disabled: {}/{}",metadata.authenticated,metadata.authentication_kind,metadata.model_page_complete,metadata.shell_tool_disabled,metadata.hooks_disabled,metadata.enabled_mcp_servers,metadata.enabled_plugins,metadata.project_docs_disabled,metadata.memories_disabled)))
                .child(hint(format!("Available model ids: {}",metadata.model_choices.join(", "))));
        }
        let mut request=card("Explicit context selection")
            .child(hint(format!("Captured profile: {} · SQL: {} bytes · schema: {} bytes. Parent selection changes do not retarget this panel.",self.profile.as_ref().map(|p|p.name.as_str()).unwrap_or("none"),self.captured_sql.as_ref().map_or(0,String::len),self.captured_schema.as_ref().map_or(0,String::len))))
            .child(div().flex().gap_2().flex_wrap().children([(AssistantTask::GenerateSql,"Generate SQL"),(AssistantTask::Explain,"Explain"),(AssistantTask::Optimize,"Optimize")].into_iter().enumerate().map(|(index,(task,label))|Button::new(("assistant-task",index)).label(label).small().disabled(busy).when(self.task==task,|b|b.primary()).on_click(cx.listener(move|s,_,_,cx|{s.task=task;s.invalidate_preview(cx);})))))
            .child(Textarea::new(&self.instruction).h(px(90.)).disabled(busy).aria_label("AI instruction, at most 64 KiB").on_paste(|clipboard,_,_|clipboard.text().is_some_and(|text|text.len()>MAX_INSTRUCTION_BYTES)))
            .child(div().flex().gap_2().flex_wrap()
                .child(self.button("assistant-include-sql",if self.include_sql {"Include SQL: on"}else{"Include SQL: off"}).disabled(busy||self.captured_sql.is_none()).when(self.include_sql,|b|b.primary()).on_click(cx.listener(|s,_,_,cx|{s.include_sql= !s.include_sql;s.invalidate_preview(cx);})))
                .child(self.button("assistant-include-schema",if self.include_schema {"Include schema: on"}else{"Include schema: off"}).disabled(busy||self.captured_schema.is_none()).when(self.include_schema,|b|b.primary()).on_click(cx.listener(|s,_,_,cx|{s.include_schema= !s.include_schema;s.invalidate_preview(cx);}))))
            .child(hint("Both context flags start off. Explain/Optimize require Include SQL. Instruction is always included. No credentials, history, live catalog or database rows are collected."))
            .child(self.button("assistant-preview","Preview selected payload").disabled(busy||!self.enabled).on_click(cx.listener(|s,_,w,cx|s.preview(w,cx))));
        if let Some(reviewed) = &self.reviewed {
            request=request.child(hint(format!("Destination: {} · instruction {} B · SQL {:?} B · schema {:?} B · request {} B · ephemeral thread. Exact selected context:",reviewed.summary.destination_origin,reviewed.summary.instruction_bytes,reviewed.summary.sql_bytes,reviewed.summary.schema_bytes,reviewed.summary.serialized_request_bytes)))
                .child(Textarea::new(&self.preview_text).readonly(true).h(px(180.)).aria_label("Exact reviewed outbound instruction and opted-in context"))
                .child(self.button("assistant-send","Send reviewed snapshot").primary().disabled(busy||!self.enabled).on_click(cx.listener(|s,_,_,cx|s.send(cx))));
        }
        let mut output=card("Untrusted model response")
            .child(hint("Transient progress may omit chunks while the UI is busy; only a completed response becomes a suggestion. SQL parser validation does not establish safety or correctness."));
        // Textarea state is refreshed from bounded output without exposing any capability token.
        let text = if let Some(response) = &self.suggestion {
            response.raw_text.clone()
        } else {
            self.progress.clone()
        };
        if self.response_text.read(cx).value().as_ref() != text {
            self.response_text
                .update(cx, |state, cx| state.set_value(text, _window, cx));
        }
        output=output.child(Textarea::new(&self.response_text).readonly(true).h(px(200.)).aria_label("Untrusted transient or completed Codex response text"))
            .child(div().flex().gap_2().flex_wrap()
                .child(self.button("assistant-copy","Copy response text").disabled(busy||self.suggestion.is_none()).on_click(cx.listener(|s,_,_,cx|s.copy_response(cx))))
                .child(self.button("assistant-open-draft","Open SQL draft for review").disabled(busy||self.draft_sql.is_none()).on_click(cx.listener(|s,_,_,cx|s.open_draft(cx)))))
            .when(self.suggestion.is_some()&&self.draft_sql.is_none(),|el|el.child(hint("No complete structured SQL parsed for the captured dialect. Response can be copied as text for manual review.")));
        div().id("codex-assistant-panel").flex().flex_col().size_full().min_h_0().bg(rgb(0x171c24)).text_color(rgb(0xdce4ef))
            .child(div().px_3().py_2().font_weight(FontWeight::SEMIBOLD).child("Codex assistance · explicit, ephemeral suggestions"))
            .child(div().id("assistant-scroll").flex_1().min_h_0().overflow_y_scroll().p_3().flex().flex_col().gap_3().child(config).child(request).child(output)
                .child(hint("Listener unavailable? From the repository root: bash scripts/codex-assistant-server.sh start. Copy endpoint/token-file/scratch paths from that launcher's output. The UI never runs this command.")))
            .when_some(self.error.clone(),|el,error|el.child(div().id("assistant-error").role(Role::Alert).aria_label(error.clone()).p_3().text_color(rgb(0xffabab)).child(error)))
            .child(div().px_3().py_2().flex().items_center().justify_between().child(hint(self.status.clone())).child(Button::new("cancel-assistant").label("Cancel request").small().disabled(!busy).on_click(cx.listener(|s,_,_,cx|s.cancel(cx)))))
    }
}
