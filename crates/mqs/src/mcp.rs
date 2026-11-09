//! MCP server exposing the sandbox to agents (stdio).
//!
//! Every tool is a thin wrapper over the control API. If MQS is not running,
//! tools return a structured error instead of crashing the MCP session.

use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::config::{read_runtime, resolve_home};
use crate::db::i;

fn runtime() -> Result<(String, String), String> {
    let home = resolve_home(std::env::var("MQS_HOME").ok().as_deref());
    let profile = std::env::var("MQS_PROFILE").unwrap_or_else(|_| "default".into());
    let runtime = read_runtime(&home, &profile).map_err(|err| err.message)?;
    let port = runtime["control_port"].as_u64().unwrap_or(0);
    let token = runtime["token"].as_str().unwrap_or("").to_string();
    Ok((format!("http://127.0.0.1:{port}"), token))
}

async fn call(method: &str, path: &str, body: Option<Value>, params: Option<&Value>) -> Value {
    let Ok((base, token)) = runtime() else {
        return json!({"error": "no running MQS server found — start one with `mqs serve`", "hint": "mqs serve --profile <name>"});
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap_or_default();
    let mut request = client
        .request(
            method.parse().unwrap_or(reqwest::Method::GET),
            format!("{base}{path}"),
        )
        .header("x-mqs-token", token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    if let Some(params) = params.and_then(|p| p.as_object()) {
        request = request.query(params);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => return json!({"error": format!("cannot reach MQS: {err}")}),
    };
    if response.status().as_u16() == 204 {
        return json!({"ok": true});
    }
    let status = response.status();
    let value: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return json!({"error": value});
    }
    value
}

fn tools() -> Value {
    json!([
        {
            "name": "get_status",
            "description": "MQS 运行状态：端口、世界计数、网关连接、覆盖率",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "list_conversations",
            "description": "列出会话（群聊与单聊）",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "read_messages",
            "description": "读取会话最近消息",
            "inputSchema": {"type": "object", "properties": {
                "conversation_id": {"type": "string"},
                "limit": {"type": "integer"}
            }, "required": ["conversation_id"]}
        },
        {
            "name": "send_as_user",
            "description": "以虚拟用户身份发送消息。to 形如 group:g_000001 或 dm:10000:u_000001",
            "inputSchema": {"type": "object", "properties": {
                "as_user": {"type": "string"},
                "to": {"type": "string"},
                "text": {"type": "string"},
                "at_bots": {"type": "array", "items": {"type": "string"}},
                "no_at": {"type": "boolean"}
            }, "required": ["as_user", "to", "text"]}
        },
        {
            "name": "expect_event",
            "description": "等待事件（type 大小写不敏感，timeout 如 3s）",
            "inputSchema": {"type": "object", "properties": {
                "event_type": {"type": "string"},
                "timeout": {"type": "string"},
                "content_contains": {"type": "string"}
            }}
        },
        {
            "name": "inspect_gateway",
            "description": "查看网关连接与最近 WS 帧",
            "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer"}}}
        },
        {
            "name": "advance_clock",
            "description": "快进虚拟时钟，例如 6m / 2h",
            "inputSchema": {"type": "object", "properties": {"duration": {"type": "string"}}, "required": ["duration"]}
        },
        {
            "name": "create_random_users",
            "description": "创建随机虚拟用户",
            "inputSchema": {"type": "object", "properties": {
                "count": {"type": "integer"},
                "seed": {"type": "integer"},
                "group_id": {"type": "string"}
            }}
        },
        {
            "name": "snapshot",
            "description": "导出 profile 快照，返回本地 zip 路径",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "run_scenario",
            "description": "执行 YAML 场景（内置名称或路径）",
            "inputSchema": {"type": "object", "properties": {"name_or_path": {"type": "string"}}, "required": ["name_or_path"]}
        }
    ])
}

async fn call_tool(name: &str, args: &Value) -> Value {
    let string = |key: &str| {
        args.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let int = |key: &str| args.get(key).and_then(|v| v.as_i64());
    match name {
        "get_status" => call("GET", "/_mqs/status", None, None).await,
        "list_conversations" => call("GET", "/_mqs/conversations", None, None).await,
        "read_messages" => {
            let conversation_id = string("conversation_id");
            let limit = int("limit").unwrap_or(20);
            call(
                "GET",
                &format!("/_mqs/conversations/{conversation_id}/messages?limit={limit}"),
                None,
                None,
            )
            .await
        }
        "send_as_user" => {
            let mut body = json!({
                "as_user": string("as_user"),
                "to": string("to"),
                "content": string("text"),
            });
            if args.get("no_at").and_then(|v| v.as_bool()).unwrap_or(false) {
                body["at_bots"] = json!([]);
            } else if let Some(at_bots) = args.get("at_bots") {
                body["at_bots"] = at_bots.clone();
            }
            call("POST", "/_mqs/chat/send", Some(body), None).await
        }
        "expect_event" => {
            let event_type = string("event_type").to_uppercase();
            let timeout = args
                .get("timeout")
                .and_then(|v| v.as_str())
                .unwrap_or("3s")
                .to_string();
            let needle = string("content_contains");
            let status = call("GET", "/_mqs/status", None, None).await;
            if status.get("error").is_some() {
                return status;
            }
            let mut baseline = status
                .get("max_event_seq")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let timeout_ms = crate::clock::parse_duration(&timeout).unwrap_or(3000);
            let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms as u64);
            while std::time::Instant::now() < deadline {
                let data = call(
                    "GET",
                    &format!("/_mqs/debug/events?since_seq={baseline}&limit=200"),
                    None,
                    None,
                )
                .await;
                if data.get("error").is_some() {
                    return data;
                }
                for event in data["items"].as_array().cloned().unwrap_or_default() {
                    baseline = baseline.max(i(
                        &event.as_object().cloned().unwrap_or_default().clone(),
                        "seq",
                    ));
                    if !event_type.is_empty()
                        && event.get("type").and_then(|v| v.as_str()) != Some(event_type.as_str())
                    {
                        continue;
                    }
                    if !needle.is_empty() && !event["payload"].to_string().contains(&needle) {
                        continue;
                    }
                    return json!({"matched": true, "event": event});
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            json!({"matched": false, "error": format!("timeout after {timeout}")})
        }
        "inspect_gateway" => {
            let limit = int("limit").unwrap_or(20);
            let status = call("GET", "/_mqs/status", None, None).await;
            let frames = call(
                "GET",
                &format!("/_mqs/debug/frames?limit={limit}&kind=ws"),
                None,
                None,
            )
            .await;
            json!({"gateway": status.get("gateway").cloned().unwrap_or(Value::Null), "frames": frames})
        }
        "advance_clock" => {
            call(
                "POST",
                "/_mqs/clock/advance",
                Some(json!({"duration": string("duration")})),
                None,
            )
            .await
        }
        "create_random_users" => {
            call(
                "POST",
                "/_mqs/users/random",
                Some(json!({
                    "count": int("count").unwrap_or(1),
                    "seed": args.get("seed").cloned().unwrap_or(Value::Null),
                    "group_id": args.get("group_id").cloned().unwrap_or(Value::Null),
                })),
                None,
            )
            .await
        }
        "snapshot" => {
            let Ok((base, token)) = runtime() else {
                return json!({"error": "no running MQS server found"});
            };
            let client = reqwest::Client::new();
            let response = match client
                .post(format!("{base}/_mqs/snapshot"))
                .header("x-mqs-token", token)
                .timeout(Duration::from_secs(60))
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => return json!({"error": err.to_string()}),
            };
            if !response.status().is_success() {
                return json!({"error": response.text().await.unwrap_or_default()});
            }
            let bytes = response.bytes().await.unwrap_or_default();
            let home = resolve_home(std::env::var("MQS_HOME").ok().as_deref());
            let profile = std::env::var("MQS_PROFILE").unwrap_or_else(|_| "default".into());
            let target_dir = home.join("profiles").join(profile).join("snapshots");
            let _ = std::fs::create_dir_all(&target_dir);
            let target =
                target_dir.join(format!("mqs-{}.mqsprofile", chrono::Utc::now().timestamp()));
            let _ = std::fs::write(&target, &bytes);
            json!({"path": target.to_string_lossy(), "bytes": bytes.len()})
        }
        "run_scenario" => {
            let name_or_path = string("name_or_path");
            let body = match crate::scenario::builtin(&name_or_path) {
                Some(text) => json!({"script": text}),
                None => json!({"path": name_or_path}),
            };
            call("POST", "/_mqs/scenarios/run", Some(body), None).await
        }
        other => json!({"error": format!("unknown tool: {other}")}),
    }
}

pub async fn serve_stdio() {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(err) => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {"code": -32700, "message": format!("parse error: {err}")},
                });
                let _ = stdout.write_all(format!("{}\n", response).as_bytes()).await;
                let _ = stdout.flush().await;
                continue;
            }
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let params = request.get("params").cloned().unwrap_or(json!({}));
        let result = match method.as_str() {
            "initialize" => Ok(json!({
                "protocolVersion": params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("2024-11-05"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mqs", "version": crate::config::VERSION},
                "instructions": "MQS is a local QQ Bot API v2 sandbox. Use send_as_user to drive conversations, expect_event to assert what the bot received, and inspect_gateway to debug frames.",
            })),
            "notifications/initialized" | "initialized" => Ok(Value::Null),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                let value = call_tool(&name, &arguments).await;
                let is_error = value.get("error").is_some();
                Ok(json!({
                    "content": [{"type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default()}],
                    "isError": is_error,
                }))
            }
            other => Err((-32601, format!("method not found: {other}"))),
        };
        if method == "notifications/initialized" || method == "initialized" {
            continue;
        }
        let response = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        };
        let _ = stdout.write_all(format!("{}\n", response).as_bytes()).await;
        let _ = stdout.flush().await;
    }
}
