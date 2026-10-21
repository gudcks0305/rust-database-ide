//! Metadata-only owned-server check. Never starts a model turn.
use dbide_assistant::{AssistantClient, AssistantConfig, CancellationToken};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let endpoint = args.next().ok_or(
        "Usage: probe_codex ws://127.0.0.1:PORT /absolute/scratch /absolute/capability-file",
    )?;
    let scratch = args.next().ok_or("Missing explicit scratch directory")?;
    let capability = args
        .next()
        .ok_or("Missing explicit local capability file")?;
    if args.next().is_some() {
        return Err("Unexpected argument".into());
    }
    let mut config = AssistantConfig::codex(endpoint, scratch);
    config.enabled = true;
    config.capability_token_file = Some(capability.into());
    let summary = AssistantClient::new(config)?
        .probe_metadata(&CancellationToken::new())
        .await?;
    println!("{summary:#?}");
    Ok(())
}
