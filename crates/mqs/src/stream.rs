//! 流式发送单聊消息：`POST /v2/users/{user_openid}/stream_messages`。
//!
//! 官方语义（`autogen/api/v2_users_user_openid_stream_messages.post.html`）：
//!
//! - 首片不带 `stream_msg_id`，响应 `id` 就是后续分片要带的 `stream_msg_id`；
//! - 每个分片有从 0 递增的 `index`，同一个流共享同一个 `msg_id` / `event_id`；
//! - `input_mode=append`（默认）把 `content_raw` 追加到未下发内容；
//!   `input_mode=replace` 表示 `content_raw` 是当前全量正文，且必须以上游已下发的
//!   前缀 `SentContent` 开头，否则返回官方错误码 40007；
//! - `input_state=1` 生成中、`10` 生成结束；`content_type` 支持 `text` / `markdown`。
//!
//! 沙箱没有网络延迟与打字机节流：一次请求处理完就把内容视为「已下发」，
//! 所以 `SentContent` 就是当前全文，`replace` 的前缀校验以它为准。
//! 同一 `index` + 相同内容重放视为幂等去重（官方 `msg_seq` 的去重语义），
//! 直接返回上一次的结果，不会重复写入。

use base64::Engine;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::db::{i, json_col, s};
use crate::error::{internal, invalid_param, message_not_found, stream_prefix_locked, MqsResult};
use crate::ids::new_uuid;
use crate::messages::OutboundTarget;
use crate::p;
use crate::world::World;

const INPUT_GENERATING: i64 = 1;
const INPUT_FINISHED: i64 = 10;

impl World {
    /// 处理一个流式分片（首片或续片）。返回官方响应体
    /// `{id, timestamp, ext_info, remain_msg_len}` 加上沙箱额外的 `seq` / `conversation_id`。
    pub fn send_stream_chunk(
        &self,
        bot_id: &str,
        user_openid: &str,
        body: &Value,
    ) -> MqsResult<Value> {
        validate_stream_body(body)?;
        let index = body.get("index").and_then(Value::as_i64).unwrap_or(0);
        let input_state = body
            .get("input_state")
            .and_then(Value::as_i64)
            .unwrap_or(INPUT_GENERATING);
        if input_state != INPUT_GENERATING && input_state != INPUT_FINISHED {
            return Err(invalid_param(
                "input_state must be 1 (generating) or 10 (finished)",
            ));
        }
        if index < 0 {
            return Err(invalid_param("index must be >= 0"));
        }
        let input_mode = body
            .get("input_mode")
            .and_then(Value::as_str)
            .unwrap_or("append");
        if input_mode != "append" && input_mode != "replace" {
            return Err(invalid_param("input_mode must be append or replace"));
        }
        let content_type = body.get("content_type").and_then(Value::as_str);
        if let Some(content_type) = content_type {
            check_content_type(content_type)?;
        }
        let content_raw = body
            .get("content_raw")
            .and_then(Value::as_str)
            .unwrap_or("");
        let target = self.resolve_outbound_target(bot_id, "user", user_openid)?;
        let now = self.clock.now_ms();
        let stream_msg_id = body
            .get("stream_msg_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        match stream_msg_id {
            Some(stream_msg_id) => self.continue_stream(
                bot_id,
                &target,
                stream_msg_id,
                index,
                input_state,
                input_mode,
                content_raw,
                content_type,
                body,
                now,
            ),
            None => self.start_stream(
                bot_id,
                &target,
                index,
                input_state,
                input_mode,
                content_raw,
                content_type,
                body,
                now,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_stream(
        &self,
        bot_id: &str,
        target: &OutboundTarget,
        index: i64,
        input_state: i64,
        input_mode: &str,
        content_raw: &str,
        content_type: Option<&str>,
        body: &Value,
        now: i64,
    ) -> MqsResult<Value> {
        if index != 0 {
            return Err(invalid_param(
                "the first stream chunk must omit stream_msg_id and use index 0",
            ));
        }
        let content_type = content_type.unwrap_or("text");
        // 与普通发送共用被动窗口 / msg_seq 去重 / 主动配额与限流。
        let auth = self.authorize_outbound(
            bot_id,
            &target.bot,
            "user",
            &target.entity_id,
            &target.conversation_id,
            body,
            now,
        )?;
        let stream_msg_id = new_uuid();
        let full = content_raw.to_string();
        let max_len = self.stream_max_len();
        check_stream_len(&full, max_len)?;
        let timestamp = self.clock.iso(Some(now));
        let ref_idx = auth
            .reference_msg_id
            .as_deref()
            .map(ref_idx_for)
            .unwrap_or_default();
        let msg_type = msg_type_for(content_type);
        let mut raw = body.as_object().cloned().unwrap_or_default();
        raw.insert("id".into(), Value::String(stream_msg_id.clone()));
        raw.insert("timestamp".into(), Value::String(timestamp.clone()));
        raw.insert("msg_seq".into(), Value::from(auth.msg_seq));
        raw.insert(
            "stream".into(),
            stream_meta(
                &stream_msg_id,
                index,
                input_state,
                input_mode,
                content_type,
                &full,
                max_len,
            ),
        );
        if content_type == "markdown" {
            raw.insert("markdown".into(), json!({"content": full}));
        }
        let seq = self.insert_outbound_message(
            &target.conversation_id,
            bot_id,
            &stream_msg_id,
            msg_type,
            &full,
            &Value::Object(raw),
            now,
            &[],
            now,
        )?;
        self.db.execute(
            "INSERT INTO stream_messages(stream_msg_id,bot_id,user_id,conversation_id,content_type,\
             sent_content,input_state,last_index,last_content_raw,last_input_mode,last_input_state,\
             ref_idx,finished_at,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            &p![
                &stream_msg_id,
                bot_id,
                &target.entity_id,
                &target.conversation_id,
                content_type,
                &full,
                input_state,
                index,
                content_raw,
                input_mode,
                input_state,
                &ref_idx,
                if input_state == INPUT_FINISHED { now } else { 0 },
                now,
                now
            ],
        )?;
        let message = self.outbound_message(&stream_msg_id)?;
        self.hub.publish(&json!({
            "kind": "message",
            "conversation_id": target.conversation_id,
            "message": message,
        }));
        self.log_stream(
            bot_id,
            &target.entity_id,
            &stream_msg_id,
            index,
            input_state,
            body,
            &message,
        );
        Ok(stream_response(
            &stream_msg_id,
            &timestamp,
            &ref_idx,
            max_len - char_len(&full),
            seq,
            &target.conversation_id,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn continue_stream(
        &self,
        bot_id: &str,
        target: &OutboundTarget,
        stream_msg_id: &str,
        index: i64,
        input_state: i64,
        input_mode: &str,
        content_raw: &str,
        content_type: Option<&str>,
        body: &Value,
        now: i64,
    ) -> MqsResult<Value> {
        let row = self
            .db
            .one(
                "SELECT * FROM stream_messages WHERE stream_msg_id=? AND bot_id=? AND user_id=?",
                &p![stream_msg_id, bot_id, &target.entity_id],
            )?
            .ok_or_else(|| {
                message_not_found("stream_msg_id does not refer to a stream of this bot and user")
            })?;
        let stored_type = s(&row, "content_type");
        if let Some(content_type) = content_type {
            if content_type != stored_type {
                return Err(invalid_param(format!(
                    "content_type of this stream is {stored_type}, not {content_type}"
                )));
            }
        }
        let last_index = i(&row, "last_index");
        let sent = s(&row, "sent_content");
        let last_state = i(&row, "last_input_state");
        let last_mode = s(&row, "last_input_mode");
        let last_raw = s(&row, "last_content_raw");
        // 完全相同的一帧重放：按官方 msg_seq 的「去重」语义直接返回上一次结果。
        if index == last_index
            && content_raw == last_raw
            && input_mode == last_mode
            && input_state == last_state
        {
            let seq = self.message_seq(&target.conversation_id, stream_msg_id)?;
            return Ok(stream_response(
                stream_msg_id,
                &self.clock.iso(Some(i(&row, "updated_at"))),
                &s(&row, "ref_idx"),
                self.stream_max_len() - char_len(&sent),
                seq,
                &target.conversation_id,
            ));
        }
        if index != last_index + 1 {
            return Err(invalid_param(format!(
                "expected index {}, got {index}",
                last_index + 1
            )));
        }
        if i(&row, "finished_at") != 0 {
            return Err(invalid_param("this stream message already finished"));
        }
        let full = if input_mode == "append" {
            format!("{sent}{content_raw}")
        } else if content_raw.starts_with(&sent) {
            content_raw.to_string()
        } else {
            return Err(stream_prefix_locked(
                "input_mode=replace: content_raw must start with the content already delivered",
            ));
        };
        let max_len = self.stream_max_len();
        check_stream_len(&full, max_len)?;
        let finished_at = if input_state == INPUT_FINISHED {
            now
        } else {
            0
        };
        self.db.execute(
            "UPDATE stream_messages SET sent_content=?, input_state=?, last_index=?, \
             last_content_raw=?, last_input_mode=?, last_input_state=?, updated_at=?, finished_at=? \
             WHERE stream_msg_id=?",
            &p![
                &full,
                input_state,
                index,
                content_raw,
                input_mode,
                input_state,
                now,
                finished_at,
                stream_msg_id
            ],
        )?;
        // 消息的 raw 保留首片里的 msg_id / event_id 等字段，只更新流式状态。
        let message_row = self
            .db
            .one(
                "SELECT * FROM messages WHERE msg_id=? AND conversation_id=?",
                &p![stream_msg_id, &target.conversation_id],
            )?
            .ok_or_else(|| internal("stream message row missing"))?;
        let mut raw = json_col(&message_row, "raw_json", json!({}));
        let object = raw
            .as_object_mut()
            .ok_or_else(|| internal("stream message raw is not an object"))?;
        object.insert(
            "stream".into(),
            stream_meta(
                stream_msg_id,
                index,
                input_state,
                input_mode,
                &stored_type,
                &full,
                max_len,
            ),
        );
        if stored_type == "markdown" {
            object.insert("markdown".into(), json!({"content": full}));
        }
        self.update_outbound_message(&target.conversation_id, stream_msg_id, &full, &raw)?;
        let message = self.outbound_message(stream_msg_id)?;
        self.hub.publish(&json!({
            "kind": "message_update",
            "conversation_id": target.conversation_id,
            "message": message,
        }));
        self.log_stream(
            bot_id,
            &target.entity_id,
            stream_msg_id,
            index,
            input_state,
            body,
            &message,
        );
        Ok(stream_response(
            stream_msg_id,
            &self.clock.iso(Some(now)),
            &s(&row, "ref_idx"),
            max_len - char_len(&full),
            i(&message_row, "seq"),
            &target.conversation_id,
        ))
    }

    fn stream_max_len(&self) -> i64 {
        self.setting_i64("stream_msg_max_len").max(1)
    }

    fn message_seq(&self, conversation_id: &str, msg_id: &str) -> MqsResult<i64> {
        self.db
            .one(
                "SELECT seq FROM messages WHERE msg_id=? AND conversation_id=?",
                &p![msg_id, conversation_id],
            )?
            .map(|row| i(&row, "seq"))
            .ok_or_else(|| internal("stream message row missing"))
    }

    fn outbound_message(&self, msg_id: &str) -> MqsResult<Value> {
        let row = self
            .db
            .one("SELECT * FROM messages WHERE msg_id=?", &p![msg_id])?
            .ok_or_else(|| internal("stream message row missing"))?;
        self.message_out(&row)
    }

    #[allow(clippy::too_many_arguments)]
    fn log_stream(
        &self,
        bot_id: &str,
        user_id: &str,
        stream_msg_id: &str,
        index: i64,
        input_state: i64,
        body: &Value,
        message: &Value,
    ) {
        self.log_frame(
            "out",
            "http",
            &format!(
                "POST stream_messages -> user:{} [{stream_msg_id}] #{index} state={input_state}",
                &user_id[..user_id.len().min(12)]
            ),
            &json!({
                "request": body,
                "response": {"id": stream_msg_id, "message": message},
            }),
            Some(bot_id),
            None,
        );
    }
}

fn char_len(text: &str) -> i64 {
    text.chars().count() as i64
}

fn check_content_type(content_type: &str) -> MqsResult<()> {
    if content_type == "text" || content_type == "markdown" {
        Ok(())
    } else {
        Err(invalid_param("content_type must be text or markdown"))
    }
}

fn check_stream_len(content: &str, max_len: i64) -> MqsResult<()> {
    let length = char_len(content);
    if length > max_len {
        return Err(invalid_param(format!(
            "stream content is {length} characters, over the {max_len} character limit"
        )));
    }
    Ok(())
}

fn msg_type_for(content_type: &str) -> i64 {
    if content_type == "markdown" {
        2
    } else {
        0
    }
}

fn stream_meta(
    stream_msg_id: &str,
    index: i64,
    input_state: i64,
    input_mode: &str,
    content_type: &str,
    content: &str,
    max_len: i64,
) -> Value {
    json!({
        "stream_msg_id": stream_msg_id,
        "index": index,
        "input_state": input_state,
        "input_mode": input_mode,
        "content_type": content_type,
        "finished": input_state == INPUT_FINISHED,
        "remain_msg_len": (max_len - char_len(content)).max(0),
    })
}

/// 沙箱的 `REFIDX_*`：官方是 `message_scene.ext` 里的引用索引，沙箱没有建模，
/// 用被引用消息的 `msg_id` 派生一个稳定假值（形状与官方示例一致）。
fn ref_idx_for(msg_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mqs:ref:");
    hasher.update(msg_id.as_bytes());
    let digest = hasher.finalize();
    format!(
        "REFIDX_{}",
        base64::engine::general_purpose::STANDARD.encode(&digest[..16])
    )
}

fn stream_response(
    stream_msg_id: &str,
    timestamp: &str,
    ref_idx: &str,
    remain_msg_len: i64,
    seq: i64,
    conversation_id: &str,
) -> Value {
    let mut ext_info = Map::new();
    if !ref_idx.is_empty() {
        ext_info.insert("ref_idx".into(), Value::String(ref_idx.to_string()));
    }
    json!({
        "id": stream_msg_id,
        "timestamp": timestamp,
        "ext_info": Value::Object(ext_info),
        "remain_msg_len": remain_msg_len.max(0),
        "seq": seq,
        "conversation_id": conversation_id,
    })
}

/// 官方只写了字段类型；沙箱按 Pydantic 时代的做法拒绝错误类型而不是静默强制转换。
fn validate_stream_body(body: &Value) -> MqsResult<()> {
    let Some(object) = body.as_object() else {
        return Err(invalid_param("invalid parameter: body must be an object"));
    };
    for key in [
        "content_raw",
        "content_type",
        "input_mode",
        "event_id",
        "msg_id",
        "stream_msg_id",
    ] {
        if let Some(value) = object.get(key) {
            if !value.is_null() && !value.is_string() {
                return Err(invalid_param(format!(
                    "invalid parameter: {key}: must be a string"
                )));
            }
        }
    }
    for key in ["index", "input_state", "msg_seq"] {
        if let Some(value) = object.get(key) {
            if !value.is_null() && value.as_i64().is_none() {
                return Err(invalid_param(format!(
                    "invalid parameter: {key}: must be an integer"
                )));
            }
        }
    }
    if let Some(value) = object.get("is_wakeup") {
        if !value.is_null() && !value.is_boolean() {
            return Err(invalid_param(
                "invalid parameter: is_wakeup: must be a boolean",
            ));
        }
    }
    Ok(())
}
