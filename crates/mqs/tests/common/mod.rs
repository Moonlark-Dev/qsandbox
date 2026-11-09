//! Shared test harness: one in-process MQS server per test.

#![allow(dead_code)]

use futures_util::{SinkExt, StreamExt};
use mqs::config::ProcessConfig;
use mqs::server::{start, ServerHandle};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

pub struct Test {
    pub handle: ServerHandle,
    pub sim: String,
    pub control: String,
    pub token: String,
    _dir: tempfile::TempDir,
}

pub type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

impl Test {
    pub async fn start() -> Test {
        let dir = tempfile::tempdir().unwrap();
        let config = ProcessConfig {
            profile: "test".into(),
            home: dir.path().to_path_buf(),
            host: "127.0.0.1".into(),
            sim_port: 0,
            control_port: 0,
            control_token: "test-token".into(),
            log_level: "warn".into(),
            control_origins: Vec::new(),
            expose_docs: true,
            gui_url: String::new(),
            public_host: String::new(),
        };
        let handle = start(config).await.expect("server starts");
        let sim = format!("http://127.0.0.1:{}", handle.sim_addr.port());
        let control = format!("http://127.0.0.1:{}", handle.control_addr.port());
        Test {
            handle,
            sim,
            control,
            token: "test-token".into(),
            _dir: dir,
        }
    }

    pub fn home(&self) -> &std::path::Path {
        self._dir.path()
    }

    pub fn world(&self) -> &std::sync::Arc<mqs::world::World> {
        &self.handle.world
    }

    pub fn http(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap()
    }

    pub async fn token(&self, app_id: &str) -> String {
        let response = self
            .http()
            .post(format!("{}/app/getAppAccessToken", self.sim))
            .json(&json!({"appId": app_id, "clientSecret": format!("mqs-secret-{app_id}")}))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success(), "token request failed");
        let body: Value = response.json().await.unwrap();
        body["access_token"].as_str().unwrap().to_string()
    }

    pub async fn bot_headers(&self, app_id: &str) -> String {
        format!("QQBot {}", self.token(app_id).await)
    }

    pub async fn ctl(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self
            .http()
            .request(method.parse().unwrap(), format!("{}{}", self.control, path))
            .header("x-mqs-token", &self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let value = response.json().await.unwrap_or(Value::Null);
        (status, value)
    }

    pub async fn ctl_ok(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (status, value) = self.ctl(method, path, body).await;
        assert!(
            (200..300).contains(&status),
            "{method} {path} -> {status}: {value}"
        );
        value
    }

    pub async fn settings(&self, patch: Value) {
        self.ctl_ok("PATCH", "/_mqs/settings", Some(patch)).await;
    }

    pub async fn group_id(&self) -> String {
        let groups = self.ctl_ok("GET", "/_mqs/groups", None).await;
        groups[0]["id"].as_str().unwrap().to_string()
    }

    pub async fn user_id(&self) -> String {
        let users = self.ctl_ok("GET", "/_mqs/users?available=true", None).await;
        users[0]["id"].as_str().unwrap().to_string()
    }

    pub async fn conversations(&self) -> Value {
        self.ctl_ok("GET", "/_mqs/conversations", None).await
    }

    pub async fn group_openid(&self, bot_id: &str, group_id: &str) -> String {
        self.world()
            .openid(bot_id, "group", group_id)
            .expect("group openid")
    }

    pub async fn user_openid(&self, bot_id: &str, user_id: &str) -> String {
        self.world()
            .openid(bot_id, "user", user_id)
            .expect("user openid")
    }

    pub async fn connect_gateway(&self, app_id: &str, intents: i64) -> (Ws, Value, String) {
        let token = self.token(app_id).await;
        let url = format!("ws://{}/websocket", self.sim.trim_start_matches("http://"));
        let (mut ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("ws connects");
        let hello = next_json(&mut ws).await;
        assert_eq!(hello["op"], 10);
        ws.send(Message::Text(
            json!({"op": 2, "d": {"token": format!("QQBot {token}"), "intents": intents, "shard": [0, 1]}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let ready = next_json(&mut ws).await;
        assert_eq!(ready["t"], "READY", "{ready}");
        (ws, ready["d"].clone(), token)
    }

    pub async fn send_as_user(&self, content: &str, at_bots: Option<Vec<&str>>) -> Value {
        let user = self.user_id().await;
        let group = self.group_id().await;
        let body = json!({
            "as_user": user,
            "to": format!("group:{group}"),
            "content": content,
            "at_bots": at_bots.map(|bots| bots.into_iter().map(str::to_string).collect::<Vec<_>>()),
        });
        self.ctl_ok("POST", "/_mqs/chat/send", Some(body)).await
    }
}

pub async fn next_json(ws: &mut Ws) -> Value {
    loop {
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("ws frame before timeout")
            .expect("ws open")
            .expect("ws frame ok");
        match message {
            Message::Text(text) => return serde_json::from_str(&text).expect("valid json frame"),
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(frame) => panic!("socket closed: {frame:?}"),
            other => panic!("unexpected frame: {other:?}"),
        }
    }
}

/// Spawn an echo bot: replies `echo: <text>` to group @ and C2C messages.
pub async fn spawn_echo_bot(test: &Test) -> tokio::task::JoinHandle<()> {
    let sim = test.sim.clone();
    let token = test.token("10000").await;
    tokio::spawn(async move {
        let url = format!("ws://{}/websocket", sim.trim_start_matches("http://"));
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let _ = next_json(&mut ws).await;
        ws.send(Message::Text(
            json!({"op": 2, "d": {"token": format!("QQBot {token}"), "intents": 1 << 25, "shard": [0, 1]}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let _ = next_json(&mut ws).await;
        let client = reqwest::Client::new();
        loop {
            let Some(Ok(message)) = ws.next().await else {
                break;
            };
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(frame) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if frame["op"] != 0 {
                continue;
            }
            let kind = frame["t"].as_str().unwrap_or("");
            if ![
                "GROUP_AT_MESSAGE_CREATE",
                "GROUP_MESSAGE_CREATE",
                "C2C_MESSAGE_CREATE",
            ]
            .contains(&kind)
            {
                continue;
            }
            let data = &frame["d"];
            if kind == "GROUP_MESSAGE_CREATE"
                && !data["mentions"]
                    .as_array()
                    .map(|items| items.iter().any(|m| m["is_you"].as_bool().unwrap_or(false)))
                    .unwrap_or(false)
            {
                continue;
            }
            let content = data["content"].as_str().unwrap_or("");
            let content = regex_lite_strip(content);
            let url = if kind == "C2C_MESSAGE_CREATE" {
                format!(
                    "{sim}/v2/users/{}/messages",
                    data["author"]["user_openid"].as_str().unwrap_or("")
                )
            } else {
                format!(
                    "{sim}/v2/groups/{}/messages",
                    data["group_openid"].as_str().unwrap_or("")
                )
            };
            let _ = client
                .post(url)
                .header("authorization", format!("QQBot {token}"))
                .json(&json!({"content": format!("echo: {content}"), "msg_id": data["id"]}))
                .send()
                .await;
        }
    })
}

fn regex_lite_strip(content: &str) -> String {
    // remove <@!openid> prefixes without pulling regex into the test crate
    let mut out = String::new();
    let mut rest = content;
    while let Some(start) = rest.find("<@!") {
        out.push_str(&rest[..start]);
        match rest[start..].find('>') {
            Some(end) => rest = &rest[start + end + 1..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}
