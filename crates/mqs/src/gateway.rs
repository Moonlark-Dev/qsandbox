//! QQ gateway protocol: hello/identify/ready/heartbeat/resume, intents and sharding.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Weak};
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::db::{i, s, Row};
use crate::ids::new_session_id;
use crate::world::World;

pub const OP_DISPATCH: i64 = 0;
pub const OP_HEARTBEAT: i64 = 1;
pub const OP_IDENTIFY: i64 = 2;
pub const OP_RESUME: i64 = 6;
pub const OP_INVALID_SESSION: i64 = 9;
pub const OP_HELLO: i64 = 10;
pub const OP_HEARTBEAT_ACK: i64 = 11;
pub const OP_HTTP_CALLBACK_ACK: i64 = 12;

pub const CLOSE_AUTH_FAILED: u16 = 4004;
pub const CLOSE_INVALID_SESSION: u16 = 4006;
pub const CLOSE_SESSION_TIMEOUT: u16 = 4009;
pub const CLOSE_INVALID_SHARD: u16 = 4010;

pub fn intent_for(event_type: &str) -> Option<i64> {
    Some(match event_type {
        "GROUP_AT_MESSAGE_CREATE"
        | "GROUP_MESSAGE_CREATE"
        | "C2C_MESSAGE_CREATE"
        | "GROUP_ADD_ROBOT"
        | "GROUP_DEL_ROBOT"
        | "GROUP_MSG_REJECT"
        | "GROUP_MSG_RECEIVE" => 1 << 25,
        "INTERACTION_CREATE" => 1 << 26,
        _ => return None,
    })
}

struct Conn {
    id: u64,
    bot_id: String,
    session_id: String,
    intents: i64,
    shard_id: i64,
    shard_count: i64,
    last_seq: i64,
    last_seen: Instant,
    identified: bool,
    tx: mpsc::UnboundedSender<Message>,
}

#[derive(Default)]
struct Inner {
    conns: HashMap<u64, Conn>,
}

pub struct GatewayManager {
    world: Weak<World>,
    inner: Mutex<Inner>,
    next_id: AtomicU64,
}

impl GatewayManager {
    pub fn new(world: Weak<World>) -> Self {
        Self {
            world,
            inner: Mutex::new(Inner::default()),
            next_id: AtomicU64::new(1),
        }
    }

    fn world(&self) -> Option<std::sync::Arc<World>> {
        self.world.upgrade()
    }

    pub fn is_online(&self, bot_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .conns
            .values()
            .any(|conn| conn.identified && conn.bot_id == bot_id)
    }

    pub fn online_bots(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        let mut bots: Vec<String> = inner
            .conns
            .values()
            .filter(|conn| conn.identified)
            .map(|conn| conn.bot_id.clone())
            .collect();
        bots.sort();
        bots.dedup();
        bots
    }

    pub fn stats(&self) -> Value {
        // `std::sync::Mutex` is not reentrant: everything must come from this one
        // guard (calling `online_bots()` here would deadlock the status endpoint).
        let inner = self.inner.lock().unwrap();
        let identified: Vec<&Conn> = inner
            .conns
            .values()
            .filter(|conn| conn.identified)
            .collect();
        let mut sessions: Vec<&str> = identified.iter().map(|c| c.session_id.as_str()).collect();
        sessions.sort();
        sessions.dedup();
        let mut bots: Vec<String> = identified.iter().map(|c| c.bot_id.clone()).collect();
        bots.sort();
        bots.dedup();
        json!({
            "connections": identified.len(),
            "sessions": sessions.len(),
            "bots": bots,
            "shards": identified.iter().map(|conn| json!({
                "bot_id": conn.bot_id,
                "session_id": conn.session_id,
                "shard": [conn.shard_id, conn.shard_count],
                "intents": conn.intents,
                "last_seq": conn.last_seq,
            })).collect::<Vec<_>>(),
        })
    }

    /// Dispatch one event frame to every interested connection of `bot_id`.
    pub async fn dispatch(
        &self,
        bot_id: &str,
        frame: &Value,
        conversation_id: Option<&str>,
    ) -> usize {
        let event_type = frame.get("t").and_then(|v| v.as_str()).unwrap_or("");
        let required = intent_for(event_type);
        let targets: Vec<(u64, mpsc::UnboundedSender<Message>, String)> = {
            let inner = self.inner.lock().unwrap();
            inner
                .conns
                .values()
                .filter(|conn| conn.identified && conn.bot_id == bot_id)
                .filter(|conn| {
                    let intents = if conn.intents == 0 { 1 } else { conn.intents };
                    required.map(|bit| intents & bit != 0).unwrap_or(true)
                })
                .filter(|conn| shard_owns(conn.shard_id, conn.shard_count, conversation_id))
                .map(|conn| (conn.id, conn.tx.clone(), conn.session_id.clone()))
                .collect()
        };
        let text = frame.to_string();
        let mut delivered = 0;
        let mut updated: Vec<(u64, i64, String)> = Vec::new();
        for (id, tx, session_id) in targets {
            if tx.send(Message::Text(text.clone().into())).is_ok() {
                delivered += 1;
                updated.push((
                    id,
                    frame.get("s").and_then(|v| v.as_i64()).unwrap_or(0),
                    session_id,
                ));
            }
        }
        let mut inner = self.inner.lock().unwrap();
        let mut updates: Vec<(i64, String)> = Vec::new();
        for (id, seq, session_id) in updated {
            if let Some(conn) = inner.conns.get_mut(&id) {
                conn.last_seq = seq.max(conn.last_seq);
                updates.push((conn.last_seq, session_id));
            }
        }
        drop(inner);
        if let Some(world) = self.world() {
            for (seq, session_id) in updates {
                let _ = world.db.execute(
                    "UPDATE gateway_sessions SET last_seq=? WHERE session_id=?",
                    &crate::p![seq, &session_id],
                );
            }
        }
        delivered
    }

    pub async fn handle(&self, socket: WebSocket) {
        let Some(world) = self.world() else {
            return;
        };
        let (mut sink, mut stream) = socket.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.conns.insert(
                id,
                Conn {
                    id,
                    bot_id: String::new(),
                    session_id: String::new(),
                    intents: 0,
                    shard_id: 0,
                    shard_count: 1,
                    last_seq: 0,
                    last_seen: Instant::now(),
                    identified: false,
                    tx: tx.clone(),
                },
            );
        }
        let writer = tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                let is_close = matches!(message, Message::Close(_));
                if sink.send(message).await.is_err() {
                    break;
                }
                if is_close {
                    break;
                }
            }
        });

        let interval = world.setting_i64("heartbeat_interval_ms").max(100) as u64;
        let timeout = Duration::from_millis(interval * 2);
        send(
            &tx,
            json!({"op": OP_HELLO, "d": {"heartbeat_interval": interval}}),
        );

        loop {
            match tokio::time::timeout(timeout, stream.next()).await {
                Err(_) => {
                    self.close_conn(id, CLOSE_SESSION_TIMEOUT, "heartbeat timeout")
                        .await;
                    if let Some(world) = self.world() {
                        world.log_frame(
                            "internal",
                            "ws",
                            "heartbeat timeout",
                            &json!({}),
                            self.bot_of(id).as_deref(),
                            None,
                        );
                    }
                    break;
                }
                Ok(None) => break,
                Ok(Some(Err(_))) => break,
                Ok(Some(Ok(Message::Close(_)))) => break,
                Ok(Some(Ok(Message::Text(text)))) => {
                    self.touch(id);
                    match serde_json::from_str::<Value>(&text) {
                        Ok(payload) => {
                            if !self.on_frame(&world, id, payload).await {
                                break;
                            }
                        }
                        Err(_) => {
                            self.close_conn(id, 4002, "invalid json").await;
                            break;
                        }
                    }
                }
                Ok(Some(Ok(Message::Binary(_)))) => continue,
                Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => continue,
            }
        }

        // remove the connection, then let the writer flush the close frame
        let (bot_id, session_id) = {
            let mut inner = self.inner.lock().unwrap();
            inner
                .conns
                .remove(&id)
                .map(|conn| (conn.bot_id, conn.session_id))
                .unwrap_or_default()
        };
        drop(tx);
        let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
        if !session_id.is_empty() {
            let _ = world.db.execute(
                "UPDATE gateway_sessions SET closed_at=? WHERE session_id=?",
                &crate::p![world.clock.now_ms(), &session_id],
            );
        }
        world.log_frame(
            "internal",
            "ws",
            &format!(
                "gateway disconnected (bot={}, session={})",
                if bot_id.is_empty() { "-" } else { &bot_id },
                if session_id.is_empty() {
                    "-"
                } else {
                    &session_id
                }
            ),
            &json!({}),
            if bot_id.is_empty() {
                None
            } else {
                Some(&bot_id)
            },
            if session_id.is_empty() {
                None
            } else {
                Some(&session_id)
            },
        );
    }

    fn touch(&self, id: u64) {
        if let Some(conn) = self.inner.lock().unwrap().conns.get_mut(&id) {
            conn.last_seen = Instant::now();
        }
    }

    fn bot_of(&self, id: u64) -> Option<String> {
        self.inner
            .lock()
            .unwrap()
            .conns
            .get(&id)
            .map(|conn| conn.bot_id.clone())
            .filter(|bot| !bot.is_empty())
    }

    async fn close_conn(&self, id: u64, code: u16, reason: &str) {
        let tx = self
            .inner
            .lock()
            .unwrap()
            .conns
            .get(&id)
            .map(|conn| conn.tx.clone());
        if let Some(tx) = tx {
            let _ = tx.send(Message::Close(Some(CloseFrame {
                code,
                reason: reason.to_string().into(),
            })));
        }
    }

    async fn on_frame(&self, world: &std::sync::Arc<World>, id: u64, payload: Value) -> bool {
        let Some(object) = payload.as_object() else {
            self.close_conn(id, 4002, "frame must be an object").await;
            return false;
        };
        let op = object.get("op").and_then(|v| v.as_i64()).unwrap_or(-1);
        let (bot_id, session_id) = self.conn_ids(id);
        world.log_frame(
            "in",
            "ws",
            format!(
                "<- op={op} {}",
                object.get("t").and_then(|v| v.as_str()).unwrap_or("")
            )
            .trim(),
            &payload,
            if bot_id.is_empty() {
                None
            } else {
                Some(&bot_id)
            },
            if session_id.is_empty() {
                None
            } else {
                Some(&session_id)
            },
        );
        match op {
            OP_HEARTBEAT => {
                let seq = object.get("d").and_then(|v| v.as_i64());
                if let Some(seq) = seq {
                    let mut inner = self.inner.lock().unwrap();
                    if let Some(conn) = inner.conns.get_mut(&id) {
                        if seq > conn.last_seq {
                            conn.last_seq = seq;
                        }
                    }
                    drop(inner);
                    if !session_id.is_empty() {
                        let _ = world.db.execute(
                            "UPDATE gateway_sessions SET last_seq=? WHERE session_id=?",
                            &crate::p![seq, &session_id],
                        );
                    }
                }
                self.send_to(id, json!({"op": OP_HEARTBEAT_ACK, "d": null}));
                true
            }
            OP_IDENTIFY => {
                let data = object.get("d").cloned().unwrap_or(json!({}));
                self.identify(world, id, &data).await
            }
            OP_RESUME => {
                let data = object.get("d").cloned().unwrap_or(json!({}));
                self.resume(world, id, &data).await
            }
            OP_HTTP_CALLBACK_ACK => true,
            _ => true,
        }
    }

    fn conn_ids(&self, id: u64) -> (String, String) {
        self.inner
            .lock()
            .unwrap()
            .conns
            .get(&id)
            .map(|conn| (conn.bot_id.clone(), conn.session_id.clone()))
            .unwrap_or_default()
    }

    fn send_to(&self, id: u64, frame: Value) {
        let tx = self
            .inner
            .lock()
            .unwrap()
            .conns
            .get(&id)
            .map(|conn| conn.tx.clone());
        if let Some(tx) = tx {
            let _ = tx.send(Message::Text(frame.to_string().into()));
        }
    }

    async fn identify(&self, world: &std::sync::Arc<World>, id: u64, data: &Value) -> bool {
        let token = data.get("token").and_then(|v| v.as_str()).unwrap_or("");
        let bot = match world.authenticate_bot(token) {
            Ok(bot) => bot,
            Err(err) => {
                world.log_frame(
                    "internal",
                    "ws",
                    &format!("identify rejected: {err}"),
                    &json!({"error": err.message}),
                    None,
                    None,
                );
                self.close_conn(id, CLOSE_AUTH_FAILED, "invalid token")
                    .await;
                return false;
            }
        };
        let shard = data
            .get("shard")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_i64())
                    .collect::<Vec<i64>>()
            })
            .filter(|items| items.len() >= 2)
            .unwrap_or_else(|| vec![0, 1]);
        let (shard_id, shard_count) = (shard[0], shard[1]);
        if shard_count < 1 || shard_id < 0 || shard_id >= shard_count {
            self.close_conn(id, CLOSE_INVALID_SHARD, "invalid shard")
                .await;
            return false;
        }
        let mut intents = data.get("intents").and_then(|v| v.as_i64()).unwrap_or(0);
        if intents == 0 {
            intents = 1; // botpy behaves this way
        }
        let session_id = new_session_id();
        let bot_id = bot["id"].as_str().unwrap_or("").to_string();
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(conn) = inner.conns.get_mut(&id) {
                conn.bot_id = bot_id.clone();
                conn.intents = intents;
                conn.shard_id = shard_id;
                conn.shard_count = shard_count;
                conn.session_id = session_id.clone();
                conn.identified = true;
            }
        }
        let _ = world.db.execute(
            "INSERT INTO gateway_sessions(session_id,bot_id,intents,last_seq,shard_id,shard_count,created_at) \
             VALUES(?,?,?,?,?,?,?)",
            &crate::p![
                &session_id,
                &bot_id,
                intents,
                0,
                shard_id,
                shard_count,
                world.clock.now_ms()
            ],
        );
        let frame = json!({
            "op": OP_DISPATCH,
            "s": 0,
            "t": "READY",
            "d": {
                "version": 1,
                "session_id": session_id,
                "shard": [shard_id, shard_count],
                "user": {"id": bot["app_id"], "username": bot["name"], "bot": true},
            },
        });
        self.send_to(id, frame.clone());
        world.log_frame(
            "out",
            "ws",
            &format!("READY bot={bot_id} shard={shard_id}/{shard_count}"),
            &frame,
            Some(&bot_id),
            Some(&session_id),
        );
        world.hub.publish(&json!({
            "kind": "gateway",
            "event": "ready",
            "bot_id": bot_id,
            "session_id": session_id,
        }));
        true
    }

    async fn resume(&self, world: &std::sync::Arc<World>, id: u64, data: &Value) -> bool {
        let token = data.get("token").and_then(|v| v.as_str()).unwrap_or("");
        let session_id = data
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let last_seq = data.get("seq").and_then(|v| v.as_i64()).unwrap_or(0);
        let bot = match world.authenticate_bot(token) {
            Ok(bot) => bot,
            Err(_) => {
                self.close_conn(id, CLOSE_AUTH_FAILED, "invalid token")
                    .await;
                return false;
            }
        };
        let bot_id = bot["id"].as_str().unwrap_or("").to_string();
        let row: Option<Row> = world
            .db
            .one(
                "SELECT * FROM gateway_sessions WHERE session_id=?",
                &crate::p![&session_id],
            )
            .unwrap_or(None);
        let mut valid = row
            .as_ref()
            .map(|r| s(r, "bot_id") == bot_id)
            .unwrap_or(false);
        if valid {
            let closed_at = row.as_ref().map(|r| i(r, "closed_at")).unwrap_or(0);
            if closed_at > 0 && world.clock.now_ms() - closed_at > 5 * 60 * 1000 {
                valid = false;
            }
        }
        if !valid {
            self.send_to(id, json!({"op": OP_INVALID_SESSION, "d": false}));
            world.log_frame(
                "out",
                "ws",
                &format!(
                    "INVALID_SESSION session={}",
                    &session_id[..session_id.len().min(12)]
                ),
                &json!({"op": OP_INVALID_SESSION}),
                Some(&bot_id),
                None,
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
            self.close_conn(id, CLOSE_INVALID_SESSION, "invalid session")
                .await;
            return false;
        }
        let row = row.unwrap();
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(conn) = inner.conns.get_mut(&id) {
                conn.bot_id = bot_id.clone();
                conn.session_id = session_id.clone();
                conn.intents = i(&row, "intents");
                conn.shard_id = i(&row, "shard_id");
                conn.shard_count = i(&row, "shard_count");
                conn.last_seq = last_seq.max(i(&row, "last_seq"));
                conn.identified = true;
            }
        }
        let conn_last = self
            .inner
            .lock()
            .unwrap()
            .conns
            .get(&id)
            .map(|conn| conn.last_seq)
            .unwrap_or(0);
        let _ = world.db.execute(
            "UPDATE gateway_sessions SET closed_at=0 WHERE session_id=?",
            &crate::p![&session_id],
        );
        self.send_to(
            id,
            json!({"op": OP_DISPATCH, "s": conn_last, "t": "RESUMED", "d": ""}),
        );
        world.log_frame(
            "out",
            "ws",
            &format!("RESUMED bot={bot_id} seq={conn_last}"),
            &json!({"session_id": session_id}),
            Some(&bot_id),
            Some(&session_id),
        );
        let events = world
            .db
            .all(
                "SELECT seq,event_id,type,payload_json FROM events WHERE target_bot_id=? AND seq>? ORDER BY seq LIMIT 1000",
                &crate::p![&bot_id, conn_last],
            )
            .unwrap_or_default();
        let mut last = conn_last;
        for event in &events {
            let seq = i(event, "seq");
            let frame = json!({
                "op": OP_DISPATCH,
                "s": seq,
                "t": s(event, "type"),
                "id": s(event, "event_id"),
                "d": serde_json::from_str::<Value>(&s(event, "payload_json")).unwrap_or(json!({})),
            });
            self.send_to(id, frame);
            last = seq;
        }
        if !events.is_empty() {
            let _ = world.db.execute(
                "UPDATE gateway_sessions SET last_seq=? WHERE session_id=?",
                &crate::p![last, &session_id],
            );
            world.log_frame(
                "out",
                "ws",
                &format!("resume replay {} events", events.len()),
                &json!({"count": events.len()}),
                Some(&bot_id),
                Some(&session_id),
            );
            if let Some(conn) = self.inner.lock().unwrap().conns.get_mut(&id) {
                conn.last_seq = last;
            }
        }
        world.hub.publish(&json!({
            "kind": "gateway",
            "event": "resumed",
            "bot_id": bot_id,
            "session_id": session_id,
        }));
        true
    }

    /// Watchdog: close connections that stopped sending heartbeats.
    pub fn spawn_watchdog(self: &std::sync::Arc<Self>) {
        let manager = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(world) = manager.world() else { return };
                let interval = world.setting_i64("heartbeat_interval_ms").max(100) / 1000;
                let now = Instant::now();
                let stale: Vec<u64> = {
                    let inner = manager.inner.lock().unwrap();
                    inner
                        .conns
                        .values()
                        .filter(|conn| {
                            now.duration_since(conn.last_seen).as_secs_f64()
                                > (interval as f64 * 2.0 + 10.0)
                        })
                        .map(|conn| conn.id)
                        .collect()
                };
                for id in stale {
                    manager
                        .close_conn(id, CLOSE_SESSION_TIMEOUT, "heartbeat timeout")
                        .await;
                }
            }
        });
    }

    pub async fn close_all(&self) {
        let ids: Vec<u64> = {
            let inner = self.inner.lock().unwrap();
            inner.conns.keys().copied().collect()
        };
        for id in ids {
            self.close_conn(id, 1001, "server shutdown").await;
        }
    }
}

fn shard_owns(shard_id: i64, shard_count: i64, conversation_id: Option<&str>) -> bool {
    if shard_count <= 1 {
        return true;
    }
    match conversation_id {
        Some(conv) if !conv.is_empty() => {
            (crc32fast::hash(conv.as_bytes()) as i64) % shard_count == shard_id
        }
        _ => true,
    }
}

fn send(tx: &mpsc::UnboundedSender<Message>, frame: Value) {
    let _ = tx.send(Message::Text(frame.to_string().into()));
}
