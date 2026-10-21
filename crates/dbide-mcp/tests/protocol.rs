use dbide_mcp::{DatabaseMcpServer, MAX_RESULT_BYTES, request_from_file};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine};
use rmcp::{
    ClientHandler, ServiceExt,
    model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

struct LegacyClient;
impl ClientHandler for LegacyClient {
    fn get_info(&self) -> ClientConfig {
        let mut config = ClientConfig::default();
        config.protocol_version = ProtocolVersion::V_2025_11_25;
        config
    }
}
fn fixture() -> (tempfile::TempDir, ConnectionRequest) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("owned-fixture.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE items(id INTEGER PRIMARY KEY, label TEXT); INSERT INTO items VALUES (1, '한글'), (2, 'fixture'); WITH RECURSIVE n(x) AS (SELECT 3 UNION ALL SELECT x+1 FROM n WHERE x<300) INSERT INTO items SELECT x, 'bounded' FROM n;").unwrap();
    db.execute_batch("CREATE TABLE big(value TEXT)").unwrap();
    db.execute("INSERT INTO big VALUES (?1)", ["\n\\".repeat(100_000)])
        .unwrap();
    drop(db);
    let profile = ConnectionProfile {
        id: "mcp-owned-fixture".into(),
        engine: DatabaseEngine::Sqlite,
        sqlite_path: Some(path.to_str().unwrap().into()),
        sqlite_create: true,
        read_only: false,
        ..ConnectionProfile::default()
    };
    (
        dir,
        ConnectionRequest {
            profile,
            password: "fixture-password-never-disclose".into(),
        },
    )
}
fn text_json(result: &impl serde::Serialize) -> Value {
    let value = serde_json::to_value(result).unwrap();
    serde_json::from_str(value["content"][0]["text"].as_str().unwrap()).unwrap()
}
fn params(name: &str, value: Value) -> CallToolRequestParams {
    let mut params = CallToolRequestParams::new(name.to_owned());
    params.arguments = Some(value.as_object().unwrap().clone());
    params
}

#[tokio::test]
async fn mcp_initialize_tools_and_calls_are_bounded_read_only_and_scoped() {
    let (_dir, request) = fixture();
    let original = request.profile.sqlite_path.clone().unwrap();
    let secret = request.password.clone();
    let (server_io, client_io) = tokio::io::duplex(1024 * 1024);
    let server = DatabaseMcpServer::new(request).unwrap();
    let task = tokio::spawn(async move {
        server
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap()
    });
    let client = LegacyClient.serve(client_io).await.unwrap();
    assert_eq!(
        client
            .peer_info()
            .unwrap()
            .server_info
            .as_ref()
            .unwrap()
            .name,
        "dbide-mcp"
    );
    let tools = client.list_tools(None).await.unwrap();
    let mut names: Vec<_> = tools.tools.iter().map(|t| t.name.as_ref()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "connectioninfo",
            "query.explain",
            "query.read",
            "schema.read",
            "serverinfo",
            "sql.format"
        ]
    );
    assert!(
        tools
            .tools
            .iter()
            .all(|t| t.annotations.as_ref().unwrap().read_only_hint == Some(true))
    );
    let info = client
        .call_tool(params("connectioninfo", json!({})))
        .await
        .unwrap();
    let encoded = serde_json::to_string(&info).unwrap();
    assert!(!encoded.contains(&secret));
    assert!(!encoded.contains(&original));
    assert_eq!(text_json(&info)["read_only"], true);
    let query = client
        .call_tool(params(
            "query.read",
            json!({"sql":"SELECT * FROM items ORDER BY id"}),
        ))
        .await
        .unwrap();
    let data = text_json(&query);
    assert_eq!(data["rows"].as_array().unwrap().len(), 128);
    assert_eq!(data["truncated"], true);
    assert_eq!(data["rows"][0][1], "한글");
    for (sql, columns, expected_rows) in [
        (
            "SELECT 1 AS x, 2 AS x",
            json!(["x", "x"]),
            json!([["1", "2"]]),
        ),
        (
            "SELECT 1 AS x, 2 AS x UNION ALL SELECT 3, 4 ORDER BY 1 LIMIT 1 OFFSET 1",
            json!(["x", "x"]),
            json!([["3", "4"]]),
        ),
        (
            "SELECT id FROM items ORDER BY id LIMIT 2 OFFSET 1",
            json!(["id"]),
            json!([["2"], ["3"]]),
        ),
        (
            "SELECT id FROM items ORDER BY id LIMIT 1, 2",
            json!(["id"]),
            json!([["2"], ["3"]]),
        ),
        ("SELECT id FROM items LIMIT 0", json!(["id"]), json!([])),
    ] {
        let response = client
            .call_tool(params("query.read", json!({"sql":sql})))
            .await
            .unwrap();
        let data = text_json(&response);
        assert_eq!(data["columns"], columns, "{sql}");
        assert_eq!(data["rows"], expected_rows, "{sql}");
        assert_eq!(data["truncated"], false, "{sql}");
    }
    for (limit, count, truncated) in [(128, 128, false), (129, 128, true), (300, 128, true)] {
        let response = client
            .call_tool(params(
                "query.read",
                json!({"sql":format!("SELECT id FROM items ORDER BY id LIMIT {limit}")}),
            ))
            .await
            .unwrap();
        let data = text_json(&response);
        assert_eq!(data["rows"].as_array().unwrap().len(), count);
        assert_eq!(data["truncated"], truncated);
    }
    let schema = client
        .call_tool(params("schema.read", json!({})))
        .await
        .unwrap();
    assert!(serde_json::to_string(&schema).unwrap().contains("items"));
    let format = client
        .call_tool(params("sql.format", json!({"sql":"select id from items"})))
        .await
        .unwrap();
    assert!(
        text_json(&format)["sql"]
            .as_str()
            .unwrap()
            .contains("SELECT")
    );
    for sql in [
        "DELETE FROM items",
        "SELECT 1; DELETE FROM items",
        "WITH x AS (DELETE FROM items RETURNING *) SELECT * FROM x",
        "SELECT load_extension('x')",
        "SELECT * INTO replacement FROM items",
    ] {
        let result = client
            .call_tool(params("query.read", json!({"sql":sql})))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap()["isError"],
            true,
            "{sql}"
        );
    }
    let unsupported = client
        .call_tool(params("query.explain", json!({"sql":"SELECT 1"})))
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(unsupported).unwrap()["isError"], true);
    assert!(
        client
            .call_tool(params(
                "query.read",
                json!({"sql":"SELECT 1","path":"/tmp/forbidden"})
            ))
            .await
            .is_err()
    );
    let too_long = client
        .call_tool(params("query.read", json!({"sql":"x".repeat(65537)})))
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(too_long).unwrap()["isError"], true);
    let huge = client
        .call_tool(params("query.read", json!({"sql":"SELECT value FROM big"})))
        .await
        .unwrap();
    assert!(serde_json::to_vec(&huge).unwrap().len() < MAX_RESULT_BYTES);
    assert_eq!(text_json(&huge)["truncated"], true);
    let timeout = client.call_tool(params("query.read", json!({"sql":"WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n","timeout_secs":1}))).await.unwrap();
    assert!(
        serde_json::to_string(&timeout)
            .unwrap()
            .contains("timed out")
    );
    let next = client
        .call_tool(params(
            "query.read",
            json!({"sql":"SELECT count(*) FROM items"}),
        ))
        .await
        .unwrap();
    assert_eq!(text_json(&next)["rows"][0][0], "300");
    client.cancel().await.unwrap();
    task.await.unwrap();
    let db = rusqlite::Connection::open(original).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        300
    );
}

#[tokio::test]
async fn cancellation_retires_owned_session_and_concurrency_has_no_queue() {
    let (_dir, request) = fixture();
    let (server_io, client_io) = tokio::io::duplex(1024 * 1024);
    let server = DatabaseMcpServer::new(request).unwrap();
    let task = tokio::spawn(async move {
        server
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap()
    });
    let client = LegacyClient.serve(client_io).await.unwrap();
    let peer = client.peer().clone();
    let long = tokio::spawn(async move {
        peer.call_tool(params("query.read", json!({"sql":"WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n","timeout_secs":30}))).await
    });
    let mut saw_busy = false;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let response = client
            .call_tool(params("query.read", json!({"sql":"SELECT 1"})))
            .await
            .unwrap();
        if serde_json::to_string(&response)
            .unwrap()
            .contains("Another database call is running")
        {
            saw_busy = true;
            break;
        }
    }
    assert!(saw_busy);
    long.abort(); // SDK's request guard emits MCP notifications/cancelled.
    assert!(long.await.unwrap_err().is_cancelled());
    let mut recovered = false;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let response = client
            .call_tool(params("query.read", json!({"sql":"SELECT 1"})))
            .await
            .unwrap();
        if serde_json::to_value(&response).unwrap()["isError"] != true {
            assert_eq!(text_json(&response)["rows"][0][0], "1");
            recovered = true;
            break;
        }
    }
    assert!(recovered, "cancelled call retained its concurrency permit");
    client.cancel().await.unwrap();
    task.await.unwrap();
}

#[test]
fn explicit_profile_files_are_bounded_validated_and_exactly_selected() {
    let (dir, request) = fixture();
    let path = dir.path().join("selected.json");
    std::fs::write(&path, serde_json::to_vec(&request.profile).unwrap()).unwrap();
    let loaded = request_from_file(&path, "mcp-owned-fixture", "explicit-secret".into()).unwrap();
    assert_eq!(loaded.profile.id, request.profile.id);
    assert_eq!(loaded.password, "explicit-secret");
    assert!(request_from_file(&path, "other", String::new()).is_err());
    let mut workspace = dbide_profiles::WorkspaceState::default();
    workspace.profiles.push(request.profile.clone());
    std::fs::write(&path, serde_json::to_vec(&workspace).unwrap()).unwrap();
    assert!(request_from_file(&path, "mcp-owned-fixture", String::new()).is_ok());
    workspace.profiles.push(request.profile.clone());
    std::fs::write(&path, serde_json::to_vec(&workspace).unwrap()).unwrap();
    assert!(request_from_file(&path, "mcp-owned-fixture", String::new()).is_err());
    let mut value = serde_json::to_value(&request.profile).unwrap();
    value["password"] = json!("should-never-be-in-profile");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(request_from_file(&path, "mcp-owned-fixture", String::new()).is_err());
    std::fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
    assert!(request_from_file(&path, "mcp-owned-fixture", String::new()).is_err());
    assert!(
        request_from_file(
            &dir.path().join("missing"),
            "mcp-owned-fixture",
            String::new()
        )
        .is_err()
    );
}

#[test]
fn stdio_binary_protocol_stdout_is_clean_and_configuration_is_explicit() {
    let output = Command::new(env!("CARGO_BIN_EXE_dbide-mcp"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("--profile-file")
    );
    let (dir, request) = fixture();
    let path = dir.path().join("selected.json");
    std::fs::write(&path, serde_json::to_vec(&request.profile).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_dbide-mcp"))
        .args([
            "--profile-file",
            path.to_str().unwrap(),
            "--profile-id",
            "mcp-owned-fixture",
        ])
        .env_remove("DBIDE_MCP_PASSWORD")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    for request in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"owned-fixture","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"query.read","arguments":{"sql":"SELECT 1 AS n"}}}),
    ] {
        writeln!(input, "{request}").unwrap();
        input.flush().unwrap();
        if request.get("id").is_some() {
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            let response: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(response["id"], request["id"]);
            assert!(response.get("error").is_none(), "{response}");
            if request["id"] == 3 {
                assert!(
                    response["result"]["content"][0]["text"]
                        .as_str()
                        .unwrap()
                        .contains("rows")
                );
            }
        }
    }
    drop(input);
    assert!(child.wait().unwrap().success());
}
