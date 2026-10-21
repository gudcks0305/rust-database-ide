//! Opt-in tests for fixed project-owned loopback fixtures only.
use dbide_mcp::DatabaseMcpServer;
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine, TlsMode};
use rmcp::{
    ClientHandler, ServiceExt,
    model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
};
use serde_json::json;

struct Client;
impl ClientHandler for Client {
    fn get_info(&self) -> ClientConfig {
        let mut config = ClientConfig::default();
        config.protocol_version = ProtocolVersion::V_2025_11_25;
        config
    }
}
async fn smoke(profile: ConnectionProfile) {
    assert_eq!(
        std::env::var("DBIDE_MCP_FIXTURE").as_deref(),
        Ok("owned-loopback"),
        "Set DBIDE_MCP_FIXTURE=owned-loopback only for project-owned fixtures"
    );
    let pg = profile.engine == DatabaseEngine::PostgreSql;
    let server = DatabaseMcpServer::new(ConnectionRequest {
        profile,
        password: "dbide_fixture_only".into(),
    })
    .unwrap();
    let (a, b) = tokio::io::duplex(1024 * 1024);
    let task = tokio::spawn(async move { server.serve(a).await.unwrap().waiting().await.unwrap() });
    let client = Client.serve(b).await.unwrap();
    for name in if pg {
        vec!["query.read", "schema.read", "query.explain"]
    } else {
        vec!["query.read", "schema.read"]
    } {
        let mut params = CallToolRequestParams::new(name);
        params.arguments = Some(
            if name == "schema.read" {
                json!({})
            } else {
                json!({"sql":"SELECT 1 AS n"})
            }
            .as_object()
            .unwrap()
            .clone(),
        );
        let result = client.call_tool(params).await.unwrap();
        assert_ne!(
            serde_json::to_value(&result).unwrap()["isError"],
            true,
            "{name}: {result:?}"
        );
    }
    for (sql, expected_rows) in [
        ("SELECT 1 AS x, 2 AS x", json!([["1", "2"]])),
        (
            "SELECT 1 AS x, 2 AS x UNION ALL SELECT 3, 4 ORDER BY 1 LIMIT 1 OFFSET 1",
            json!([["3", "4"]]),
        ),
        (
            "SELECT 1 AS x, 2 AS x UNION ALL SELECT 3, 4 ORDER BY 1 LIMIT 0",
            json!([]),
        ),
    ] {
        let mut params = CallToolRequestParams::new("query.read");
        params.arguments = Some(json!({"sql":sql}).as_object().unwrap().clone());
        let response = client.call_tool(params).await.unwrap();
        let response = serde_json::to_value(response).unwrap();
        assert_ne!(response["isError"], true, "{sql}: {response}");
        let data: serde_json::Value =
            serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(data["columns"], json!(["x", "x"]), "{sql}");
        assert_eq!(data["rows"], expected_rows, "{sql}");
        assert_eq!(data["truncated"], false, "{sql}");
    }
    let recursive = "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<300) SELECT x FROM n ORDER BY x";
    for (limit, offset, count, truncated) in [
        (2, 1, 2, false),
        (128, 0, 128, false),
        (129, 0, 128, true),
        (300, 0, 128, true),
    ] {
        let sql = format!("{recursive} LIMIT {limit} OFFSET {offset}");
        let mut params = CallToolRequestParams::new("query.read");
        params.arguments = Some(json!({"sql":sql}).as_object().unwrap().clone());
        let response = serde_json::to_value(client.call_tool(params).await.unwrap()).unwrap();
        assert_ne!(response["isError"], true, "{sql}: {response}");
        let data: serde_json::Value =
            serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(data["rows"].as_array().unwrap().len(), count, "{sql}");
        assert_eq!(data["rows"][0][0], (offset + 1).to_string(), "{sql}");
        assert_eq!(data["truncated"], truncated, "{sql}");
    }
    if !pg {
        let mut params = CallToolRequestParams::new("query.read");
        params.arguments = Some(
            json!({"sql":format!("{recursive} LIMIT 1, 2")})
                .as_object()
                .unwrap()
                .clone(),
        );
        let response = serde_json::to_value(client.call_tool(params).await.unwrap()).unwrap();
        assert_ne!(response["isError"], true, "{response}");
        let data: serde_json::Value =
            serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(data["rows"], json!([["2"], ["3"]]));
        // Rejection occurs before native submission, independently of sql_mode.
        let mut params = CallToolRequestParams::new("query.read");
        params.arguments = Some(json!({"sql":r"SELECT 'a\b'"}).as_object().unwrap().clone());
        let response = serde_json::to_value(client.call_tool(params).await.unwrap()).unwrap();
        assert_eq!(response["isError"], true);
    }
    client.cancel().await.unwrap();
    task.await.unwrap();
}
#[tokio::test]
#[ignore = "requires explicitly opted-in owned PostgreSQL fixture on 127.0.0.1:55439"]
async fn owned_postgres_mcp_query_catalog_and_plan() {
    smoke(ConnectionProfile::fixture()).await;
}
#[tokio::test]
#[ignore = "requires explicitly opted-in owned MySQL fixture on 127.0.0.1:55441"]
async fn owned_mysql_mcp_query_and_catalog() {
    smoke(ConnectionProfile {
        engine: DatabaseEngine::MySql,
        host: "127.0.0.1".into(),
        port: 55441,
        database: "dbide_fixture".into(),
        username: "root".into(),
        tls_mode: TlsMode::Disable,
        connect_timeout_secs: 3,
        ..ConnectionProfile::default()
    })
    .await;
}
