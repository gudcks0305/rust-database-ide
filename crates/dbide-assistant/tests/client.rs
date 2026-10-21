//! Synthetic loopback app-server protocol only. No model, account, or API calls.
use dbide_assistant::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{net::TcpListener, sync::mpsc};
use tokio_tungstenite::{accept_async, tungstenite::Message};

fn context() -> AssistantContext {
    AssistantContext {
        instruction: "Count chosen rows".into(),
        sql: Some("SELECT count(*) FROM chosen".into()),
        schema: None,
    }
}
fn event(method: &str, fields: Value) -> Value {
    let mut params = json!({"threadId":"thread-fixture","turnId":"turn-fixture"});
    params
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    json!({"method":method,"params":params})
}
fn completed(text: &str) -> Vec<Value> {
    vec![
        event(
            "item/agentMessage/delta",
            json!({"itemId":"message","delta":"transient draft"}),
        ),
        event(
            "item/completed",
            json!({"item":{"type":"agentMessage","id":"message","text":text}}),
        ),
        event(
            "turn/completed",
            json!({"turn":{"id":"turn-fixture","status":"completed","error":null}}),
        ),
    ]
}
#[derive(Default)]
struct Script {
    messages: Vec<Value>,
    raw: Option<String>,
    early_events: bool,
    delay: Duration,
    rpc_error: bool,
    api_key_account: bool,
}
async fn fixture(script: Script) -> (AssistantClient, mpsc::UnboundedReceiver<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        while let Some(Ok(message)) = ws.next().await {
            let Message::Text(text) = message else {
                continue;
            };
            let request: Value = serde_json::from_str(&text).unwrap();
            let _ = tx.send(request.clone());
            match request["method"].as_str() {
                Some("initialize") => {
                    ws.send(Message::Text(
                        json!({"id":1,"result":{"userAgent":"fixture"}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                }
                Some("account/read") => {
                    let kind = if script.api_key_account {
                        "apiKey"
                    } else {
                        "chatgpt"
                    };
                    ws.send(Message::Text(
                        json!({"id":5,"result":{"account":{"type":kind}}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                }
                Some("thread/start") => {
                    ws.send(Message::Text(
                        json!({"id":2,"result":{"thread":{"id":"thread-fixture"},"model":"fixture-model"}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                }
                Some("turn/start") => {
                    if script.rpc_error {
                        let _ = ws.send(Message::Text(json!({"id":3,"error":{"code":-1,"message":"private fixture-secret body"}}).to_string().into())).await;
                        continue;
                    }
                    if !script.early_events {
                        let _ = ws
                            .send(Message::Text(
                                json!({"id":3,"result":{"turn":{"id":"turn-fixture"}}})
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                    }
                    tokio::time::sleep(script.delay).await;
                    for value in &script.messages {
                        if ws
                            .send(Message::Text(value.to_string().into()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    if let Some(raw) = &script.raw {
                        let _ = ws.send(Message::Text(raw.clone().into())).await;
                    }
                    if script.early_events {
                        let _ = ws
                            .send(Message::Text(
                                json!({"id":3,"result":{"turn":{"id":"turn-fixture"}}})
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                    }
                }
                Some("turn/interrupt") => {
                    let _ = ws
                        .send(Message::Text(
                            json!({"id":4,"result":{}}).to_string().into(),
                        ))
                        .await;
                }
                _ => {}
            }
        }
    });
    let mut config = AssistantConfig::codex(endpoint, "/tmp/dbide-fixture-empty");
    config.enabled = true;
    config.timeout = Duration::from_millis(250);
    (AssistantClient::new(config).unwrap(), rx)
}
async fn generate(client: &AssistantClient) -> Result<Suggestion, AssistantError> {
    client
        .generate(
            AssistantTask::GenerateSql,
            &context(),
            &CancellationToken::new(),
        )
        .await
}

#[tokio::test]
async fn handshake_scope_and_authoritative_item_contract() {
    let text = r#"{"sql":"SELECT 1","explanation":"Review fixture"}"#;
    let (client, mut sent) = fixture(Script {
        messages: completed(text),
        ..Script::default()
    })
    .await;
    let mut deltas = Vec::new();
    let result = client
        .generate_with_progress(
            AssistantTask::GenerateSql,
            &context(),
            &CancellationToken::new(),
            |delta| deltas.push(delta.to_owned()),
        )
        .await
        .unwrap();
    assert_eq!(result.sql.as_deref(), Some("SELECT 1"));
    assert_eq!(result.raw_text, text);
    assert_eq!(
        result.completion.as_ref().unwrap().model.as_deref(),
        Some("fixture-model")
    );
    assert_eq!(result.completion.as_ref().unwrap().turn_id, "turn-fixture");
    assert_eq!(deltas, ["transient draft"]);
    let initialize = sent.recv().await.unwrap();
    assert_eq!(initialize["method"], "initialize");
    assert!(initialize.get("jsonrpc").is_none());
    assert_eq!(sent.recv().await.unwrap()["method"], "initialized");
    assert_eq!(sent.recv().await.unwrap()["method"], "account/read");
    let thread = sent.recv().await.unwrap();
    assert_eq!(thread["params"]["ephemeral"], true);
    assert_eq!(thread["params"]["sandbox"], "read-only");
    assert!(thread["params"].get("model").is_none());
    let turn = sent.recv().await.unwrap();
    assert_eq!(turn["params"]["sandboxPolicy"]["networkAccess"], false);
    assert_eq!(turn["params"]["approvalsReviewer"], "user");
    let prompt = turn["params"]["input"][0]["text"].as_str().unwrap();
    let selected: Value =
        serde_json::from_str(prompt.split_once("Selected context JSON:\n").unwrap().1).unwrap();
    assert_eq!(
        selected,
        json!({"instruction":"Count chosen rows", "sql":"SELECT count(*) FROM chosen"})
    );
    for key in ["password", "history", "connection", "schema"] {
        assert!(selected.get(key).is_none());
    }
    assert!(!format!("{client:?} {result:?} {:?}", context()).contains("SELECT"));
}

#[tokio::test]
async fn notifications_before_turn_rpc_response_are_preserved() {
    let (client, _) = fixture(Script {
        messages: completed("final explanation"),
        early_events: true,
        ..Script::default()
    })
    .await;
    let result = generate(&client).await.unwrap();
    assert_eq!(result.sql, None);
    assert_eq!(result.explanation.as_deref(), Some("final explanation"));
}

#[test]
fn endpoint_validation_cannot_escape_numeric_loopback() {
    for endpoint in [
        "ws://example.com:4500",
        "ws://localhost:4500",
        "wss://127.0.0.1:4500",
        "ws://127.0.0.1",
        "ws://user:private@127.0.0.1:4500",
        "ws://127.0.0.1:4500/?key=private",
    ] {
        let config = AssistantConfig::codex(endpoint, "/tmp/fixture");
        let error = AssistantClient::new(config).unwrap_err();
        assert_eq!(error, AssistantError::InvalidConfig);
        assert!(!format!("{error} {error:?}").contains("private"));
    }
    assert!(
        AssistantClient::new(AssistantConfig::codex("ws://[::1]:4500", "/tmp/fixture")).is_ok()
    );
    assert!(
        AssistantClient::new(AssistantConfig::codex("ws://127.0.0.1:4500", "relative")).is_err()
    );
}

#[test]
fn preview_has_only_redacted_counts_and_explicit_selection() {
    let client = AssistantClient::new(AssistantConfig::codex(
        "ws://127.0.0.1:4500",
        "/tmp/fixture",
    ))
    .unwrap();
    let mut context = context();
    context.schema = Some("CREATE TABLE private_name (id INTEGER)".into());
    let preview = client
        .build_preview(AssistantTask::Optimize, &context)
        .unwrap();
    assert_eq!(preview.destination_origin, "ws://127.0.0.1:4500");
    assert_eq!(
        preview.schema_bytes,
        context.schema.as_ref().map(String::len)
    );
    assert!(preview.ephemeral_thread);
    assert!(!format!("{preview:?}").contains("private_name"));
    assert!(preview.serialized_request_bytes > context.sql.unwrap().len());
}

#[tokio::test]
async fn only_explicitly_selected_schema_is_sent() {
    let (client, mut sent) = fixture(Script {
        messages: completed("explanation"),
        ..Script::default()
    })
    .await;
    let mut context = context();
    context.schema = Some("CREATE TABLE selected_only (id INT)".into());
    client
        .generate(AssistantTask::Explain, &context, &CancellationToken::new())
        .await
        .unwrap();
    while let Some(request) = sent.recv().await {
        if request["method"] == "turn/start" {
            let prompt = request["params"]["input"][0]["text"].as_str().unwrap();
            let selected: Value =
                serde_json::from_str(prompt.split_once("Selected context JSON:\n").unwrap().1)
                    .unwrap();
            assert_eq!(selected["schema"], context.schema.unwrap());
            break;
        }
    }
}

#[tokio::test]
async fn disabled_and_pre_cancelled_never_connect() {
    let config = AssistantConfig::codex("ws://127.0.0.1:1", "/tmp/fixture");
    let client = AssistantClient::new(config.clone()).unwrap();
    assert_eq!(generate(&client).await, Err(AssistantError::Disabled));
    let client = AssistantClient::new(AssistantConfig {
        enabled: true,
        ..config
    })
    .unwrap();
    let token = CancellationToken::new();
    token.cancel();
    assert_eq!(
        client
            .generate(AssistantTask::GenerateSql, &context(), &token)
            .await,
        Err(AssistantError::Cancelled)
    );
}

#[tokio::test]
async fn approvals_are_refused_with_error_and_never_approved() {
    let messages = vec![
        json!({"id":88,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-fixture","turnId":"turn-fixture","command":"private"}}),
    ];
    let (client, mut sent) = fixture(Script {
        messages,
        ..Script::default()
    })
    .await;
    assert_eq!(
        generate(&client).await,
        Err(AssistantError::ApprovalOrToolRequestRefused)
    );
    let mut refusal = None;
    let mut interrupted = false;
    while let Some(request) = sent.recv().await {
        if request["id"] == 88 {
            refusal = Some(request);
        } else if request["method"] == "turn/interrupt" {
            interrupted = true;
            break;
        }
    }
    let refusal = refusal.unwrap();
    assert_eq!(refusal["error"]["code"], -32601);
    assert!(refusal.get("result").is_none());
    assert!(interrupted);
}

#[tokio::test]
async fn tool_activity_is_not_treated_as_a_suggestion() {
    let messages = vec![event(
        "item/started",
        json!({"item":{"id":"tool","type":"commandExecution","command":"private"}}),
    )];
    let (client, _) = fixture(Script {
        messages,
        ..Script::default()
    })
    .await;
    assert_eq!(
        generate(&client).await,
        Err(AssistantError::UnexpectedToolActivity)
    );
}

#[tokio::test]
async fn malformed_rpc_errors_and_empty_turns_are_safe() {
    let cases = [
        (
            Script {
                raw: Some("private malformed text".into()),
                ..Script::default()
            },
            AssistantError::MalformedResponse,
        ),
        (
            Script {
                rpc_error: true,
                ..Script::default()
            },
            AssistantError::RpcRejected,
        ),
        (
            Script {
                messages: vec![event(
                    "turn/completed",
                    json!({"turn":{"id":"turn-fixture","status":"failed","error":{"message":"private"}}}),
                )],
                ..Script::default()
            },
            AssistantError::IncompleteResponse,
        ),
        (
            Script {
                messages: vec![event(
                    "turn/completed",
                    json!({"turn":{"id":"turn-fixture","status":"completed"}}),
                )],
                ..Script::default()
            },
            AssistantError::NoText,
        ),
    ];
    for (script, expected) in cases {
        let (client, _) = fixture(script).await;
        assert_eq!(generate(&client).await, Err(expected));
        assert!(!format!("{expected:?} {expected}").contains("private"));
    }
}

#[tokio::test]
async fn timeout_and_inflight_cancel_attempt_targeted_interrupt() {
    let (client, mut sent) = fixture(Script::default()).await;
    assert_eq!(generate(&client).await, Err(AssistantError::Timeout));
    let mut interrupted = false;
    while let Some(request) = sent.recv().await {
        if request["method"] == "turn/interrupt" {
            interrupted = true;
            break;
        }
    }
    assert!(interrupted);
    let (client, mut sent) = fixture(Script::default()).await;
    let cancel = CancellationToken::new();
    let other = cancel.clone();
    let task = tokio::spawn(async move {
        client
            .generate(AssistantTask::GenerateSql, &context(), &other)
            .await
    });
    while let Some(request) = sent.recv().await {
        if request["method"] == "turn/start" {
            break;
        }
    }
    // Wait for the turn/start response to make its turn ID available.
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();
    assert_eq!(task.await.unwrap(), Err(AssistantError::Cancelled));
    let interrupt = sent.recv().await.unwrap();
    assert_eq!(interrupt["method"], "turn/interrupt");
    assert_eq!(interrupt["params"]["turnId"], "turn-fixture");
}

#[tokio::test]
async fn oversized_websocket_response_is_rejected() {
    let (client, _) = fixture(Script {
        raw: Some("x".repeat(MAX_PAYLOAD_BYTES + 1)),
        ..Script::default()
    })
    .await;
    assert_eq!(
        generate(&client).await,
        Err(AssistantError::ResponseTooLarge)
    );
}

#[test]
fn field_limits_escaped_wire_limit_and_required_sql_are_enforced() {
    let client = AssistantClient::new(AssistantConfig::codex(
        "ws://127.0.0.1:4500",
        "/tmp/fixture",
    ))
    .unwrap();
    assert_eq!(
        client.build_preview(AssistantTask::GenerateSql, &AssistantContext::default()),
        Err(AssistantError::InvalidContext)
    );
    let mut context = context();
    context.schema = Some("x".repeat(MAX_SCHEMA_BYTES + 1));
    assert_eq!(
        client.build_preview(AssistantTask::GenerateSql, &context),
        Err(AssistantError::InvalidContext)
    );
    context.schema = None;
    context.sql = None;
    assert_eq!(
        client.build_preview(AssistantTask::Explain, &context),
        Err(AssistantError::InvalidContext)
    );
    context.sql = Some("\0".repeat(MAX_SQL_BYTES));
    assert_eq!(
        client.build_preview(AssistantTask::GenerateSql, &context),
        Err(AssistantError::RequestTooLarge)
    );
}

#[cfg(unix)]
#[tokio::test]
#[allow(clippy::result_large_err)] // tungstenite requires its concrete ErrorResponse callback type.
async fn optional_local_capability_token_is_sent_only_in_ws_handshake() {
    use std::os::unix::fs::PermissionsExt;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let path = std::env::temp_dir().join(format!(
        "dbide-ws-capability-fixture-{}",
        listener.local_addr().unwrap().port()
    ));
    std::fs::write(&path, "fixture-capability").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_hdr_async(
            stream,
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                assert_eq!(
                    request.headers()["Authorization"],
                    "Bearer fixture-capability"
                );
                Ok(response)
            },
        )
        .await
        .unwrap();
        let init = ws.next().await.unwrap().unwrap().into_text().unwrap();
        assert!(!init.contains("fixture-capability"));
        ws.send(Message::Text(
            json!({"id":1,"error":{"code":-1,"message":"fixture stopped before turn"}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    });
    let mut config = AssistantConfig::codex(endpoint, "/tmp/fixture");
    config.enabled = true;
    config.capability_token_file = Some(path.clone());
    let client = AssistantClient::new(config).unwrap();
    assert!(!format!("{client:?}").contains("capability-fixture"));
    assert_eq!(generate(&client).await, Err(AssistantError::RpcRejected));
    task.await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn final_answer_phase_wins_and_foreign_turn_notifications_are_ignored() {
    let text = r#"{"sql":"SELECT 2","explanation":"final"}"#;
    let messages = vec![
        json!({"method":"item/completed","params":{"threadId":"foreign","turnId":"turn-fixture","item":{"type":"agentMessage","id":"foreign","text":"private"}}}),
        event(
            "item/completed",
            json!({"item":{"type":"agentMessage","id":"unknown","text":"legacy interim"}}),
        ),
        event(
            "item/completed",
            json!({"item":{"type":"agentMessage","id":"comment","phase":"commentary","text":"interim commentary"}}),
        ),
        event(
            "item/completed",
            json!({"item":{"type":"agentMessage","id":"final","phase":"final_answer","text":text}}),
        ),
        event(
            "turn/completed",
            json!({"turn":{"id":"turn-fixture","status":"completed"}}),
        ),
    ];
    let (client, _) = fixture(Script {
        messages,
        ..Script::default()
    })
    .await;
    let result = generate(&client).await.unwrap();
    assert_eq!(result.raw_text, text);
    assert_eq!(result.sql.as_deref(), Some("SELECT 2"));
}

#[tokio::test]
async fn legacy_completed_items_keep_arrival_order_instead_of_sorting_ids() {
    let messages = vec![
        event(
            "item/completed",
            json!({"item":{"type":"agentMessage","id":"z","text":"first"}}),
        ),
        event(
            "item/completed",
            json!({"item":{"type":"agentMessage","id":"a","text":"second"}}),
        ),
        event(
            "turn/completed",
            json!({"turn":{"id":"turn-fixture","status":"completed"}}),
        ),
    ];
    let (client, _) = fixture(Script {
        messages,
        ..Script::default()
    })
    .await;
    assert_eq!(generate(&client).await.unwrap().raw_text, "first\nsecond");
}

#[tokio::test]
async fn metadata_probe_does_not_start_threads_and_redacts_account_details() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        let mut seen = Vec::new();
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            let request: Value = serde_json::from_str(&text).unwrap();
            let method = request["method"].as_str().unwrap();
            seen.push(method.to_owned());
            let result = match method {
                "initialize" => json!({"userAgent":"fixture"}),
                "initialized" => continue,
                "account/read" => {
                    assert_eq!(request["params"]["refreshToken"], false);
                    json!({"account":{"type":"chatgpt","email":"private@example.invalid"}})
                }
                "model/list" => json!({"data":[{"model":"selected-model"}],"nextCursor":null}),
                "config/read" => json!({"config":{
                    "features":{"shell_tool":false,"hooks":false,"codex_hooks":false,"plugin_hooks":false},
                    "mcp_servers":{"fixture":{"enabled":false,"env":{"private":"fixture-secret"}}},
                    "plugins":{"fixture":{"enabled":false}},"project_doc_max_bytes":0,
                    "memories":{"use_memories":false,"generate_memories":false}
                }}),
                _ => panic!("Metadata probe attempted an unexpected method"),
            };
            ws.send(Message::Text(
                json!({"id":request["id"],"result":result})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            if method == "config/read" {
                break;
            }
        }
        seen
    });
    let mut config = AssistantConfig::codex(endpoint, "/tmp/fixture");
    config.enabled = true;
    let probe = AssistantClient::new(config)
        .unwrap()
        .probe_metadata(&CancellationToken::new())
        .await
        .unwrap();
    assert!(probe.authenticated && probe.shell_tool_disabled && probe.hooks_disabled);
    assert!(probe.project_docs_disabled && probe.memories_disabled);
    assert_eq!(probe.enabled_mcp_servers, 0);
    assert_eq!(probe.enabled_plugins, 0);
    assert_eq!(probe.model_choices, ["selected-model"]);
    let debug = format!("{probe:?}");
    assert!(!debug.contains("private@example.invalid") && !debug.contains("fixture-secret"));
    assert_eq!(
        server.await.unwrap(),
        [
            "initialize",
            "initialized",
            "account/read",
            "model/list",
            "config/read"
        ]
    );
}

#[tokio::test]
async fn generate_rejects_api_key_account_before_creating_any_thread() {
    let (client, mut sent) = fixture(Script {
        api_key_account: true,
        ..Script::default()
    })
    .await;
    assert_eq!(
        generate(&client).await,
        Err(AssistantError::ChatgptAuthenticationRequired)
    );
    let mut methods = Vec::new();
    while let Some(request) = sent.recv().await {
        methods.push(request["method"].as_str().unwrap().to_owned());
    }
    assert_eq!(methods, ["initialize", "initialized", "account/read"]);
}
