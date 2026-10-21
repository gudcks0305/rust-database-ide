use crate::model::ReviewedRequest;
use dbide_assistant::{AssistantClient, CancellationToken, MetadataProbe, Suggestion};
use futures_channel::mpsc;
use futures_util::SinkExt;

pub enum Completed {
    Probe(MetadataProbe),
    Suggestion(Suggestion, Option<String>),
}
pub enum Event {
    Delta(String),
    Complete(Result<Completed, String>),
}
pub enum Job {
    Probe(dbide_assistant::AssistantConfig),
    Generate(
        Box<ReviewedRequest>,
        Option<dbide_profiles::ConnectionProfile>,
    ),
}
/// Finite channel; progress may omit chunks if the UI is slow. Final text is authoritative.
pub fn spawn(job: Job, cancel: CancellationToken) -> mpsc::Receiver<Event> {
    let (mut tx, rx) = mpsc::channel(8);
    let _ = std::thread::Builder::new()
        .name("dbide-codex-assistant".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => runtime.block_on(async move {
                    let result = async {
                        if cancel.is_cancelled() {
                            return Err("Codex request cancelled before connection".into());
                        }
                        match job {
                            Job::Probe(config) => {
                                let client =
                                    AssistantClient::new(config).map_err(|e| e.to_string())?;
                                client
                                    .probe_metadata(&cancel)
                                    .await
                                    .map(Completed::Probe)
                                    .map_err(|e| e.to_string())
                            }
                            Job::Generate(request, profile) => {
                                let client = AssistantClient::new(request.config)
                                    .map_err(|e| e.to_string())?;
                                let suggestion = client
                                    .generate_with_progress(
                                        request.task,
                                        &request.context,
                                        &cancel,
                                        |delta| {
                                            let _ = tx.try_send(Event::Delta(delta.to_string()));
                                        },
                                    )
                                    .await
                                    .map_err(|e| e.to_string())?;
                                let draft_sql = crate::model::sql_draft(
                                    suggestion.sql.as_deref(),
                                    profile.as_ref(),
                                )
                                .ok();
                                Ok(Completed::Suggestion(suggestion, draft_sql))
                            }
                        }
                    }
                    .await;
                    let _ = tx.send(Event::Complete(result)).await;
                }),
                Err(_) => {
                    let _ = tx.try_send(Event::Complete(Err(
                        "Could not start Codex worker runtime".into(),
                    )));
                }
            }
        });
    rx
}
#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    #[test]
    fn pre_cancelled_job_does_not_connect_or_open_token() {
        let mut config = dbide_assistant::AssistantConfig::codex("ws://127.0.0.1:1", "/scratch");
        config.enabled = true;
        config.capability_token_file = Some("/does/not/exist".into());
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut events = spawn(Job::Probe(config), cancel);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let event = runtime.block_on(events.next()).unwrap();
        match event {
            Event::Complete(Err(message)) => {
                assert!(message.contains("cancelled before connection"))
            }
            _ => panic!("unexpected worker event"),
        }
    }
}
