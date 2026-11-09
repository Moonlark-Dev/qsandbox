//! Message semantics: delivery, openid scoping, passive window, dedupe, recall.

use serde_json::{json, Map, Value};

use crate::db::{b, conn_exec, conn_insert, conn_one, i, json_col, s};
use crate::error::{
    bot_offline, duplicate_reply, internal, invalid_param, message_not_found, no_permission,
    passive_window_expired, quota_exhausted, MqsResult,
};
use crate::ids::{new_event_id, new_msg_id, openid_for, union_openid_for};
use crate::p;
use crate::world::{World, BOT_SCOPE_C2C, BOT_SCOPE_GROUP, BOT_SCOPE_MEDIA};

struct Delivery {
    bot_id: String,
    seq: i64,
    payload: Value,
    bot: Value,
    event_type: String,
}

impl World {
    // ------------------------------------------------------------------ user -> bot
    #[allow(clippy::too_many_arguments)]
    pub async fn send_as_user(
        &self,
        conversation_id: &str,
        user_id: &str,
        content: &str,
        msg_type: i64,
        mentions: &[String],
        at_bots: Option<&[String]>,
        implicit_bots: Option<&[String]>,
        reply_to: Option<&str>,
        attachments: &[Value],
    ) -> MqsResult<Value> {
        self.ensure_conversation(conversation_id)?;
        let conv = self
            .db
            .one(
                "SELECT * FROM conversations WHERE id=?",
                &p![conversation_id],
            )?
            .ok_or_else(|| invalid_param(format!("conversation not found: {conversation_id}")))?;
        let user = self
            .db
            .one("SELECT * FROM users WHERE id=?", &p![user_id])?
            .ok_or_else(|| invalid_param(format!("user not found: {user_id}")))?;
        let mut msg_type = msg_type;
        if !attachments.is_empty() && msg_type == 0 {
            msg_type = 7;
        }
        let conv_type = s(&conv, "type");
        let peer_id = s(&conv, "peer_id");

        let mut mentioned_users: Vec<(String, String)> = Vec::new();
        let mut mentioned_bots: Vec<(String, String, String)> = Vec::new();
        let (bot_rows, target_bot_ids, member_role) = if conv_type == "group" {
            let member = self
                .db
                .one(
                    "SELECT * FROM group_members WHERE group_id=? AND user_id=?",
                    &p![&peer_id, user_id],
                )?
                .ok_or_else(|| no_permission("user is not a member of this group"))?;
            if i(&member, "mute_until") > self.clock.now_ms() {
                return Err(no_permission("user is muted in this group"));
            }
            let bot_rows = self.db.all(
                "SELECT b.*, gb.recv_msg_setting AS recv_msg_setting FROM group_bots gb \
                 JOIN bots b ON b.id=gb.bot_id WHERE gb.group_id=?",
                &p![&peer_id],
            )?;
            let mut mention_ids: Vec<String> = Vec::new();
            for id in mentions.iter().chain(at_bots.into_iter().flatten()) {
                if !mention_ids.contains(id) {
                    mention_ids.push(id.clone());
                }
            }
            let present = |bot_id: &str| bot_rows.iter().any(|row| s(row, "id") == bot_id);
            let target_bot_ids: Vec<String> = if let Some(implicit) = implicit_bots {
                implicit.iter().filter(|id| present(id)).cloned().collect()
            } else if let Some(at) = at_bots {
                at.iter().filter(|id| present(id)).cloned().collect()
            } else if !mentions.is_empty() {
                mention_ids
                    .iter()
                    .filter(|id| present(id))
                    .cloned()
                    .collect()
            } else if self.setting_bool("chat.auto_at_bot") {
                bot_rows.iter().map(|row| s(row, "id")).collect()
            } else {
                Vec::new()
            };
            for mid in &mention_ids {
                if let Some(bot_row) = bot_rows.iter().find(|row| &s(row, "id") == mid) {
                    mentioned_bots.push((mid.clone(), s(bot_row, "app_id"), s(bot_row, "name")));
                    continue;
                }
                if let Some(row) = self.db.one(
                    "SELECT u.nickname FROM group_members gm JOIN users u ON u.id=gm.user_id \
                     WHERE gm.group_id=? AND gm.user_id=?",
                    &p![&peer_id, mid],
                )? {
                    mentioned_users.push((mid.clone(), s(&row, "nickname")));
                }
            }
            (bot_rows, target_bot_ids, s(&member, "role"))
        } else {
            if peer_id != user_id {
                return Err(no_permission("this DM belongs to another user"));
            }
            let bot_rows = self
                .db
                .all("SELECT * FROM bots WHERE id=?", &p![s(&conv, "bot_id")])?;
            let target_bot_ids: Vec<String> = bot_rows.iter().map(|row| s(row, "id")).collect();
            (bot_rows, target_bot_ids, String::new())
        };

        if bot_rows.is_empty() {
            return Err(invalid_param("no bot is present in this conversation"));
        }

        let reply_ref = if let Some(reply) = reply_to {
            if self
                .db
                .one(
                    "SELECT * FROM messages WHERE msg_id=? AND conversation_id=?",
                    &p![reply, conversation_id],
                )?
                .is_none()
            {
                return Err(message_not_found(
                    "reply_to message not found in this conversation",
                ));
            }
            Some(reply.to_string())
        } else {
            None
        };

        let event_id = new_event_id();
        let (msg_id, deliveries, raw_message, member_role_out) = self.db.tx(|conn| {
            conn_exec(
                conn,
                "UPDATE conversations SET last_seq=last_seq+1 WHERE id=?",
                &p![conversation_id],
            )?;
            let seq = conn_one(conn, "SELECT last_seq FROM conversations WHERE id=?", &p![conversation_id])?
                .map(|row| i(&row, "last_seq"))
                .unwrap_or(0);
            let msg_id = new_msg_id("MQS_MSG");
            let sent_at = self.clock.now_ms();
            let mut raw_message = json!({
                "id": msg_id,
                "content": content,
                "group_id": if conv_type == "group" { Value::String(peer_id.clone()) } else { Value::Null },
                "timestamp": self.clock.iso(Some(sent_at)),
                "msg_seq": seq,
                "attachments": attachments,
            });
            let display_at_bot_ids: Vec<String> = if conv_type == "group" {
                match at_bots {
                    Some(at) => bot_rows
                        .iter()
                        .map(|row| s(row, "id"))
                        .filter(|id| at.contains(id))
                        .collect(),
                    None => Vec::new(),
                }
            } else {
                Vec::new()
            };
            if !display_at_bot_ids.is_empty() {
                raw_message["at_bots"] = json!(display_at_bot_ids);
            }
            conn_exec(
                conn,
                "INSERT INTO messages(conversation_id,seq,msg_id,event_id,direction,sender_id,sender_type,bot_id,\
                 msg_type,content,raw_json,sent_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                &p![
                    conversation_id,
                    seq,
                    &msg_id,
                    &event_id,
                    "in",
                    user_id,
                    "user",
                    Value::Null,
                    msg_type,
                    content,
                    raw_message.to_string(),
                    sent_at
                ],
            )?;
            let mut deliveries: Vec<Delivery> = Vec::new();
            for bot in &bot_rows {
                let bot_id = s(bot, "id");
                let mut mentioned;
                let bot_event_type;
                if conv_type == "group" {
                    mentioned = target_bot_ids.contains(&bot_id);
                    match s(bot, "recv_msg_setting").as_str() {
                        "all" | "" => bot_event_type = "GROUP_MESSAGE_CREATE".to_string(),
                        _ if mentioned => bot_event_type = "GROUP_AT_MESSAGE_CREATE".to_string(),
                        _ => continue,
                    }
                } else {
                    if !target_bot_ids.contains(&bot_id) {
                        continue;
                    }
                    mentioned = false;
                    bot_event_type = "C2C_MESSAGE_CREATE".to_string();
                }
                let _ = &mut mentioned;
                let group_openid = if conv_type == "group" {
                    Some(Self::openid_tx(conn, bot, "group", &peer_id)?)
                } else {
                    None
                };
                let user_openid = Self::openid_tx(conn, bot, "user", user_id)?;
                let union = union_openid_for(user_id);
                let mut author = json!({
                    "id": user_openid,
                    "member_openid": if conv_type == "group" { Value::String(user_openid.clone()) } else { Value::Null },
                    "user_openid": if conv_type == "dm" { Value::String(user_openid.clone()) } else { Value::Null },
                    "union_openid": union,
                    "username": s(&user, "nickname"),
                });
                if conv_type == "group" {
                    author["bot"] = Value::Bool(false);
                    author["member_role"] = Value::String(member_role.clone());
                }
                let mut payload = json!({
                    "id": msg_id,
                    "content": content,
                    "author": author,
                    "timestamp": self.clock.iso(Some(sent_at)),
                    "msg_seq": seq,
                    "mentions": [],
                    "attachments": attachments,
                });
                if conv_type == "group" {
                    payload["group_openid"] = Value::String(group_openid.clone().unwrap_or_default());
                    payload["group_id"] = Value::String(peer_id.clone());
                    let explicit = at_bots.is_none() || at_bots.map(|at| at.contains(&bot_id)).unwrap_or(false);
                    if bot_event_type == "GROUP_AT_MESSAGE_CREATE" && explicit {
                        let bot_openid = openid_for(&s(bot, "app_id"), "bot", &bot_id);
                        payload["content"] = Value::String(if content.is_empty() {
                            format!("<@!{bot_openid}>")
                        } else {
                            format!("<@!{bot_openid}> {content}")
                        });
                    }
                } else {
                    payload["author"]["user_openid"] = Value::String(user_openid.clone());
                }
                let mut mention_values: Vec<Value> = Vec::new();
                for (mid, nickname) in &mentioned_users {
                    let mrow = Self::openid_tx(conn, bot, "user", mid)?;
                    mention_values.push(json!({
                        "scope": "single",
                        "bot": false,
                        "id": mrow,
                        "is_you": false,
                        "member_openid": mrow,
                        "username": nickname,
                    }));
                }
                for (mid, app_id, name) in &mentioned_bots {
                    let mrow = openid_for(app_id, "bot", mid);
                    mention_values.push(json!({
                        "scope": "single",
                        "bot": true,
                        "id": mrow,
                        "is_you": mid == &bot_id,
                        "member_openid": mrow,
                        "username": name,
                    }));
                }
                payload["mentions"] = Value::Array(mention_values);
                if let Some(reply) = &reply_ref {
                    payload["message_reference"] = json!({"message_id": reply});
                }
                let event_seq = conn_insert(
                    conn,
                    "INSERT INTO events(event_id,type,payload_json,conversation_id,target_bot_id,created_at) \
                     VALUES(?,?,?,?,?,?)",
                    &p![
                        &event_id,
                        &bot_event_type,
                        payload.to_string(),
                        conversation_id,
                        &bot_id,
                        sent_at
                    ],
                )?;
                deliveries.push(Delivery {
                    bot_id,
                    seq: event_seq,
                    payload,
                    bot: Value::Object(bot.clone()),
                    event_type: bot_event_type,
                });
            }
            Ok((msg_id, deliveries, raw_message, member_role))
        })?;

        let message_row = self
            .db
            .one("SELECT * FROM messages WHERE msg_id=?", &p![&msg_id])?
            .ok_or_else(|| internal("message row missing after insert"))?;
        let out = self.message_out(&message_row)?;
        for delivery in &deliveries {
            self.deliver(
                &delivery.bot_id,
                delivery.seq,
                &delivery.event_type,
                &event_id,
                &delivery.payload,
                &delivery.bot,
                Some(conversation_id),
            )
            .await;
        }
        let rule_event = if conv_type == "group" {
            if target_bot_ids.is_empty() {
                "GROUP_MESSAGE_CREATE"
            } else {
                "GROUP_AT_MESSAGE_CREATE"
            }
        } else {
            "C2C_MESSAGE_CREATE"
        };
        let rule_payload = deliveries
            .first()
            .map(|d| d.payload.clone())
            .unwrap_or_else(|| json!({}));
        self.hub.publish(&json!({
            "kind": "message",
            "conversation_id": conversation_id,
            "message": out,
        }));
        let _ = member_role_out;
        self.maybe_run_rules(rule_event, &rule_payload, conversation_id)?;
        let _ = raw_message;
        Ok(json!({
            "message": out,
            "event_ids": [event_id],
            "delivered_to": deliveries.iter().map(|d| Value::String(d.bot_id.clone())).collect::<Vec<_>>(),
        }))
    }

    // ------------------------------------------------------------------ delivery
    #[allow(clippy::too_many_arguments)]
    pub async fn deliver(
        &self,
        bot_id: &str,
        seq: i64,
        event_type: &str,
        event_id: &str,
        payload: &Value,
        bot: &Value,
        conversation_id: Option<&str>,
    ) {
        let frame = json!({
            "op": 0,
            "s": seq,
            "t": event_type,
            "id": event_id,
            "d": payload,
        });
        if let Some(gateway) = self.gateway.get() {
            gateway.dispatch(bot_id, &frame, conversation_id).await;
        }
        let delivery = bot.get("delivery").and_then(|v| v.as_str()).unwrap_or("ws");
        if delivery == "webhook" || !crate::webhooks::endpoints(self, bot_id).is_empty() {
            crate::webhooks::deliver(&self.arc(), bot_id, &frame);
        }
        self.hub.publish(&json!({
            "kind": "event",
            "event": {
                "seq": seq,
                "event_id": event_id,
                "type": event_type,
                "bot_id": bot_id,
                "conversation_id": conversation_id,
                "payload": payload,
                "created_at": self.clock.now_ms(),
            }
        }));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_deliver(
        &self,
        bot_id: &str,
        seq: i64,
        event_type: &str,
        event_id: &str,
        payload: &Value,
        bot: &Value,
        conversation_id: Option<&str>,
    ) {
        let world = self.arc();
        let bot_id = bot_id.to_string();
        let event_type = event_type.to_string();
        let event_id = event_id.to_string();
        let payload = payload.clone();
        let bot = bot.clone();
        let conversation_id = conversation_id.map(str::to_string);
        tokio::spawn(async move {
            world
                .deliver(
                    &bot_id,
                    seq,
                    &event_type,
                    &event_id,
                    &payload,
                    &bot,
                    conversation_id.as_deref(),
                )
                .await;
        });
    }

    // ------------------------------------------------------------------ bot -> user
    pub async fn send_as_bot(
        &self,
        bot_id: &str,
        target_kind: &str,
        target_openid: &str,
        body: &Value,
    ) -> MqsResult<Value> {
        validate_send_body(body)?;
        let bot = self.bot_by_id(bot_id)?;
        let scopes = json_col(&bot, "scopes", json!([]));
        let has_scope = |scope: &str| {
            scopes
                .as_array()
                .map(|items| items.iter().any(|v| v.as_str() == Some(scope)))
                .unwrap_or(false)
        };
        if target_kind == "group" && !has_scope(BOT_SCOPE_GROUP) {
            return Err(no_permission("bot lacks the group_message scope"));
        }
        if target_kind == "user" && !has_scope(BOT_SCOPE_C2C) {
            return Err(no_permission("bot lacks the c2c_message scope"));
        }
        if body.get("media").map(|v| !v.is_null()).unwrap_or(false) && !has_scope(BOT_SCOPE_MEDIA) {
            return Err(no_permission("bot lacks the media scope"));
        }
        let mapping = self.resolve_openid(bot_id, target_openid)?;
        if s(&mapping, "kind") != target_kind {
            return Err(invalid_param("openid kind mismatch"));
        }
        let entity_id = s(&mapping, "entity_id");
        let conversation_id = if target_kind == "group" {
            if self
                .db
                .one(
                    "SELECT 1 FROM group_bots WHERE group_id=? AND bot_id=?",
                    &p![&entity_id, bot_id],
                )?
                .is_none()
            {
                return Err(no_permission("bot is not a member of this group"));
            }
            format!("group:{entity_id}")
        } else {
            format!("dm:{bot_id}:{entity_id}")
        };
        self.ensure_conversation(&conversation_id)?;

        if self.setting_bool("require_online")
            && s(&bot, "delivery") == "ws"
            && self
                .gateway
                .get()
                .map(|gateway| !gateway.is_online(bot_id))
                .unwrap_or(true)
        {
            return Err(bot_offline(
                "connect the bot to the gateway before sending messages",
            ));
        }

        let now = self.clock.now_ms();
        let mut rate_result: Option<Value> = None;
        let msg_id_in = body.get("msg_id").and_then(|v| v.as_str()).unwrap_or("");
        let event_id_in = body.get("event_id").and_then(|v| v.as_str()).unwrap_or("");
        // Official passive replies accept either msg_id (a message) or event_id
        // (INTERACTION_CREATE / GROUP_ADD_ROBOT / GROUP_MSG_RECEIVE / C2C_MSG_RECEIVE /
        // FRIEND_ADD). Both are "reply to something the bot already received".
        let passive_key: Option<String>;
        let passive_sent_at: i64;
        if !msg_id_in.is_empty() {
            let reference = self
                .db
                .one(
                    "SELECT * FROM messages WHERE msg_id=? AND conversation_id=?",
                    &p![msg_id_in, &conversation_id],
                )?
                .ok_or_else(|| {
                    message_not_found(
                        "msg_id does not refer to an inbound message of this conversation",
                    )
                })?;
            if s(&reference, "direction") != "in" {
                return Err(message_not_found(
                    "msg_id does not refer to an inbound message of this conversation",
                ));
            }
            passive_key = Some(msg_id_in.to_string());
            passive_sent_at = i(&reference, "sent_at");
        } else if !event_id_in.is_empty() {
            let event = self
                .db
                .one(
                    "SELECT * FROM events WHERE event_id=? AND target_bot_id=?",
                    &p![event_id_in, bot_id],
                )?
                .ok_or_else(|| {
                    message_not_found("event_id does not refer to an event delivered to this bot")
                })?;
            passive_key = Some(event_id_in.to_string());
            passive_sent_at = i(&event, "created_at");
        } else {
            passive_key = None;
            passive_sent_at = 0;
        }
        let msg_seq: i64;
        if let Some(passive_key) = &passive_key {
            let passive_key = passive_key.as_str();
            let (window, limit) = if target_kind == "user" {
                (
                    self.setting_i64("passive_window_c2c_ms"),
                    self.setting_i64("passive_reply_limit_c2c"),
                )
            } else {
                (
                    self.setting_i64("passive_window_group_ms"),
                    self.setting_i64("passive_reply_limit_group"),
                )
            };
            if now - passive_sent_at > window {
                return Err(passive_window_expired(format!(
                    "passive reply window ({} minutes) has expired",
                    window / 60000
                )));
            }
            let used = self
                .db
                .one(
                    "SELECT * FROM reply_windows WHERE msg_id=? AND bot_id=?",
                    &p![passive_key, bot_id],
                )?
                .map(|row| i(&row, "count"))
                .unwrap_or(0);
            if used >= limit {
                return Err(passive_window_expired(format!(
                    "passive reply limit ({limit}) exhausted for this msg_id"
                )));
            }
            match body.get("msg_seq").and_then(|v| v.as_i64()) {
                None => {
                    let max = self
                        .db
                        .one(
                            "SELECT COALESCE(MAX(msg_seq),0) AS m FROM reply_keys WHERE bot_id=? AND msg_id=?",
                            &p![bot_id, passive_key],
                        )?
                        .map(|row| i(&row, "m"))
                        .unwrap_or(0);
                    msg_seq = max + 1;
                }
                Some(seq) => {
                    if self
                        .db
                        .one(
                            "SELECT 1 FROM reply_keys WHERE bot_id=? AND msg_id=? AND msg_seq=?",
                            &p![bot_id, passive_key, seq],
                        )?
                        .is_some()
                    {
                        return Err(duplicate_reply(format!(
                            "msg_seq {seq} already used for {passive_key}"
                        )));
                    }
                    msg_seq = seq;
                }
            }
            self.db.execute(
                "INSERT INTO reply_keys(bot_id,msg_id,msg_seq,created_at) VALUES(?,?,?,?)",
                &p![bot_id, passive_key, msg_seq, now],
            )?;
            self.db.execute(
                "INSERT INTO reply_windows(msg_id,bot_id,count,expires_at) VALUES(?,?,?,?) \
                 ON CONFLICT(msg_id,bot_id) DO UPDATE SET count=count+1, expires_at=excluded.expires_at",
                &p![passive_key, bot_id, 1, passive_sent_at + window],
            )?;
        } else {
            if !b(&bot, "allow_proactive_msg") {
                return Err(no_permission(
                    "proactive messages are disabled for this bot",
                ));
            }
            if target_kind == "group" {
                if let Some(group_bot) = self.db.one(
                    "SELECT allow_proactive_msg FROM group_bots WHERE group_id=? AND bot_id=?",
                    &p![&entity_id, bot_id],
                )? {
                    if !b(&group_bot, "allow_proactive_msg") {
                        return Err(no_permission(
                            "proactive messages are disabled for this bot in this group",
                        ));
                    }
                }
            }
            msg_seq = body.get("msg_seq").and_then(|v| v.as_i64()).unwrap_or(1);
            let scene = if target_kind == "user" {
                "c2c"
            } else {
                "group"
            };
            let info = self
                .rate
                .lock()
                .unwrap()
                .check_active(bot_id, scene, &entity_id)?;
            let quota = self.setting_i64("active_quota_per_day");
            let day = self.clock.day();
            let used = self
                .db
                .one(
                    "SELECT used FROM active_quota WHERE bot_id=? AND conversation_id=? AND day=?",
                    &p![bot_id, &conversation_id, &day],
                )?
                .map(|row| i(&row, "used"))
                .unwrap_or(0);
            if used >= quota {
                return Err(quota_exhausted(format!(
                    "active message quota ({quota}/day) exhausted"
                )));
            }
            self.db.execute(
                "INSERT INTO active_quota(bot_id,conversation_id,day,used) VALUES(?,?,?,1) \
                 ON CONFLICT(bot_id,conversation_id,day) DO UPDATE SET used=used+1",
                &p![bot_id, &conversation_id, &day],
            )?;
            rate_result = Some(info.as_dict());
        }

        let msg_type = body.get("msg_type").and_then(|v| v.as_i64()).unwrap_or(0);
        let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let has_payload = ["markdown", "keyboard", "ark", "embed", "media"]
            .iter()
            .any(|key| body.get(*key).map(|v| !v.is_null()).unwrap_or(false));
        if content.is_empty() && !has_payload {
            return Err(invalid_param("message body is empty"));
        }

        let msg_id = new_msg_id("MQS_MSG");
        let sent_at = self.clock.now_ms();
        let mut raw = body.as_object().cloned().unwrap_or_default();
        raw.insert("id".into(), Value::String(msg_id.clone()));
        raw.insert(
            "timestamp".into(),
            Value::String(self.clock.iso(Some(sent_at))),
        );
        raw.insert("msg_seq".into(), Value::from(msg_seq));
        let mut attachments: Vec<Value> = Vec::new();
        if let Some(file_info) = body
            .get("media")
            .and_then(|media| media.get("file_info"))
            .and_then(|v| v.as_str())
        {
            let mhash = file_info.strip_prefix("mqs-file-").unwrap_or(file_info);
            if let Some(media) = self.media_by_hash(mhash)? {
                attachments.push(json!({
                    "content_type": s(&media, "mime"),
                    "filename": &mhash[..mhash.len().min(12)],
                    "size": i(&media, "size"),
                    "url": format!("/media/{mhash}"),
                    "id": mhash,
                }));
            }
        }
        let seq = self.db.tx(|conn| {
            conn_exec(
                conn,
                "UPDATE conversations SET last_seq=last_seq+1 WHERE id=?",
                &p![&conversation_id],
            )?;
            let seq = conn_one(conn, "SELECT last_seq FROM conversations WHERE id=?", &p![&conversation_id])?
                .map(|row| i(&row, "last_seq"))
                .unwrap_or(0);
            conn_exec(
                conn,
                "INSERT INTO messages(conversation_id,seq,msg_id,event_id,direction,sender_id,sender_type,bot_id,\
                 msg_type,content,raw_json,sent_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                &p![
                    &conversation_id,
                    seq,
                    &msg_id,
                    Value::Null,
                    "out",
                    bot_id,
                    "bot",
                    bot_id,
                    msg_type,
                    content,
                    Value::Object(raw.clone()).to_string(),
                    sent_at
                ],
            )?;
            Ok(seq)
        })?;
        let message_row = self
            .db
            .one("SELECT * FROM messages WHERE msg_id=?", &p![&msg_id])?
            .ok_or_else(|| internal("message row missing"))?;
        let mut out = self.message_out(&message_row)?;
        out["attachments"] = Value::Array(attachments);
        self.hub.publish(&json!({
            "kind": "message",
            "conversation_id": conversation_id,
            "message": out,
        }));
        self.log_frame(
            "out",
            "http",
            &format!(
                "POST message -> {target_kind}:{} [{msg_id}]",
                &target_openid[..target_openid.len().min(12)]
            ),
            &json!({
                "request": Value::Object(raw.clone()),
                "response": {"id": msg_id, "timestamp": raw.get("timestamp")},
            }),
            Some(bot_id),
            None,
        );
        let mut result = json!({
            "id": msg_id,
            "timestamp": raw.get("timestamp").cloned().unwrap_or(Value::Null),
            "seq": seq,
            "conversation_id": conversation_id,
        });
        if let Some(info) = rate_result {
            result["_rate"] = info;
        }
        Ok(result)
    }

    // ------------------------------------------------------------------ recall / interactions
    pub fn recall_message(
        &self,
        bot_id: &str,
        target_kind: &str,
        target_openid: &str,
        message_id: &str,
    ) -> MqsResult<()> {
        let mapping = self.resolve_openid(bot_id, target_openid)?;
        if s(&mapping, "kind") != target_kind {
            return Err(invalid_param("openid kind mismatch"));
        }
        let conversation_id = if target_kind == "group" {
            format!("group:{}", s(&mapping, "entity_id"))
        } else {
            format!("dm:{bot_id}:{}", s(&mapping, "entity_id"))
        };
        let row = self
            .db
            .one(
                "SELECT * FROM messages WHERE msg_id=? AND conversation_id=?",
                &p![message_id, &conversation_id],
            )?
            .ok_or_else(|| message_not_found("message not found or not owned by this bot"))?;
        if s(&row, "sender_id") != bot_id {
            return Err(message_not_found(
                "message not found or not owned by this bot",
            ));
        }
        // Official limit: a message cannot be recalled 2 minutes after it was sent.
        if self.clock.now_ms() - i(&row, "sent_at") > 120_000 {
            return Err(message_not_found(
                "message is older than 2 minutes and cannot be recalled",
            ));
        }
        self.db.execute(
            "UPDATE messages SET recalled_at=? WHERE msg_id=?",
            &p![self.clock.now_ms(), message_id],
        )?;
        self.hub.publish(&json!({
            "kind": "recall",
            "conversation_id": conversation_id,
            "message_id": message_id,
            "bot_id": bot_id,
        }));
        Ok(())
    }

    /// Simulate a user clicking one inline-keyboard button.
    pub async fn click_keyboard_button(
        &self,
        user_id: &str,
        message_id: &str,
        index: i64,
    ) -> MqsResult<Value> {
        let row = self
            .db
            .one("SELECT * FROM messages WHERE msg_id=?", &p![message_id])?
            .ok_or_else(|| message_not_found("message is not an outbound bot message"))?;
        if s(&row, "direction") != "out" {
            return Err(message_not_found("message is not an outbound bot message"));
        }
        let raw = json_col(&row, "raw_json", json!({}));
        let buttons: Vec<Value> = raw["keyboard"]["content"]["rows"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .flat_map(|row| row["buttons"].as_array().cloned().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        if index < 0 || index as usize >= buttons.len() {
            return Err(invalid_param("keyboard button index out of range"));
        }
        let button = &buttons[index as usize];
        let data = button["action"]["data"].as_str().unwrap_or("").to_string();
        let kind = button["action"]["type"].as_i64().unwrap_or(1);
        let conversation_id = s(&row, "conversation_id");
        let bot_id = s(&row, "bot_id");
        if self
            .db
            .one("SELECT 1 FROM users WHERE id=?", &p![user_id])?
            .is_none()
        {
            return Err(invalid_param("unknown user"));
        }
        if kind == 0 {
            return Ok(json!({"kind": "link", "data": data, "message": null}));
        }
        if kind == 2 {
            let bot = self.bot_out(&self.bot_by_id(&bot_id)?);
            let interaction = self
                .emit_button_interaction(
                    &bot,
                    &bot_id,
                    user_id,
                    &conversation_id,
                    message_id,
                    button,
                    &data,
                    index,
                )
                .await?;
            return Ok(json!({
                "kind": "callback",
                "data": data,
                "interaction": interaction,
                "message": null,
            }));
        }
        let is_group = conversation_id.starts_with("group:");
        let no_mentions: Vec<String> = Vec::new();
        let implicit = [bot_id.clone()];
        let result = self
            .send_as_user(
                &conversation_id,
                user_id,
                &data,
                0,
                &[],
                if is_group {
                    Some(&no_mentions[..])
                } else {
                    None
                },
                if is_group { Some(&implicit[..]) } else { None },
                None,
                &[],
            )
            .await?;
        let mut out = Map::new();
        out.insert("kind".into(), Value::String("command".into()));
        out.insert("data".into(), Value::String(data));
        if let Some(obj) = result.as_object() {
            for (key, value) in obj {
                out.insert(key.clone(), value.clone());
            }
        }
        Ok(Value::Object(out))
    }

    #[allow(clippy::too_many_arguments)]
    async fn emit_button_interaction(
        &self,
        bot: &Value,
        bot_id: &str,
        user_id: &str,
        conversation_id: &str,
        message_id: &str,
        button: &Value,
        data: &str,
        index: i64,
    ) -> MqsResult<Value> {
        let user_openid = self.openid(bot_id, "user", user_id)?;
        let interaction_id = new_event_id();
        let application_id = match bot["app_id"].as_str() {
            Some(text) if text.chars().all(|c| c.is_ascii_digit()) => {
                text.parse::<i64>().map(Value::from).unwrap_or(Value::Null)
            }
            other => other
                .map(|v| Value::String(v.to_string()))
                .unwrap_or(Value::Null),
        };
        let mut payload = json!({
            "id": interaction_id,
            "application_id": application_id,
            "type": 11,
            "timestamp": self.clock.iso(None),
            "version": 1,
            "data": {
                "type": 11,
                "resolved": {
                    "button_id": button["id"].as_str().map(str::to_string).unwrap_or_else(|| index.to_string()),
                    "button_data": data,
                    "message_id": message_id,
                    "user_id": user_openid,
                },
            },
        });
        if conversation_id.starts_with("group:") {
            let group_id = conversation_id.trim_start_matches("group:");
            payload["scene"] = Value::String("group".into());
            payload["chat_type"] = Value::from(1);
            payload["group_openid"] = Value::String(self.openid(bot_id, "group", group_id)?);
            payload["group_member_openid"] = Value::String(user_openid.clone());
            payload["user_openid"] = Value::String(user_openid);
        } else if conversation_id.starts_with("dm:") {
            payload["scene"] = Value::String("c2c".into());
            payload["chat_type"] = Value::from(2);
            payload["user_openid"] = Value::String(user_openid);
        }
        let seq = self.db.execute(
            "INSERT INTO events(event_id,type,payload_json,conversation_id,target_bot_id,created_at) \
             VALUES(?,?,?,?,?,?)",
            &p![
                &interaction_id,
                "INTERACTION_CREATE",
                payload.to_string(),
                conversation_id,
                bot_id,
                self.clock.now_ms()
            ],
        )? as i64;
        self.deliver(
            bot_id,
            seq,
            "INTERACTION_CREATE",
            &interaction_id,
            &payload,
            bot,
            Some(conversation_id),
        )
        .await;
        Ok(json!({"id": interaction_id, "payload": payload, "seq": seq}))
    }

    pub fn ack_interaction(
        &self,
        bot_id: &str,
        interaction_id: &str,
        code: i64,
    ) -> MqsResult<Value> {
        let row = self.db.one(
            "SELECT seq FROM events WHERE event_id=? AND type='INTERACTION_CREATE' AND target_bot_id=?",
            &p![interaction_id, bot_id],
        )?;
        let row = row.ok_or_else(|| message_not_found("interaction not found"))?;
        Ok(json!({"code": code, "seq": i(&row, "seq")}))
    }
}

/// Pydantic used to enforce these types; the Rust port must reject the same
/// shapes with the same QQ error (10001) instead of coercing or ignoring them.
fn validate_send_body(body: &Value) -> MqsResult<()> {
    let Some(object) = body.as_object() else {
        return Err(invalid_param("invalid parameter: body must be an object"));
    };
    for key in ["content", "msg_id", "event_id"] {
        if let Some(value) = object.get(key) {
            if !value.is_null() && !value.is_string() {
                return Err(invalid_param(format!(
                    "invalid parameter: {key}: must be a string"
                )));
            }
        }
    }
    for key in ["msg_type", "msg_seq", "timestamp"] {
        if let Some(value) = object.get(key) {
            if !value.is_null() && value.as_i64().is_none() {
                return Err(invalid_param(format!(
                    "invalid parameter: {key}: must be an integer"
                )));
            }
        }
    }
    for key in [
        "markdown",
        "keyboard",
        "ark",
        "embed",
        "media",
        "message_reference",
    ] {
        if let Some(value) = object.get(key) {
            if !value.is_null() && !value.is_object() {
                return Err(invalid_param(format!(
                    "invalid parameter: {key}: must be an object"
                )));
            }
        }
    }
    Ok(())
}
