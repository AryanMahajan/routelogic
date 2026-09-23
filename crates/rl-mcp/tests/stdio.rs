//! The server as an agent sees it: the real binary, spawned, spoken to over stdio.
//!
//! A tiny FastAPI-shaped project gives the scan something to find, and a local server
//! plays its API: `POST /auth/login` hands out a token (and, carelessly, echoes the admin
//! password back, which is a secret the agent must never see), `GET /users/me` wants the
//! token. The test builds a flow the way an agent would — list, describe, try, save, run —
//! and checks each step's answer.

use rl_core::RouteLogic;
use rl_workspace::{Environment, WorkspaceKind};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{json, Value};
use std::path::Path;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PASSWORD: &str = "hunter2-very-secret";

async fn api() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let line = request.lines().next().unwrap_or_default().to_string();
                let authed = request
                    .lines()
                    .any(|l| l.eq_ignore_ascii_case("authorization: Bearer tok-xyz"));
                let (status, body) = if line.starts_with("POST /auth/login") {
                    (
                        200,
                        format!(r#"{{"accessToken":"tok-xyz","debug":"password was {PASSWORD}"}}"#),
                    )
                } else if line.starts_with("GET /users/me") && authed {
                    (200, r#"{"id":7,"name":"Ann"}"#.to_string())
                } else if line.starts_with("GET /users/me") {
                    (401, r#"{"detail":"missing token"}"#.to_string())
                } else {
                    (404, r#"{"detail":"no"}"#.to_string())
                };
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    base
}

/// A project on disk with a workspace whose `local` environment points at `base`.
fn project(base: &str, data: &Path) -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("requirements.txt"), "fastapi\n").unwrap();
    std::fs::write(
        dir.path().join("main.py"),
        "from fastapi import FastAPI\n\
         app = FastAPI()\n\
         \n\
         @app.post(\"/auth/login\")\n\
         def login(body: dict): ...\n\
         \n\
         @app.get(\"/users/me\")\n\
         def me(): ...\n",
    )
    .unwrap();

    let mut app = RouteLogic::with_data_dir(data);
    app.create_workspace(dir.path(), "demo", WorkspaceKind::Project)
        .unwrap();
    let mut local = Environment::new("local");
    local.set("base_url", base);
    app.save_environment(&local).unwrap();
    app.set_active_environment(Some("local")).unwrap();
    app.set_secret("admin_password", PASSWORD).unwrap();
    dir
}

async fn connect(project: &Path, data: &Path) -> RunningService<RoleClient, ()> {
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(env!("CARGO_BIN_EXE_routelogic-mcp")).configure(|cmd| {
            cmd.arg("--workspace")
                .arg(project)
                .env(rl_workspace::DATA_DIR_ENV, data);
        }),
    )
    .unwrap();
    ().serve(transport).await.unwrap()
}

async fn call(client: &RunningService<RoleClient, ()>, tool: &str, args: Value) -> CallToolResult {
    client
        .call_tool(
            CallToolRequestParams::new(tool.to_string())
                .with_arguments(args.as_object().cloned().unwrap_or_default()),
        )
        .await
        .unwrap_or_else(|e| panic!("{tool} failed at the protocol level: {e}"))
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_error(result: &CallToolResult) -> bool {
    result.is_error == Some(true)
}

#[tokio::test]
async fn an_agent_can_read_the_api_try_it_and_write_and_run_a_flow() {
    let base = api().await;
    let data = TempDir::new().unwrap();
    let project = project(&base, data.path());
    let client = connect(project.path(), data.path()).await;

    // The tools, and the manual that comes with them.
    let tools: Vec<String> = client
        .list_all_tools()
        .await
        .unwrap()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    for expected in [
        "workspace_info",
        "list_endpoints",
        "describe_endpoint",
        "prepare_request",
        "send_request",
        "list_flows",
        "get_flow",
        "save_flow",
        "run_flow",
    ] {
        assert!(
            tools.contains(&expected.to_string()),
            "{expected} in {tools:?}"
        );
    }
    let instructions = client
        .peer_info()
        .and_then(|info| info.instructions.clone())
        .unwrap_or_default();
    assert!(instructions.contains("{{base_url}}"), "{instructions}");

    // What the workspace offers, secrets named but never shown.
    let info = text(&call(&client, "workspace_info", json!({})).await);
    assert!(info.contains("\"local\""), "{info}");
    assert!(info.contains("{{secret:admin_password}}"), "{info}");
    assert!(!info.contains(PASSWORD));
    assert!(
        info.contains("127.0.0.1"),
        "loopback is listed as allowed: {info}"
    );

    // The API, read from source.
    let listing = text(&call(&client, "list_endpoints", json!({})).await);
    assert!(listing.contains("POST    /auth/login"), "{listing}");
    assert!(listing.contains("GET     /users/me"), "{listing}");
    assert!(
        listing.contains("main.py:"),
        "the handler's file: {listing}"
    );

    let described = call(
        &client,
        "describe_endpoint",
        json!({"id": "post /auth/login"}),
    )
    .await;
    assert!(!is_error(&described), "{}", text(&described));
    let described: Value = serde_json::from_str(&text(&described)).unwrap();
    assert_eq!(described["flow_node"]["id"], "post_auth_login");
    assert_eq!(
        described["flow_node"]["request"]["url"],
        "{{base_url}}/auth/login"
    );

    // Try the login: the real field name comes back, the password does not.
    let sent = call(
        &client,
        "send_request",
        json!({"request": {"method": "POST", "url": "{{base_url}}/auth/login"}}),
    )
    .await;
    let sent_text = text(&sent);
    assert!(!is_error(&sent), "{sent_text}");
    assert!(sent_text.contains("accessToken"), "{sent_text}");
    assert!(
        sent_text.contains("{{secret:admin_password}}"),
        "{sent_text}"
    );
    assert!(!sent_text.contains(PASSWORD), "{sent_text}");

    // Somewhere it may not go: refused, with the fix, as a tool error.
    let refused = call(
        &client,
        "send_request",
        json!({"request": {"method": "DELETE", "url": "https://api.example.invalid/users/7"}}),
    )
    .await;
    assert!(is_error(&refused));
    assert!(text(&refused).contains("agent.allow"), "{}", text(&refused));

    // Write the flow the way an agent would: words for ids, no positions.
    let flow = json!({
        "name": "login then me",
        "nodes": [
            {"id": "login", "type": "request",
             "request": {"method": "POST", "url": "{{base_url}}/auth/login"},
             "extract": [{"name": "token", "from": "body", "path": "accessToken"}],
             "assert": [{"from": "status", "op": "equals", "expected": "200"}]},
            {"id": "me", "type": "request",
             "request": {"method": "GET", "url": "{{base_url}}/users/me",
                         "headers": [{"key": "Authorization", "value": "Bearer {{token}}"}]},
             "assert": [{"from": "body", "path": "name", "op": "equals", "expected": "Ann"}]}
        ],
        "edges": [{"from": "login", "to": "me"}]
    });
    let saved = call(&client, "save_flow", json!({"flow": flow})).await;
    assert!(!is_error(&saved), "{}", text(&saved));
    let saved: Value = serde_json::from_str(&text(&saved)).unwrap();
    assert_eq!(saved["warnings"], json!([]));
    let on_disk = std::fs::read_to_string(saved["path"].as_str().unwrap()).unwrap();
    assert!(on_disk.contains("position:"), "laid out on save: {on_disk}");

    // Saving over it needs saying so.
    let again = call(&client, "save_flow", json!({"flow": flow})).await;
    assert!(is_error(&again));
    assert!(text(&again).contains("overwrite"));

    let run = call(&client, "run_flow", json!({"name": "login then me"})).await;
    let run: Value = serde_json::from_str(&text(&run)).unwrap();
    assert_eq!(run["passed"], true, "{run:#}");
    assert_eq!(run["steps"][0]["extracted"], json!([["token", "tok-xyz"]]));

    // The mistake agents make most — capture in one step, use in another, no edge — is
    // named before it runs.
    let mut unwired = flow.clone();
    unwired["name"] = json!("unwired");
    unwired["edges"] = json!([]);
    unwired["nodes"] = json!([flow["nodes"][1].clone(), flow["nodes"][0].clone()]);
    let warned = call(&client, "save_flow", json!({"flow": unwired})).await;
    assert!(
        text(&warned).contains("add an edge from `login` to `me`"),
        "{}",
        text(&warned)
    );

    let flows = text(&call(&client, "list_flows", json!({})).await);
    assert!(
        flows.contains("login then me") && flows.contains("unwired"),
        "{flows}"
    );

    client.cancel().await.unwrap();
}
