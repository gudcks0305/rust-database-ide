//! Explicit opt-in synthetic model turn. Never executes SQL or accesses a DB.
use dbide_assistant::{
    AssistantClient, AssistantConfig, AssistantContext, AssistantTask, CancellationToken,
};
use std::time::{Duration, Instant};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--confirm-live-turn") {
        return Err("Usage: smoke_codex --confirm-live-turn ws://127.0.0.1:PORT /absolute/scratch /absolute/capability-file explicit-model".into());
    }
    let endpoint = args.next().ok_or("Missing explicit endpoint")?;
    let scratch = args.next().ok_or("Missing explicit scratch directory")?;
    let capability = args
        .next()
        .ok_or("Missing explicit local capability file")?;
    let model = args
        .next()
        .ok_or("Missing explicit available model selection")?;
    if args.next().is_some() {
        return Err("Unexpected argument".into());
    }
    let mut config = AssistantConfig::codex(endpoint, scratch);
    config.enabled = true;
    config.capability_token_file = Some(capability.into());
    config.model = Some(model);
    config.timeout = Duration::from_secs(90);
    let client = AssistantClient::new(config)?;
    let cancel = CancellationToken::new();
    let probe = client.probe_metadata(&cancel).await?;
    if probe.authentication_kind != "chatgpt"
        || !probe.builtin_openai_provider
        || !probe.chatgpt_login_required
        || !probe.shell_tool_disabled
        || !probe.hooks_disabled
        || probe.enabled_mcp_servers != 0
        || probe.enabled_plugins != 0
        || !probe.project_docs_disabled
        || !probe.memories_disabled
        || !probe.skills_instructions_disabled
        || !probe.custom_instructions_cleared
        || !probe.custom_file_overrides_absent
    {
        return Err("Owned server restriction metadata did not pass; no live turn started".into());
    }
    let context = AssistantContext {
        instruction: "Return exactly SELECT 1 AS result; in the sql field and a one-sentence explanation. Use only this synthetic context. Never call tools, read files, or access any database.".into(),
        sql: Some("SELECT 1 AS result;".into()),
        schema: None,
    };
    let started = Instant::now();
    let suggestion = client
        .generate(AssistantTask::GenerateSql, &context, &cancel)
        .await?;
    let metadata = suggestion
        .completion
        .ok_or("Missing terminal completion metadata")?;
    let sql = suggestion
        .sql
        .as_deref()
        .ok_or("Model did not return the requested SQL field")?;
    if sql.trim().trim_end_matches(';').trim() != "SELECT 1 AS result" {
        return Err("Model SQL differed from the synthetic fixture; raw output not printed".into());
    }
    println!("auth_kind: chatgpt");
    println!(
        "resolved_model: {}",
        metadata.model.as_deref().unwrap_or("not-reported")
    );
    println!("turn_completed: true");
    println!("elapsed_ms: {}", started.elapsed().as_millis());
    println!("synthetic_sql: SELECT 1 AS result;");
    println!(
        "explanation_bytes: {}",
        suggestion.explanation.as_ref().map_or(0, String::len)
    );
    println!("database_execution: none; raw model text and capability omitted");
    Ok(())
}
