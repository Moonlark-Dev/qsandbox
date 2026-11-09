//! The simulated QQ Bot API (second listener).

use std::sync::Arc;

use axum::extract::{FromRequest, Multipart, Path, Query, Request, State, WebSocketUpgrade};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use base64::Engine;
use serde_json::{json, Map, Value};

use crate::config::VERSION;
use crate::error::{invalid_param, message_not_found, MqsResult};
use crate::http::{
    attach_rate, json_status, openapi_document, sim_log_mw, trace_mw, AppState, BotCtx, QqJson,
    WhitelistedBot,
};
use crate::ids::avatar_svg;
use crate::models::{AppAccessTokenRequest, MessageSendRequest};
use crate::world::{sniff_mime, World};

const FILE_LIMITS: [(i64, usize); 3] = [
    (1, 10 * 1024 * 1024),
    (2, 30 * 1024 * 1024),
    (3, 5 * 1024 * 1024),
];

fn check_file_type(file_type: i64) -> MqsResult<usize> {
    if file_type == 4 {
        return Err(invalid_param(
            "file_type 4 (generic file) is not supported in QSandbox",
        ));
    }
    FILE_LIMITS
        .iter()
        .find(|(kind, _)| *kind == file_type)
        .map(|(_, limit)| *limit)
        .ok_or_else(|| invalid_param(format!("invalid file_type: {file_type}")))
}

fn ws_base(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1")
        .to_string();
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let ws_scheme = if scheme == "https" { "wss" } else { "ws" };
    format!("{ws_scheme}://{host}")
}

// ------------------------------------------------------------------ auth / gateway
async fn get_app_access_token(
    State(_state): State<Arc<AppState>>,
    QqJson(payload): QqJson<AppAccessTokenRequest>,
) -> MqsResult<Json<Value>> {
    let world = &_state.world;
    let app_id = match &payload.app_id {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    };
    Ok(Json(world.issue_token(&app_id, &payload.client_secret)?))
}

async fn gateway_url(headers: HeaderMap, BotCtx(_bot): BotCtx) -> Json<Value> {
    Json(json!({"url": format!("{}/websocket", ws_base(&headers))}))
}

async fn gateway_bot(headers: HeaderMap, BotCtx(_bot): BotCtx) -> Json<Value> {
    Json(json!({
        "url": format!("{}/websocket", ws_base(&headers)),
        "shards": 1,
        "session_start_limit": {"total": 1000, "remaining": 999, "reset_after": 86400000, "max_concurrency": 1},
    }))
}

async fn bot_me(BotCtx(bot): BotCtx) -> Json<Value> {
    Json(json!({
        "id": bot["app_id"],
        "username": bot["name"],
        "avatar": bot["avatar"],
        "bot": true,
        "union_openid": null,
        "share_url": null,
    }))
}

// ------------------------------------------------------------------ messages
async fn send_group_message(
    State(state): State<Arc<AppState>>,
    Path(group_openid): Path<String>,
    BotCtx(bot): BotCtx,
    QqJson(payload): QqJson<MessageSendRequest>,
) -> MqsResult<Response> {
    let result = state
        .world
        .send_as_bot(
            bot["id"].as_str().unwrap_or(""),
            "group",
            &group_openid,
            &Value::Object(payload.body),
        )
        .await?;
    Ok(rate_and_json(result))
}

async fn send_c2c_message(
    State(state): State<Arc<AppState>>,
    Path(openid): Path<String>,
    BotCtx(bot): BotCtx,
    QqJson(payload): QqJson<MessageSendRequest>,
) -> MqsResult<Response> {
    let result = state
        .world
        .send_as_bot(
            bot["id"].as_str().unwrap_or(""),
            "user",
            &openid,
            &Value::Object(payload.body),
        )
        .await?;
    Ok(rate_and_json(result))
}

fn rate_and_json(mut result: Value) -> Response {
    let rate = result
        .as_object_mut()
        .and_then(|object| object.remove("_rate"));
    let response = Json(result).into_response();
    match rate {
        Some(rate) => attach_rate(
            response,
            &crate::ratelimit::RateInfo {
                limit: rate["limit"].as_i64().unwrap_or(0),
                remaining: rate["remaining"].as_i64().unwrap_or(0),
                reset_after: rate["reset_after"].as_f64().unwrap_or(0.0),
                bucket: rate["bucket"].as_str().unwrap_or("").to_string(),
            },
        ),
        None => response,
    }
}

async fn delete_group_message(
    State(state): State<Arc<AppState>>,
    Path((group_openid, message_id)): Path<(String, String)>,
    BotCtx(bot): BotCtx,
) -> MqsResult<StatusCode> {
    state.world.recall_message(
        bot["id"].as_str().unwrap_or(""),
        "group",
        &group_openid,
        &message_id,
    )?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_c2c_message(
    State(state): State<Arc<AppState>>,
    Path((openid, message_id)): Path<(String, String)>,
    BotCtx(bot): BotCtx,
) -> MqsResult<StatusCode> {
    state.world.recall_message(
        bot["id"].as_str().unwrap_or(""),
        "user",
        &openid,
        &message_id,
    )?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ack_interaction(
    State(state): State<Arc<AppState>>,
    Path(interaction_id): Path<String>,
    BotCtx(bot): BotCtx,
    body: Option<Json<Value>>,
) -> MqsResult<Json<Value>> {
    let code = body
        .as_ref()
        .and_then(|Json(value)| value.get("code"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    Ok(Json(state.world.ack_interaction(
        bot["id"].as_str().unwrap_or(""),
        &interaction_id,
        code,
    )?))
}

// ------------------------------------------------------------------ media
async fn store_upload(
    world: &World,
    file_type: i64,
    data: &[u8],
    mime: Option<&str>,
) -> MqsResult<crate::db::Row> {
    let limit = check_file_type(file_type)?;
    if data.is_empty() {
        return Err(invalid_param("empty file"));
    }
    if data.len() > limit {
        return Err(invalid_param(format!(
            "file too large: {} > {} bytes",
            data.len(),
            limit
        )));
    }
    let mime = mime.unwrap_or("application/octet-stream");
    if file_type == 1 && !mime.starts_with("image/") {
        return Err(invalid_param("file_type 1 expects an image"));
    }
    world.store_media(data, mime)
}

async fn resolve_media_url(world: &World, url: &str, file_type: i64) -> MqsResult<crate::db::Row> {
    check_file_type(file_type)?;
    let candidate = url
        .strip_prefix("mqs://media/")
        .map(|hash| format!("/media/{hash}"));
    let candidate = candidate.as_deref().unwrap_or(url);
    if let Some(hash) = candidate.strip_prefix("/media/") {
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(invalid_param("invalid media hash"));
        }
        return world
            .media_by_hash(hash)?
            .ok_or_else(|| invalid_param("media not found"));
    }
    if candidate.starts_with("http://") || candidate.starts_with("https://") {
        if !world.setting_bool("allow_remote_media") {
            return Err(invalid_param(
                "remote media URLs are disabled (enable allow_remote_media in settings)",
            ));
        }
        let response = reqwest::Client::new()
            .get(candidate)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|err| invalid_param(format!("failed to fetch media: {err}")))?;
        if !response.status().is_success() {
            return Err(invalid_param(format!(
                "failed to fetch media: HTTP {}",
                response.status().as_u16()
            )));
        }
        let mime = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let data = response
            .bytes()
            .await
            .map_err(|err| invalid_param(format!("failed to read media: {err}")))?;
        return store_upload(world, file_type, &data, mime.as_deref()).await;
    }
    Err(invalid_param(
        "unsupported media url (use /media/<sha256> or mqs://media/<sha256>)",
    ))
}

fn upload_query(uri: &axum::http::Uri) -> Map<String, Value> {
    axum::extract::Query::<Map<String, Value>>::try_from_uri(uri)
        .map(|query| query.0)
        .unwrap_or_default()
}

async fn upload_result(
    state: &Arc<AppState>,
    bot_id: &str,
    kind: &str,
    target_openid: &str,
    file_type: i64,
    srv_send_msg: bool,
    media: crate::db::Row,
) -> MqsResult<Value> {
    let mhash = crate::db::s(&media, "hash");
    let mut result = json!({
        "file_uuid": mhash,
        "file_info": format!("mqs-file-{mhash}"),
        "ttl": 0,
        "file_type": file_type,
    });
    if srv_send_msg {
        let sent = state
            .world
            .send_as_bot(
                bot_id,
                if kind == "group" { "group" } else { "user" },
                target_openid,
                &json!({"msg_type": 7, "content": "", "media": {"file_info": result["file_info"]}}),
            )
            .await?;
        result["message"] = sent;
    }
    Ok(result)
}

async fn upload_multipart(
    state: &Arc<AppState>,
    bot_id: &str,
    kind: &str,
    target_openid: &str,
    uri: axum::http::Uri,
    mut form: Multipart,
) -> MqsResult<Value> {
    let query = upload_query(&uri);
    let mut file_type = query.get("file_type").and_then(|v| v.as_i64()).unwrap_or(1);
    let mut srv_send_msg = false;
    let mut data: Option<Vec<u8>> = None;
    let mut mime: Option<String> = None;
    while let Some(field) = form
        .next_field()
        .await
        .map_err(|err| invalid_param(format!("invalid multipart body: {err}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" | "media" => {
                mime = field.content_type().map(str::to_string);
                data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|err| invalid_param(format!("invalid file field: {err}")))?
                        .to_vec(),
                );
            }
            "file_type" => {
                if let Ok(text) = field.text().await {
                    file_type = text.trim().parse().unwrap_or(file_type);
                }
            }
            "srv_send_msg" => {
                if let Ok(text) = field.text().await {
                    srv_send_msg = text == "1" || text.eq_ignore_ascii_case("true");
                }
            }
            _ => {}
        }
    }
    let data = data.ok_or_else(|| invalid_param("multipart upload requires a 'file' field"))?;
    let media = store_upload(&state.world, file_type, &data, mime.as_deref()).await?;
    upload_result(
        state,
        bot_id,
        kind,
        target_openid,
        file_type,
        srv_send_msg,
        media,
    )
    .await
}

async fn upload_json(
    state: &Arc<AppState>,
    bot_id: &str,
    kind: &str,
    target_openid: &str,
    body: Map<String, Value>,
) -> MqsResult<Value> {
    let file_type = body.get("file_type").and_then(|v| v.as_i64()).unwrap_or(1);
    let srv_send_msg = body
        .get("srv_send_msg")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if let Some(upload_id) = body.get("upload_id").and_then(|v| v.as_str()) {
        return state
            .world
            .finalize_upload(
                bot_id,
                kind,
                target_openid,
                upload_id,
                body.get("srv_send_msg").and_then(|v| v.as_bool()),
            )
            .await;
    }
    let media = if let Some(file_data) = body
        .get("file_data")
        .or_else(|| body.get("fileData"))
        .and_then(|v| v.as_str())
    {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(file_data)
            .map_err(|err| invalid_param(format!("invalid base64 file_data: {err}")))?;
        let mime = sniff_mime(&decoded);
        store_upload(&state.world, file_type, &decoded, Some(&mime)).await?
    } else if let Some(url) = body.get("url").and_then(|v| v.as_str()) {
        resolve_media_url(&state.world, url, file_type).await?
    } else {
        return Err(invalid_param(
            "url or file_data is required for JSON upload",
        ));
    };
    upload_result(
        state,
        bot_id,
        kind,
        target_openid,
        file_type,
        srv_send_msg,
        media,
    )
    .await
}

async fn handle_upload(
    state: Arc<AppState>,
    bot_id: String,
    kind: &'static str,
    target_openid: String,
    req: Request,
) -> MqsResult<Json<Value>> {
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let uri = req.uri().clone();
    if content_type.starts_with("multipart/") {
        let form = Multipart::from_request(req, &state)
            .await
            .map_err(|err| invalid_param(format!("invalid multipart body: {err}")))?;
        let result = upload_multipart(&state, &bot_id, kind, &target_openid, uri, form).await?;
        return Ok(Json(result));
    }
    let bytes = axum::body::to_bytes(req.into_body(), 32 * 1024 * 1024)
        .await
        .map_err(|err| invalid_param(format!("invalid request body: {err}")))?;
    let body: Map<String, Value> = serde_json::from_slice(&bytes)
        .map_err(|err| invalid_param(format!("invalid parameter: {err}")))?;
    Ok(Json(
        upload_json(&state, &bot_id, kind, &target_openid, body).await?,
    ))
}

async fn group_upload_file(
    State(state): State<Arc<AppState>>,
    Path(group_openid): Path<String>,
    BotCtx(bot): BotCtx,
    req: Request,
) -> MqsResult<Json<Value>> {
    handle_upload(
        state,
        bot["id"].as_str().unwrap_or("").to_string(),
        "group",
        group_openid,
        req,
    )
    .await
}

async fn c2c_upload_file(
    State(state): State<Arc<AppState>>,
    Path(openid): Path<String>,
    BotCtx(bot): BotCtx,
    req: Request,
) -> MqsResult<Json<Value>> {
    handle_upload(
        state,
        bot["id"].as_str().unwrap_or("").to_string(),
        "user",
        openid,
        req,
    )
    .await
}

async fn prepare_upload(
    state: &Arc<AppState>,
    bot_id: &str,
    kind: &str,
    target_openid: &str,
    body: Value,
) -> MqsResult<Value> {
    let body = body.as_object().cloned().unwrap_or_default();
    let (upload_id, block_size, count) =
        state
            .world
            .prepare_upload(bot_id, kind, target_openid, &body)?;
    let base = state.config.base_url();
    let parts: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "index": index,
                "presigned_url": format!("{base}/v2/uploads/{upload_id}/parts/{index}"),
            })
        })
        .collect();
    Ok(json!({
        "upload_id": upload_id,
        "block_size": block_size,
        "parts": parts,
        "upload_config": {"concurrency": 1, "retry_timeout": 0, "retry_delay": 0},
    }))
}

async fn group_upload_prepare(
    State(state): State<Arc<AppState>>,
    Path(group_openid): Path<String>,
    BotCtx(bot): BotCtx,
    QqJson(body): QqJson<Value>,
) -> MqsResult<Json<Value>> {
    Ok(Json(
        prepare_upload(
            &state,
            bot["id"].as_str().unwrap_or(""),
            "group",
            &group_openid,
            body,
        )
        .await?,
    ))
}

async fn c2c_upload_prepare(
    State(state): State<Arc<AppState>>,
    Path(openid): Path<String>,
    BotCtx(bot): BotCtx,
    QqJson(body): QqJson<Value>,
) -> MqsResult<Json<Value>> {
    Ok(Json(
        prepare_upload(
            &state,
            bot["id"].as_str().unwrap_or(""),
            "user",
            &openid,
            body,
        )
        .await?,
    ))
}

async fn put_upload_part(
    State(state): State<Arc<AppState>>,
    Path((upload_id, index)): Path<(String, usize)>,
    body: axum::body::Bytes,
) -> MqsResult<StatusCode> {
    state
        .world
        .put_upload_part(&upload_id, index, body.to_vec())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn finish_part(state: &Arc<AppState>, body: Value) -> MqsResult<StatusCode> {
    state.world.finish_upload_part(
        body.get("upload_id").and_then(|v| v.as_str()).unwrap_or(""),
        body.get("part_index").and_then(|v| v.as_i64()).unwrap_or(0) as usize,
        body.get("block_size").and_then(|v| v.as_i64()).unwrap_or(0),
        body.get("md5").and_then(|v| v.as_str()).unwrap_or(""),
    )?;
    Ok(StatusCode::NO_CONTENT)
}

async fn group_upload_part_finish(
    State(state): State<Arc<AppState>>,
    BotCtx(_bot): BotCtx,
    QqJson(body): QqJson<Value>,
) -> MqsResult<StatusCode> {
    finish_part(&state, body).await
}

async fn c2c_upload_part_finish(
    State(state): State<Arc<AppState>>,
    BotCtx(_bot): BotCtx,
    QqJson(body): QqJson<Value>,
) -> MqsResult<StatusCode> {
    finish_part(&state, body).await
}

async fn download_media(
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
    let resolved = path.canonicalize().unwrap_or(path.clone());
    let root = state
        .world
        .paths
        .media()
        .canonicalize()
        .unwrap_or_else(|_| state.world.paths.media());
    if !resolved.starts_with(&root) || !resolved.exists() {
        return Err(invalid_param("media not found"));
    }
    let data = std::fs::read(&resolved)?;
    let mime = crate::db::s(&media, "mime");
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", &hash[..12]),
            ),
        ],
        data,
    )
        .into_response())
}

async fn avatar(
    Path(key): Path<String>,
    Query(query): Query<Map<String, Value>>,
) -> MqsResult<Response> {
    let key = key.trim_end_matches(".svg").to_string();
    if key.is_empty()
        || key.len() > 64
        || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(invalid_param("invalid avatar key"));
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
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/svg+xml".to_string()),
            (header::CACHE_CONTROL, "public, max-age=3600".to_string()),
        ],
        avatar_svg(&key, &label, &kind),
    )
        .into_response())
}

// ------------------------------------------------------------------ group management
async fn group_entity(world: &World, bot_id: &str, group_openid: &str) -> MqsResult<String> {
    let mapping = world.resolve_openid(bot_id, group_openid)?;
    if crate::db::s(&mapping, "kind") != "group" {
        return Err(invalid_param("openid is not a group openid"));
    }
    Ok(crate::db::s(&mapping, "entity_id"))
}

async fn group_info(
    State(state): State<Arc<AppState>>,
    Path(group_openid): Path<String>,
    WhitelistedBot(bot): WhitelistedBot,
) -> MqsResult<Json<Value>> {
    let bot_id = bot["id"].as_str().unwrap_or("");
    let group_id = group_entity(&state.world, bot_id, &group_openid).await?;
    Ok(Json(state.world.group_info(&group_id, bot_id)?))
}

async fn group_bot_state(
    State(state): State<Arc<AppState>>,
    Path(group_openid): Path<String>,
    WhitelistedBot(bot): WhitelistedBot,
) -> MqsResult<Json<Value>> {
    let bot_id = bot["id"].as_str().unwrap_or("");
    let group_id = group_entity(&state.world, bot_id, &group_openid).await?;
    Ok(Json(state.world.group_bot_state(&group_id, bot_id)?))
}

async fn group_restrict_get(
    State(state): State<Arc<AppState>>,
    Path(group_openid): Path<String>,
    WhitelistedBot(bot): WhitelistedBot,
) -> MqsResult<Json<Value>> {
    let bot_id = bot["id"].as_str().unwrap_or("");
    let group_id = group_entity(&state.world, bot_id, &group_openid).await?;
    Ok(Json(state.world.group_mute_settings(&group_id, bot_id)?))
}

async fn group_restrict_set(
    State(state): State<Arc<AppState>>,
    Path(group_openid): Path<String>,
    WhitelistedBot(bot): WhitelistedBot,
    QqJson(body): QqJson<Value>,
) -> MqsResult<Json<Value>> {
    let bot_id = bot["id"].as_str().unwrap_or("");
    let group_id = group_entity(&state.world, bot_id, &group_openid).await?;
    let members = body
        .get("members")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(Json(
        state.world.set_group_mute(&group_id, bot_id, &members)?,
    ))
}

async fn group_blacklist_get(
    State(state): State<Arc<AppState>>,
    Path(group_openid): Path<String>,
    Query(query): Query<Map<String, Value>>,
    WhitelistedBot(bot): WhitelistedBot,
) -> MqsResult<Json<Value>> {
    let bot_id = bot["id"].as_str().unwrap_or("");
    let group_id = group_entity(&state.world, bot_id, &group_openid).await?;
    let cursor = query.get("cursor").and_then(|v| v.as_str()).unwrap_or("");
    let limit = query.get("limit").and_then(|v| v.as_i64()).unwrap_or(20);
    Ok(Json(
        state
            .world
            .group_blacklist_page(&group_id, bot_id, cursor, limit)?,
    ))
}

async fn group_blacklist_post(
    State(state): State<Arc<AppState>>,
    Path(group_openid): Path<String>,
    WhitelistedBot(bot): WhitelistedBot,
    QqJson(body): QqJson<Value>,
) -> MqsResult<Json<Value>> {
    let bot_id = bot["id"].as_str().unwrap_or("");
    let group_id = group_entity(&state.world, bot_id, &group_openid).await?;
    let op = body.get("op").and_then(|v| v.as_str()).unwrap_or("");
    let openids: Vec<String> = body
        .get("member_openids")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(
        state
            .world
            .update_group_blacklist(&group_id, bot_id, op, &openids)?,
    ))
}

async fn group_batch_remove(
    State(state): State<Arc<AppState>>,
    Path(group_openid): Path<String>,
    WhitelistedBot(bot): WhitelistedBot,
    QqJson(body): QqJson<Value>,
) -> MqsResult<Json<Value>> {
    let bot_id = bot["id"].as_str().unwrap_or("");
    let group_id = group_entity(&state.world, bot_id, &group_openid).await?;
    let openids: Vec<String> = body
        .get("member_openids")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let add_blacklist = body
        .get("add_to_member_blacklist")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(Json(state.world.batch_remove_group_members(
        &group_id,
        bot_id,
        &openids,
        add_blacklist,
    )?))
}

async fn group_join_request_list(
    State(state): State<Arc<AppState>>,
    Path(group_openid): Path<String>,
    Query(query): Query<Map<String, Value>>,
    WhitelistedBot(bot): WhitelistedBot,
) -> MqsResult<Json<Value>> {
    let bot_id = bot["id"].as_str().unwrap_or("");
    let group_id = group_entity(&state.world, bot_id, &group_openid).await?;
    let cursor = query.get("cursor").and_then(|v| v.as_str()).unwrap_or("");
    let limit = query.get("limit").and_then(|v| v.as_i64()).unwrap_or(20);
    Ok(Json(
        state
            .world
            .join_requests_page(&group_id, bot_id, cursor, limit)?,
    ))
}

async fn group_join_request_approve(
    State(state): State<Arc<AppState>>,
    Path((group_openid, member_openid)): Path<(String, String)>,
    WhitelistedBot(bot): WhitelistedBot,
    QqJson(body): QqJson<Value>,
) -> MqsResult<Json<Value>> {
    let bot_id = bot["id"].as_str().unwrap_or("");
    let group_id = group_entity(&state.world, bot_id, &group_openid).await?;
    Ok(Json(
        state.world.approve_join_request(
            &group_id,
            bot_id,
            &member_openid,
            body.get("op").and_then(|v| v.as_str()).unwrap_or(""),
            body.get("join_request_id")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            body.get("reject_reason")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            body.get("add_to_member_blacklist")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        )?,
    ))
}

async fn group_members(
    State(state): State<Arc<AppState>>,
    Path(group_openid): Path<String>,
    Query(query): Query<Map<String, Value>>,
    WhitelistedBot(bot): WhitelistedBot,
) -> MqsResult<Response> {
    let bot_id = bot["id"].as_str().unwrap_or("");
    let group_id = group_entity(&state.world, bot_id, &group_openid).await?;
    let info = state
        .world
        .rate
        .lock()
        .unwrap()
        .check_read(bot_id, "group_members", 60, 60_000)?;
    let cursor = query.get("cursor").and_then(|v| v.as_str()).unwrap_or("");
    let page = state
        .world
        .group_members_page(&group_id, bot_id, cursor, 30)?;
    Ok(attach_rate(Json(page).into_response(), &info))
}

async fn group_member_detail(
    State(state): State<Arc<AppState>>,
    Path((group_openid, member_openid)): Path<(String, String)>,
    WhitelistedBot(bot): WhitelistedBot,
) -> MqsResult<Response> {
    let bot_id = bot["id"].as_str().unwrap_or("");
    let group_id = group_entity(&state.world, bot_id, &group_openid).await?;
    let info = state
        .world
        .rate
        .lock()
        .unwrap()
        .check_read(bot_id, "group_member", 30, 60_000)?;
    let detail = state
        .world
        .group_member_detail(&group_id, bot_id, &member_openid)?;
    Ok(attach_rate(Json(detail).into_response(), &info))
}

// ------------------------------------------------------------------ webhook / ws
async fn webhook_entry(
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    QqJson(body): QqJson<Value>,
) -> MqsResult<Json<Value>> {
    let bot = state
        .world
        .bot_by_id(&bot_id)
        .map_err(|_| message_not_found("bot not found"))?;
    let data = body.get("d").cloned().unwrap_or(body);
    let plain_token = data
        .get("plain_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let event_ts = data.get("event_ts").and_then(|v| v.as_str()).unwrap_or("");
    if !plain_token.is_empty() && !event_ts.is_empty() {
        return Ok(Json(json!({
            "plain_token": plain_token,
            "signature": crate::webhooks::sign_handshake(&crate::db::s(&bot, "secret"), event_ts, plain_token),
        })));
    }
    Ok(Json(json!({"op": 12, "d": {}})))
}

async fn websocket(State(state): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    let gateway = state.world.gateway.get().cloned();
    match gateway {
        Some(gateway) => ws
            .on_upgrade(move |socket| async move {
                gateway.handle(socket).await;
            })
            .into_response(),
        None => json_status(StatusCode::SERVICE_UNAVAILABLE, json!({"code": 10002})),
    }
}

async fn protocol_info() -> Json<Value> {
    Json(json!({"mqs": 1, "api": "QQ Bot API v2 sandbox", "notice": "non-official sandbox"}))
}

async fn openapi() -> Json<Value> {
    Json(openapi_document("QSandbox Simulate API", VERSION))
}

// ------------------------------------------------------------------ app
pub fn sim_router(state: Arc<AppState>) -> Router {
    let router = Router::new()
        .route("/app/getAppAccessToken", post(get_app_access_token))
        .route("/gateway", get(gateway_url))
        .route("/gateway/bot", get(gateway_bot))
        .route("/users/@me", get(bot_me))
        .route(
            "/v2/groups/{group_openid}/messages",
            post(send_group_message),
        )
        .route(
            "/v2/groups/{group_openid}/messages/{message_id}",
            delete(delete_group_message),
        )
        .route("/v2/users/{openid}/messages", post(send_c2c_message))
        .route(
            "/v2/users/{openid}/messages/{message_id}",
            delete(delete_c2c_message),
        )
        .route("/v2/interactions/{interaction_id}", put(ack_interaction))
        .route(
            "/v2/groups/{group_openid}/upload_prepare",
            post(group_upload_prepare),
        )
        .route(
            "/v2/users/{openid}/upload_prepare",
            post(c2c_upload_prepare),
        )
        .route(
            "/v2/uploads/{upload_id}/parts/{index}",
            put(put_upload_part),
        )
        .route(
            "/v2/groups/{group_openid}/upload_part_finish",
            post(group_upload_part_finish),
        )
        .route(
            "/v2/users/{openid}/upload_part_finish",
            post(c2c_upload_part_finish),
        )
        .route("/v2/groups/{group_openid}/files", post(group_upload_file))
        .route("/v2/users/{openid}/files", post(c2c_upload_file))
        .route("/v2/groups/{group_openid}/info", get(group_info))
        .route("/v2/groups/{group_openid}/bot_state", get(group_bot_state))
        .route(
            "/v2/groups/{group_openid}/restrict_chat_setting",
            get(group_restrict_get).post(group_restrict_set),
        )
        .route(
            "/v2/groups/{group_openid}/member_blacklist",
            get(group_blacklist_get).post(group_blacklist_post),
        )
        .route(
            "/v2/groups/{group_openid}/batch_remove_members",
            post(group_batch_remove),
        )
        .route(
            "/v2/groups/{group_openid}/join_request_list",
            get(group_join_request_list),
        )
        .route(
            "/v2/groups/{group_openid}/approval_join_request/{member_openid}",
            post(group_join_request_approve),
        )
        .route("/v2/groups/{group_openid}/members", get(group_members))
        .route(
            "/v2/groups/{group_openid}/members/{member_openid}",
            get(group_member_detail),
        )
        .route("/webhook/{bot_id}", post(webhook_entry))
        .route("/media/{hash}", get(download_media))
        .route("/avatars/{key}", get(avatar))
        .route("/websocket", get(websocket))
        .route("/websocket/", get(websocket))
        .route("/protocol", get(protocol_info))
        .route("/openapi.json", get(openapi))
        .with_state(state.clone());

    Router::new()
        .merge(router)
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            sim_log_mw,
        ))
        .layer(axum::middleware::from_fn(trace_mw))
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
}

/// Route table used by the contract test (must match `contract::REGISTRY`).
pub const ROUTE_TABLE: &[(&str, &str)] = &[
    ("POST", "/app/getAppAccessToken"),
    ("GET", "/gateway"),
    ("GET", "/gateway/bot"),
    ("GET", "/users/@me"),
    ("POST", "/v2/groups/{group_openid}/messages"),
    ("DELETE", "/v2/groups/{group_openid}/messages/{message_id}"),
    ("POST", "/v2/users/{openid}/messages"),
    ("DELETE", "/v2/users/{openid}/messages/{message_id}"),
    ("PUT", "/v2/interactions/{interaction_id}"),
    ("POST", "/v2/groups/{group_openid}/upload_prepare"),
    ("POST", "/v2/users/{openid}/upload_prepare"),
    ("PUT", "/v2/uploads/{upload_id}/parts/{index}"),
    ("POST", "/v2/groups/{group_openid}/upload_part_finish"),
    ("POST", "/v2/users/{openid}/upload_part_finish"),
    ("POST", "/v2/groups/{group_openid}/files"),
    ("POST", "/v2/users/{openid}/files"),
    ("GET", "/v2/groups/{group_openid}/info"),
    ("GET", "/v2/groups/{group_openid}/bot_state"),
    ("GET", "/v2/groups/{group_openid}/restrict_chat_setting"),
    ("POST", "/v2/groups/{group_openid}/restrict_chat_setting"),
    ("GET", "/v2/groups/{group_openid}/member_blacklist"),
    ("POST", "/v2/groups/{group_openid}/member_blacklist"),
    ("POST", "/v2/groups/{group_openid}/batch_remove_members"),
    ("GET", "/v2/groups/{group_openid}/join_request_list"),
    (
        "POST",
        "/v2/groups/{group_openid}/approval_join_request/{member_openid}",
    ),
    ("GET", "/v2/groups/{group_openid}/members"),
    ("GET", "/v2/groups/{group_openid}/members/{member_openid}"),
    ("POST", "/webhook/{bot_id}"),
    ("GET", "/media/{hash}"),
];
