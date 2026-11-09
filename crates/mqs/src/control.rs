//! Control-plane API (token + origin protected) with the embedded Vue GUI.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{FromRequest, Multipart, Path, Query, Request, State, WebSocketUpgrade};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde_json::{json, Map, Value};

use crate::assets;
use crate::clock::parse_ts;
use crate::config::VERSION;
use crate::db::s;
use crate::error::{invalid_param, ApiError, MqsResult};
use crate::http::{
    is_authorized, openapi_document, origin_guard_mw, require_control_mw, trace_mw, AppState,
    QqJson,
};
#[allow(unused_imports)]
use crate::models::MediaUploadRequest;
use crate::models::*;

// ------------------------------------------------------------------ public

async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({"ok": true, "profile": state.config.profile, "mqs": VERSION}))
}

fn origin_allowed(state: &AppState, headers: &axum::http::HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    crate::http::origin_is_allowed(state, origin)
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    if state.nonce_created.elapsed() > Duration::from_secs(3600) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"code": 11241, "message": "login nonce expired"})),
        )
            .into_response();
    }
    if !origin_allowed(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"code": 40040, "message": "origin not allowed"})),
        )
            .into_response();
    }
    let supplied = body
        .as_ref()
        .and_then(|Json(value)| value.get("nonce").cloned())
        .and_then(|v| v.as_str().map(str::to_string))
        .or_else(|| {
            headers
                .get("x-mqs-nonce")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default();
    if supplied.is_empty() || supplied != state.nonce {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"code": 11241, "message": "invalid nonce"})),
        )
            .into_response();
    }
    let session = crate::ids::short_token(32);
    state.sessions.lock().unwrap().insert(session.clone());
    let mut response = Json(json!({"ok": true})).into_response();
    if let Ok(cookie) = HeaderValue::from_str(&format!(
        "mqs_session={session}; HttpOnly; SameSite=Strict; Max-Age=43200; Path=/"
    )) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

async fn auth_check(State(state): State<Arc<AppState>>, req: Request) -> Response {
    if is_authorized(&state, &req) {
        Json(json!({"ok": true})).into_response()
    } else {
        ApiError::new(11241, "missing or invalid MQS control token").into_response()
    }
}

// ------------------------------------------------------------------ status / settings
async fn status(State(state): State<Arc<AppState>>) -> MqsResult<Json<Value>> {
    let mut data = state.world.status()?;
    data["sim_port"] = Value::from(state.config.sim_port);
    data["control_port"] = Value::from(state.config.control_port);
    data["host"] = Value::String(state.config.host.clone());
    data["base_url"] = Value::String(state.config.base_url());
    data["ws_url"] = Value::String(state.config.ws_url());
    data["profile_dir"] =
        Value::String(state.world.paths.directory().to_string_lossy().into_owned());
    data["gui_url"] = Value::String(state.config.gui_url.clone());
    Ok(Json(data))
}

async fn list_profiles(State(state): State<Arc<AppState>>) -> Json<Value> {
    let mut items = Vec::new();
    let root = state.world.paths.home.join("profiles");
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect();
    names.sort();
    for name in names {
        let directory = root.join(&name);
        let meta: Value = std::fs::read_to_string(directory.join("profile.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(json!({}));
        let runtime: Value = std::fs::read_to_string(directory.join("runtime.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(json!({}));
        let mut running = false;
        if let Some(port) = runtime.get("control_port").and_then(|v| v.as_u64()) {
            if let Ok(client) = reqwest::Client::builder()
                .timeout(Duration::from_millis(600))
                .build()
            {
                running = client
                    .get(format!("http://127.0.0.1:{port}/_mqs/health"))
                    .send()
                    .await
                    .map(|response| response.status().is_success())
                    .unwrap_or(false);
            }
        }
        items.push(json!({
            "name": name,
            "path": directory.to_string_lossy(),
            "running": running,
            "current": name == state.world.profile,
            "sim_port": runtime.get("sim_port").cloned().unwrap_or(Value::Null),
            "control_port": runtime.get("control_port").cloned().unwrap_or(Value::Null),
            "gui_url": runtime.get("gui_url").cloned().unwrap_or(Value::Null),
            "created_at": meta.get("created_at").cloned().unwrap_or(Value::Null),
        }));
    }
    Json(Value::Array(items))
}

async fn get_settings(State(state): State<Arc<AppState>>) -> MqsResult<Json<Value>> {
    Ok(Json(Value::Object(state.world.db.all_settings()?)))
}

async fn patch_settings(
    State(state): State<Arc<AppState>>,
    QqJson(payload): QqJson<SettingsPatch>,
) -> MqsResult<Json<Value>> {
    state.world.update_settings(&payload.extra)?;
    if let Some(seed) = payload.extra.get("seed").and_then(|v| v.as_i64()) {
        state.world.db.set_setting("seed", &Value::from(seed))?;
    }
    Ok(Json(Value::Object(state.world.db.all_settings()?)))
}

// ------------------------------------------------------------------ users
async fn list_users(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Map<String, Value>>,
) -> MqsResult<Json<Value>> {
    let actor_type = query.get("actor_type").and_then(|v| v.as_str());
    let available = query
        .get("available")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(Json(Value::Array(
        state.world.list_users(actor_type, available)?,
    )))
}

async fn create_user(
    State(state): State<Arc<AppState>>,
    QqJson(payload): QqJson<UserCreate>,
) -> MqsResult<Json<Value>> {
    if payload.random.unwrap_or(false) {
        let created = state.world.create_random_users(
            1,
            payload.seed,
            payload.behavior.as_deref(),
            payload.actor_type.as_deref().unwrap_or("human"),
            None,
        )?;
        return Ok(Json(created.into_iter().next().unwrap_or(Value::Null)));
    }
    Ok(Json(state.world.create_user(
        payload.nickname.as_deref(),
        payload.avatar.as_deref(),
        payload.gender.unwrap_or(0),
        payload.age.unwrap_or(0),
        payload.city.as_deref().unwrap_or(""),
        payload.actor_type.as_deref().unwrap_or("human"),
        payload.behavior.as_deref().unwrap_or("normal"),
        payload.seed,
    )?))
}

async fn create_random_users(
    State(state): State<Arc<AppState>>,
    QqJson(payload): QqJson<RandomUsersRequest>,
) -> MqsResult<Json<Value>> {
    Ok(Json(Value::Array(state.world.create_random_users(
        payload.count.unwrap_or(1),
        payload.seed,
        payload.behavior.as_deref(),
        payload.actor_type.as_deref().unwrap_or("human"),
        payload.group_id.as_deref(),
    )?)))
}

async fn get_user(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.get_user(&user_id)?))
}

async fn patch_user(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
    QqJson(payload): QqJson<UserPatch>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.update_user(&user_id, &to_map(&payload))?))
}

async fn delete_user(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
) -> MqsResult<StatusCode> {
    state.world.delete_user(&user_id)?;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------ media upload
async fn upload_media(State(state): State<Arc<AppState>>, req: Request) -> MqsResult<Json<Value>> {
    let mut form = Multipart::from_request(req, &state)
        .await
        .map_err(|err| invalid_param(format!("invalid multipart body: {err}")))?;
    while let Some(field) = form
        .next_field()
        .await
        .map_err(|err| invalid_param(format!("invalid multipart body: {err}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let mime = field
            .content_type()
            .map(str::to_string)
            .unwrap_or_else(|| "application/octet-stream".into());
        let filename = field.file_name().map(str::to_string).unwrap_or_default();
        let data = field
            .bytes()
            .await
            .map_err(|err| invalid_param(format!("invalid file field: {err}")))?
            .to_vec();
        if data.is_empty() {
            return Err(invalid_param("empty file"));
        }
        if data.len() > 10 * 1024 * 1024 {
            return Err(invalid_param("file too large (max 10 MiB)"));
        }
        if !(mime.starts_with("image/") || mime.starts_with("video/") || mime.starts_with("audio/"))
        {
            return Err(invalid_param("only image/video/audio files are supported"));
        }
        let media = state.world.store_media(&data, &mime)?;
        let hash = s(&media, "hash");
        return Ok(Json(json!({
            "hash": hash,
            "url": format!("/media/{hash}"),
            "content_type": mime,
            "size": data.len(),
            "filename": if filename.is_empty() { hash[..12].to_string() } else { filename },
        })));
    }
    Err(invalid_param("multipart upload requires a 'file' field"))
}

// ------------------------------------------------------------------ groups
async fn list_groups(State(state): State<Arc<AppState>>) -> MqsResult<Json<Value>> {
    Ok(Json(Value::Array(state.world.list_groups()?)))
}

async fn create_group(
    State(state): State<Arc<AppState>>,
    QqJson(payload): QqJson<GroupCreate>,
) -> MqsResult<Json<Value>> {
    let member_ids = payload.member_ids.clone().unwrap_or_default();
    Ok(Json(state.world.create_group(
        payload.name.as_deref(),
        payload.owner_id.as_deref(),
        &member_ids,
        &payload.bot_ids.clone().unwrap_or_default(),
        payload.join_policy.as_deref().unwrap_or("open"),
        payload.random_members.unwrap_or(0),
    )?))
}

async fn get_group(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.get_group(&group_id)?))
}

async fn patch_group(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
    QqJson(payload): QqJson<GroupPatch>,
) -> MqsResult<Json<Value>> {
    Ok(Json(
        state.world.update_group(&group_id, &to_map(&payload))?,
    ))
}

async fn delete_group(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
) -> MqsResult<StatusCode> {
    state.world.delete_group(&group_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_members(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
) -> MqsResult<Json<Value>> {
    Ok(Json(Value::Array(state.world.list_members(&group_id)?)))
}

async fn add_member(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
    QqJson(payload): QqJson<MemberAdd>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.add_member(
        &group_id,
        &payload.user_id,
        payload.role.as_deref().unwrap_or("member"),
    )?))
}

async fn patch_member(
    State(state): State<Arc<AppState>>,
    Path((group_id, user_id)): Path<(String, String)>,
    QqJson(payload): QqJson<MemberPatch>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.update_member(
        &group_id,
        &user_id,
        &to_map(&payload),
    )?))
}

async fn delete_member(
    State(state): State<Arc<AppState>>,
    Path((group_id, user_id)): Path<(String, String)>,
) -> MqsResult<StatusCode> {
    state.world.remove_member(&group_id, &user_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn add_bot_to_group(
    State(state): State<Arc<AppState>>,
    Path((group_id, bot_id)): Path<(String, String)>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.add_bot_to_group(&group_id, &bot_id)?))
}

async fn remove_bot_from_group(
    State(state): State<Arc<AppState>>,
    Path((group_id, bot_id)): Path<(String, String)>,
) -> MqsResult<StatusCode> {
    state.world.remove_bot_from_group(&group_id, &bot_id)?;
    Ok(StatusCode::NO_CONTENT)
}

fn first_bot_in_group(state: &AppState, group_id: &str) -> MqsResult<String> {
    state
        .world
        .db
        .one(
            "SELECT bot_id FROM group_bots WHERE group_id=? LIMIT 1",
            &crate::p![group_id],
        )?
        .map(|row| s(&row, "bot_id"))
        .ok_or_else(|| invalid_param("no bot in this group"))
}

async fn control_group_info(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
    Query(query): Query<Map<String, Value>>,
) -> MqsResult<Json<Value>> {
    let bot_id = query
        .get("bot_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or(first_bot_in_group(&state, &group_id)?);
    Ok(Json(state.world.group_info(&group_id, &bot_id)?))
}

async fn control_group_bot_state(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
    Query(query): Query<Map<String, Value>>,
) -> MqsResult<Json<Value>> {
    let bot_id = query
        .get("bot_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or(first_bot_in_group(&state, &group_id)?);
    Ok(Json(state.world.group_bot_state(&group_id, &bot_id)?))
}

async fn control_group_bot_patch(
    State(state): State<Arc<AppState>>,
    Path((group_id, bot_id)): Path<(String, String)>,
    QqJson(payload): QqJson<GroupBotPatch>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.update_group_bot_settings(
        &group_id,
        &bot_id,
        &to_map(&payload),
    )?))
}

async fn control_group_blacklist(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
) -> MqsResult<Json<Value>> {
    Ok(Json(Value::Array(
        state
            .world
            .db
            .all(
                "SELECT gb.user_id, gb.banned_at, u.nickname FROM group_blacklist gb \
                 JOIN users u ON u.id=gb.user_id WHERE gb.group_id=? ORDER BY gb.banned_at",
                &crate::p![&group_id],
            )?
            .iter()
            .map(|row| Value::Object(row.clone()))
            .collect(),
    )))
}

async fn control_group_blacklist_update(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
    QqJson(body): QqJson<Value>,
) -> MqsResult<Json<Value>> {
    let op = body
        .get("op")
        .and_then(|v| v.as_str())
        .unwrap_or("add")
        .to_string();
    let user_id = body
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if user_id.is_empty() {
        return Err(invalid_param("user_id is required"));
    }
    if op == "add" {
        if state
            .world
            .db
            .one(
                "SELECT 1 FROM group_members WHERE group_id=? AND user_id=?",
                &crate::p![&group_id, &user_id],
            )?
            .is_some()
        {
            return Err(invalid_param(
                "user is still in this group; remove them first",
            ));
        }
        state.world.db.execute(
            "INSERT INTO group_blacklist(group_id,user_id,banned_at) VALUES(?,?,?) \
             ON CONFLICT(group_id,user_id) DO NOTHING",
            &crate::p![&group_id, &user_id, state.world.clock.now_ms()],
        )?;
    } else {
        state.world.db.execute(
            "DELETE FROM group_blacklist WHERE group_id=? AND user_id=?",
            &crate::p![&group_id, &user_id],
        )?;
    }
    Ok(Json(json!({"ok": true})))
}

async fn control_join_requests(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
) -> MqsResult<Json<Value>> {
    Ok(Json(Value::Array(
        state
            .world
            .db
            .all(
                "SELECT jr.*, u.nickname FROM group_join_requests jr JOIN users u ON u.id=jr.user_id \
                 WHERE jr.group_id=? AND jr.status='pending' ORDER BY jr.apply_at",
                &crate::p![&group_id],
            )?
            .iter()
            .map(|row| Value::Object(row.clone()))
            .collect(),
    )))
}

async fn control_create_join_request(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
    QqJson(payload): QqJson<JoinRequestCreate>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.create_join_request(
        &group_id,
        &payload.user_id,
        payload.verify_message.as_deref().unwrap_or(""),
        payload.source.as_deref().unwrap_or("self_apply"),
    )?))
}

async fn control_decide_join_request(
    State(state): State<Arc<AppState>>,
    Path((group_id, request_id)): Path<(String, String)>,
    QqJson(payload): QqJson<JoinRequestDecision>,
) -> MqsResult<Json<Value>> {
    let row = state
        .world
        .db
        .one(
            "SELECT * FROM group_join_requests WHERE id=? AND group_id=?",
            &crate::p![&request_id, &group_id],
        )?
        .ok_or_else(|| invalid_param("join request not found"))?;
    let bot_id = first_bot_in_group(&state, &group_id)?;
    let openid = state.world.openid(&bot_id, "user", &s(&row, "user_id"))?;
    state.world.approve_join_request(
        &group_id,
        &bot_id,
        &openid,
        &payload.op,
        &request_id,
        payload.reject_reason.as_deref().unwrap_or(""),
        payload.add_to_blacklist.unwrap_or(false),
    )?;
    Ok(Json(json!({"ok": true})))
}

// ------------------------------------------------------------------ bots
async fn list_bots(State(state): State<Arc<AppState>>) -> MqsResult<Json<Value>> {
    Ok(Json(Value::Array(state.world.list_bots()?)))
}

async fn create_bot(
    State(state): State<Arc<AppState>>,
    QqJson(payload): QqJson<BotCreate>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.create_bot(
        payload.app_id.as_deref(),
        payload.secret.as_deref(),
        payload.name.as_deref(),
        payload.avatar.as_deref(),
        payload.scopes.clone(),
        payload.delivery.as_deref().unwrap_or("ws"),
        payload.random.unwrap_or(false),
        payload.whitelisted.unwrap_or(false),
    )?))
}

async fn get_bot(
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.get_bot(&bot_id)?))
}

async fn patch_bot(
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    QqJson(payload): QqJson<BotPatch>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.update_bot(&bot_id, &to_map(&payload))?))
}

async fn delete_bot(
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
) -> MqsResult<StatusCode> {
    state.world.delete_bot(&bot_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn rotate_secret(
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.rotate_secret(&bot_id)?))
}

async fn issue_bot_token(
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
) -> MqsResult<Json<Value>> {
    let bot = state.world.bot_by_id(&bot_id)?;
    Ok(Json(
        state
            .world
            .issue_token(&s(&bot, "app_id"), &s(&bot, "secret"))?,
    ))
}

async fn list_webhooks(
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
) -> MqsResult<Json<Value>> {
    Ok(Json(Value::Array(
        state
            .world
            .db
            .all(
                "SELECT * FROM webhook_endpoints WHERE bot_id=?",
                &crate::p![&bot_id],
            )?
            .iter()
            .map(|row| Value::Object(row.clone()))
            .collect(),
    )))
}

async fn add_webhook(
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    QqJson(body): QqJson<Value>,
) -> MqsResult<Json<Value>> {
    state.world.bot_by_id(&bot_id)?;
    let url = body.get("url").and_then(|v| v.as_str()).unwrap_or("");
    if url.is_empty() {
        return Err(invalid_param("url is required"));
    }
    let endpoint_id = format!("wh_{}", crate::ids::short_token(12).to_lowercase());
    let secret = body
        .get("secret")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("mqs-wh-{}", crate::ids::short_token(16).to_lowercase()));
    state.world.db.execute(
        "INSERT INTO webhook_endpoints(id,bot_id,url,secret,enabled) VALUES(?,?,?,?,1)",
        &crate::p![&endpoint_id, &bot_id, url, &secret],
    )?;
    Ok(Json(
        json!({"id": endpoint_id, "url": url, "bot_id": bot_id}),
    ))
}

async fn delete_webhook(
    State(state): State<Arc<AppState>>,
    Path((bot_id, endpoint_id)): Path<(String, String)>,
) -> MqsResult<StatusCode> {
    state.world.db.execute(
        "DELETE FROM webhook_endpoints WHERE id=? AND bot_id=?",
        &crate::p![&endpoint_id, &bot_id],
    )?;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------ conversations / chat
async fn list_conversations(State(state): State<Arc<AppState>>) -> MqsResult<Json<Value>> {
    Ok(Json(Value::Array(state.world.list_conversations()?)))
}

async fn list_messages(
    State(state): State<Arc<AppState>>,
    Path(conversation_id): Path<String>,
    Query(query): Query<Map<String, Value>>,
) -> MqsResult<Json<Value>> {
    let limit = query.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let before_seq = query.get("before_seq").and_then(|v| v.as_i64());
    let messages = state
        .world
        .list_messages(&conversation_id, limit, before_seq)?;
    let cursor = messages.first().and_then(|m| m["seq"].as_i64());
    Ok(Json(json!({"items": messages, "next_cursor": cursor})))
}

async fn click_button(
    State(state): State<Arc<AppState>>,
    Path(message_id): Path<String>,
    QqJson(payload): QqJson<KeyboardClick>,
) -> MqsResult<Json<Value>> {
    Ok(Json(
        state
            .world
            .click_keyboard_button(&payload.user_id, &message_id, payload.index.unwrap_or(0))
            .await?,
    ))
}

async fn chat_send(
    State(state): State<Arc<AppState>>,
    QqJson(payload): QqJson<ChatSend>,
) -> MqsResult<Json<Value>> {
    let conversation_id = state
        .world
        .resolve_conversation(payload.to.as_deref(), payload.conversation_id.as_deref())?;
    let mut at_bots = payload.at_bots;
    if payload.mentions.is_some() && at_bots.is_none() {
        at_bots = Some(Vec::new());
    }
    let attachments = payload.attachments.unwrap_or_default();
    Ok(Json(
        state
            .world
            .send_as_user(
                &conversation_id,
                &payload.as_user,
                payload.content.as_deref().unwrap_or(""),
                payload.msg_type.unwrap_or(0),
                payload.mentions.as_deref().unwrap_or(&[]),
                at_bots.as_deref(),
                None,
                payload.reply_to.as_deref(),
                &attachments,
            )
            .await?,
    ))
}

// ------------------------------------------------------------------ debugger
async fn debug_frames(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Map<String, Value>>,
) -> MqsResult<Json<Value>> {
    let frames = state.world.list_frames(
        query.get("since_id").and_then(|v| v.as_i64()).unwrap_or(0),
        query.get("limit").and_then(|v| v.as_i64()).unwrap_or(100),
        query.get("bot_id").and_then(|v| v.as_str()),
        query.get("kind").and_then(|v| v.as_str()),
        query.get("q").and_then(|v| v.as_str()),
    )?;
    let cursor = frames
        .last()
        .and_then(|frame| frame["id"].as_i64())
        .unwrap_or(0);
    Ok(Json(json!({"items": frames, "next_cursor": cursor})))
}

async fn debug_events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Map<String, Value>>,
) -> MqsResult<Json<Value>> {
    let events = state.world.list_events(
        query.get("since_seq").and_then(|v| v.as_i64()).unwrap_or(0),
        query.get("limit").and_then(|v| v.as_i64()).unwrap_or(100),
        query.get("conversation_id").and_then(|v| v.as_str()),
    )?;
    let cursor = events
        .last()
        .and_then(|event| event["seq"].as_i64())
        .unwrap_or(0);
    Ok(Json(json!({"items": events, "next_cursor": cursor})))
}

// ------------------------------------------------------------------ clock / rate
async fn clock_advance(
    State(state): State<Arc<AppState>>,
    QqJson(payload): QqJson<ClockAdvance>,
) -> MqsResult<Json<Value>> {
    let ms = match payload.ms {
        Some(ms) => ms,
        None => crate::clock::parse_duration(payload.duration.as_deref().unwrap_or(""))
            .map_err(invalid_param)?,
    };
    state.world.clock.advance(ms);
    state.world.persist_clock()?;
    Ok(Json(
        json!({"clock": state.world.clock.snapshot(), "advanced_ms": ms}),
    ))
}

async fn clock_set(
    State(state): State<Arc<AppState>>,
    QqJson(payload): QqJson<ClockSet>,
) -> MqsResult<Json<Value>> {
    if let Some(mode) = payload.mode.as_deref() {
        state.world.clock.set_mode(mode);
        state
            .world
            .db
            .set_setting("clock_mode", &Value::String(mode.to_string()))?;
    }
    if let Some(ms) = payload.ms {
        state.world.clock.set_ms(ms);
    } else if let Some(iso) = payload.iso.as_deref() {
        state
            .world
            .clock
            .set_ms(parse_ts(iso).map_err(invalid_param)?);
    }
    state.world.persist_clock()?;
    Ok(Json(json!({"clock": state.world.clock.snapshot()})))
}

async fn rate_reset(State(state): State<Arc<AppState>>) -> MqsResult<Json<Value>> {
    state.world.rate.lock().unwrap().reset();
    state.world.db.execute("DELETE FROM rate_buckets", &[]).ok();
    Ok(Json(
        json!({"ok": true, "rate": state.world.rate.lock().unwrap().stats()}),
    ))
}

// ------------------------------------------------------------------ rules / scenarios
async fn list_rules(State(state): State<Arc<AppState>>) -> MqsResult<Json<Value>> {
    Ok(Json(Value::Array(state.world.list_rules()?)))
}

async fn create_rule(
    State(state): State<Arc<AppState>>,
    QqJson(payload): QqJson<RuleCreate>,
) -> MqsResult<Json<Value>> {
    Ok(Json(state.world.create_rule(&to_map(&payload))?))
}

async fn delete_rule(
    State(state): State<Arc<AppState>>,
    Path(rule_id): Path<String>,
) -> MqsResult<StatusCode> {
    state.world.delete_rule(&rule_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_scenarios(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(Value::Array(crate::scenario::catalog(&state.world)))
}

async fn run_scenario(
    State(state): State<Arc<AppState>>,
    QqJson(payload): QqJson<ScenarioRunRequest>,
) -> MqsResult<Json<Value>> {
    let result = crate::scenario::run_payload(
        &state.world,
        payload.script.as_deref(),
        payload.path.as_deref(),
        &state.config.base_url(),
    )
    .await?;
    Ok(Json(result))
}

// ------------------------------------------------------------------ snapshot / contract
async fn snapshot(State(state): State<Arc<AppState>>) -> MqsResult<Response> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().to_path_buf();
    state
        .world
        .db
        .checkpoint_for_snapshot(&root.join("db.sqlite"))?;
    let meta = state.world.paths.meta();
    if meta.exists() {
        std::fs::copy(&meta, root.join("profile.json"))?;
    }
    copy_dir(&state.world.paths.media(), &root.join("media"))?;
    let name = format!(
        "mqs-{}-{}.zip",
        state.world.profile,
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    );
    let buffer = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(buffer);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    add_dir_to_zip(&mut writer, &root, &root, options)?;
    let cursor = writer
        .finish()
        .map_err(|err| crate::error::internal(format!("zip: {err}")))?;
    let data = cursor.into_inner();
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        data,
    )
        .into_response())
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    if !from.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(from)?.flatten() {
        let path = entry.path();
        if path.is_file() {
            std::fs::copy(&path, to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn add_dir_to_zip(
    writer: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>,
    root: &std::path::Path,
    dir: &std::path::Path,
    options: zip::write::SimpleFileOptions,
) -> std::io::Result<()> {
    use std::io::Write;
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            add_dir_to_zip(writer, root, &path, options)?;
        } else {
            writer
                .start_file(relative, options)
                .map_err(|err| std::io::Error::other(err.to_string()))?;
            writer.write_all(&std::fs::read(&path)?)?;
        }
    }
    Ok(())
}

async fn contract() -> Json<Value> {
    Json(json!({
        "summary": crate::contract::summary(),
        "endpoints": crate::contract::routes_table(),
    }))
}

async fn contract_types() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        crate::contract::typescript(),
    )
        .into_response()
}

async fn contract_coverage() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        crate::contract::coverage_markdown(),
    )
        .into_response()
}

// ------------------------------------------------------------------ websocket
async fn ws_ticket(State(state): State<Arc<AppState>>) -> Json<Value> {
    let ticket = crate::ids::short_token(24);
    let expires = chrono::Utc::now().timestamp_millis() as f64 + 30_000.0;
    state
        .ws_tickets
        .lock()
        .unwrap()
        .insert(ticket.clone(), expires);
    Json(json!({"ticket": ticket, "expires_in": 30}))
}

async fn control_websocket(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Map<String, Value>>,
    ws: WebSocketUpgrade,
) -> Response {
    let ticket = query.get("ticket").and_then(|v| v.as_str()).unwrap_or("");
    let expires = state.ws_tickets.lock().unwrap().remove(ticket);
    let now = chrono::Utc::now().timestamp_millis() as f64;
    match expires {
        Some(expires) if expires >= now => ws.on_upgrade(move |socket| async move {
            handle_control_ws(state, socket).await;
        }),
        _ => (
            StatusCode::FORBIDDEN,
            Json(json!({"code": 11241, "message": "invalid ws ticket"})),
        )
            .into_response(),
    }
}

async fn handle_control_ws(state: Arc<AppState>, mut socket: axum::extract::ws::WebSocket) {
    use axum::extract::ws::Message;
    let mut receiver = state.world.hub.subscribe();
    let _ = socket
        .send(Message::Text(
            json!({"kind": "hello", "profile": state.config.profile})
                .to_string()
                .into(),
        ))
        .await;
    loop {
        tokio::select! {
            message = receiver.recv() => {
                match message {
                    Ok(payload) => {
                        if socket.send(Message::Text(payload.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            incoming = socket.next() => {
                match incoming {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                    Some(Ok(_)) => continue,
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                if socket.send(Message::Ping(bytes::Bytes::new())).await.is_err() {
                    break;
                }
            }
        }
    }
    state.world.hub.unsubscribe();
}

// ------------------------------------------------------------------ static GUI
async fn asset_response(path: &str) -> Response {
    let clean = path.trim_start_matches('/');
    if let Some((data, mime)) = assets::get(if clean.is_empty() {
        "index.html"
    } else {
        clean
    }) {
        return (StatusCode::OK, [(header::CONTENT_TYPE, mime)], data).into_response();
    }
    let has_extension = clean
        .rsplit('/')
        .next()
        .map(|n| n.contains('.'))
        .unwrap_or(false);
    if has_extension {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    // SPA fallback
    match assets::get("index.html") {
        Some((data, mime)) => {
            (StatusCode::OK, [(header::CONTENT_TYPE, mime)], data).into_response()
        }
        None => Html(assets::FALLBACK_HTML).into_response(),
    }
}

async fn static_root() -> Response {
    asset_response("index.html").await
}

async fn static_fallback(Path(path): Path<String>) -> Response {
    asset_response(&path).await
}

async fn control_media(
    State(state): State<Arc<AppState>>,
    Path(hash): Path<String>,
) -> MqsResult<Response> {
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid_param("invalid media hash"));
    }
    let media = state
        .world
        .media_by_hash(&hash)?
        .ok_or_else(|| invalid_param("media not found"))?;
    let path = state.world.paths.media().join(&hash);
    if !path.exists() {
        return Err(invalid_param("media not found"));
    }
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, s(&media, "mime"))],
        std::fs::read(path)?,
    )
        .into_response())
}

async fn control_avatar(
    Path(key): Path<String>,
    Query(query): Query<Map<String, Value>>,
) -> Response {
    let key = key.trim_end_matches(".svg").to_string();
    if key.is_empty()
        || key.len() > 64
        || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"code": 10001, "message": "invalid avatar key"})),
        )
            .into_response();
    }
    let label = query
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(16)
        .collect::<String>();
    let mut kind = query
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("user")
        .to_string();
    if !["bot", "user", "npc", "group"].contains(&kind.as_str()) {
        kind = "user".into();
    }
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/svg+xml".to_string()),
            (header::CACHE_CONTROL, "public, max-age=3600".to_string()),
        ],
        crate::ids::avatar_svg(&key, &label, &kind),
    )
        .into_response()
}

async fn control_openapi() -> Json<Value> {
    Json(openapi_document("QSandbox Control API", VERSION))
}

// ------------------------------------------------------------------ app
pub fn control_router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/_mqs/status", get(status))
        .route("/_mqs/profiles", get(list_profiles))
        .route("/_mqs/settings", get(get_settings).patch(patch_settings))
        .route("/_mqs/users", get(list_users).post(create_user))
        .route("/_mqs/users/random", post(create_random_users))
        .route(
            "/_mqs/users/{user_id}",
            get(get_user).patch(patch_user).delete(delete_user),
        )
        .route("/_mqs/media", post(upload_media))
        .route("/_mqs/groups", get(list_groups).post(create_group))
        .route(
            "/_mqs/groups/{group_id}",
            get(get_group).patch(patch_group).delete(delete_group),
        )
        .route(
            "/_mqs/groups/{group_id}/members",
            get(list_members).post(add_member),
        )
        .route(
            "/_mqs/groups/{group_id}/members/{user_id}",
            patch(patch_member).delete(delete_member),
        )
        .route("/_mqs/groups/{group_id}/info", get(control_group_info))
        .route(
            "/_mqs/groups/{group_id}/bot-state",
            get(control_group_bot_state),
        )
        .route(
            "/_mqs/groups/{group_id}/bots/{bot_id}",
            post(add_bot_to_group)
                .delete(remove_bot_from_group)
                .patch(control_group_bot_patch),
        )
        .route(
            "/_mqs/groups/{group_id}/blacklist",
            get(control_group_blacklist).post(control_group_blacklist_update),
        )
        .route(
            "/_mqs/groups/{group_id}/join-requests",
            get(control_join_requests).post(control_create_join_request),
        )
        .route(
            "/_mqs/groups/{group_id}/join-requests/{request_id}/decide",
            post(control_decide_join_request),
        )
        .route("/_mqs/bots", get(list_bots).post(create_bot))
        .route(
            "/_mqs/bots/{bot_id}",
            get(get_bot).patch(patch_bot).delete(delete_bot),
        )
        .route("/_mqs/bots/{bot_id}/rotate-secret", post(rotate_secret))
        .route("/_mqs/bots/{bot_id}/token", post(issue_bot_token))
        .route(
            "/_mqs/bots/{bot_id}/webhook",
            get(list_webhooks).post(add_webhook),
        )
        .route(
            "/_mqs/bots/{bot_id}/webhook/{endpoint_id}",
            delete(delete_webhook),
        )
        .route("/_mqs/conversations", get(list_conversations))
        .route(
            "/_mqs/conversations/{conversation_id}/messages",
            get(list_messages),
        )
        .route("/_mqs/messages/{message_id}/click", post(click_button))
        .route("/_mqs/chat/send", post(chat_send))
        .route("/_mqs/debug/frames", get(debug_frames))
        .route("/_mqs/debug/events", get(debug_events))
        .route("/_mqs/clock/advance", post(clock_advance))
        .route("/_mqs/clock/set", post(clock_set))
        .route("/_mqs/rate/reset", post(rate_reset))
        .route("/_mqs/rules", get(list_rules).post(create_rule))
        .route("/_mqs/rules/{rule_id}", delete(delete_rule))
        .route("/_mqs/scenarios", get(list_scenarios))
        .route("/_mqs/scenarios/run", post(run_scenario))
        .route("/_mqs/snapshot", post(snapshot))
        .route("/_mqs/contract", get(contract))
        .route("/_mqs/contract/types.ts", get(contract_types))
        .route("/_mqs/contract/coverage.md", get(contract_coverage))
        .route("/_mqs/ws-ticket", post(ws_ticket))
        .route("/_mqs/ws", get(control_websocket))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_control_mw,
        ));

    let public = Router::new()
        .route("/_mqs/health", get(health))
        .route("/_mqs/session", post(create_session))
        .route("/_mqs/auth-check", get(auth_check));

    let app = Router::new()
        .merge(public)
        .merge(protected)
        .route("/", get(static_root))
        .route("/media/{hash}", get(control_media))
        .route("/avatars/{key}", get(control_avatar))
        .route("/openapi.json", get(control_openapi))
        .route("/{*path}", get(static_fallback))
        .with_state(state.clone());

    Router::new()
        .merge(app)
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            origin_guard_mw,
        ))
        .layer(axum::middleware::from_fn(trace_mw))
}
