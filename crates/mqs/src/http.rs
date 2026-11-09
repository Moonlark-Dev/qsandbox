//! Shared HTTP plumbing: state, QQ-style errors, extractors, middleware.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::{to_bytes, Body};
use axum::extract::{FromRequest, FromRequestParts, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::config::ProcessConfig;
use crate::error::{
    current_trace_id, endpoint_not_exposed, invalid_param, invalid_token, new_trace_id,
    no_interface_access, ApiError, TRACE_ID,
};
use crate::ratelimit::RateInfo;
use crate::world::World;

pub struct AppState {
    pub world: Arc<World>,
    pub config: ProcessConfig,
    pub sessions: Mutex<HashSet<String>>,
    pub nonce: String,
    pub nonce_created: Instant,
    pub ws_tickets: Mutex<HashMap<String, f64>>,
}

impl AppState {
    pub fn new(world: Arc<World>, config: ProcessConfig) -> Arc<Self> {
        Self::new_with_nonce(world, config, crate::ids::short_token(32))
    }

    pub fn new_with_nonce(world: Arc<World>, config: ProcessConfig, nonce: String) -> Arc<Self> {
        Arc::new(Self {
            world,
            config,
            sessions: Mutex::new(HashSet::new()),
            nonce,
            nonce_created: Instant::now(),
            ws_tickets: Mutex::new(HashMap::new()),
        })
    }
}

// ------------------------------------------------------------------ extractors

pub type BotTrace = Arc<Mutex<Option<String>>>;

pub struct BotCtx(pub Value);

fn bearer_token(parts: &axum::http::request::Parts) -> Result<String, ApiError> {
    let header = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let mut pieces = header.splitn(2, char::is_whitespace);
    let scheme = pieces.next().unwrap_or("").to_lowercase();
    let token = pieces.next().unwrap_or("").trim().to_string();
    if (scheme == "qqbot" || scheme == "bearer") && !token.is_empty() {
        return Ok(token);
    }
    if let Some(token) = parts
        .headers
        .get("x-bot-token")
        .and_then(|v| v.to_str().ok())
    {
        if !token.is_empty() {
            return Ok(token.to_string());
        }
    }
    Err(invalid_token("missing Authorization: QQBot <access_token>"))
}

impl FromRequestParts<Arc<AppState>> for BotCtx {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(parts)?;
        let bot = state.world.authenticate_bot(&token)?;
        if let Some(trace) = parts.extensions.get::<BotTrace>() {
            *trace.lock().unwrap() = bot["id"].as_str().map(str::to_string);
        }
        Ok(BotCtx(bot))
    }
}

pub struct WhitelistedBot(pub Value);

impl FromRequestParts<Arc<AppState>> for WhitelistedBot {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let BotCtx(bot) = BotCtx::from_request_parts(parts, state).await?;
        if state.world.setting_str("feature.expose") == "public" {
            return Err(endpoint_not_exposed());
        }
        if state.world.setting_bool("feature.whitelist")
            && !bot["whitelisted"].as_bool().unwrap_or(false)
        {
            return Err(no_interface_access());
        }
        Ok(WhitelistedBot(bot))
    }
}

/// JSON extractor that renders QQ's `{"code":10001,...}` envelope on bad input.
pub struct QqJson<T>(pub T);

impl<T, S> FromRequest<S> for QqJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = to_bytes(req.into_body(), 32 * 1024 * 1024)
            .await
            .map_err(|err| invalid_param(format!("invalid parameter: {err}")))?;
        if bytes.is_empty() {
            return Err(invalid_param("invalid parameter: request body is required"));
        }
        let value = serde_json::from_slice(&bytes).map_err(|err| {
            invalid_param(format!("invalid parameter: {}", simplify_json_error(&err)))
        })?;
        let _ = state;
        Ok(QqJson(value))
    }
}

fn simplify_json_error(err: &serde_json::Error) -> String {
    let text = err.to_string();
    // "missing field `as_user` at line 1 column 2" -> "missing field `as_user`"
    match text.split(" at line ").next() {
        Some(head) => head.to_string(),
        None => text,
    }
}

// ------------------------------------------------------------------ helpers

pub fn attach_rate(mut response: Response, info: &RateInfo) -> Response {
    if info.limit > 0 {
        let headers = response.headers_mut();
        headers.insert("x-ratelimit-limit", HeaderValue::from(info.limit));
        headers.insert(
            "x-ratelimit-remaining",
            HeaderValue::from(info.remaining.max(0)),
        );
        if let Ok(value) = HeaderValue::from_str(&format!("{:.2}", info.reset_after)) {
            headers.insert("x-ratelimit-reset", value);
        }
    }
    response
}

pub fn json_status(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

pub fn openapi_document(title: &str, version: &str) -> Value {
    let mut paths = serde_json::Map::new();
    for endpoint in crate::contract::sim_endpoints() {
        let method = endpoint.method.to_lowercase();
        let entry = paths
            .entry(endpoint.path.to_string())
            .or_insert_with(|| json!({}));
        entry[method] = json!({
            "summary": endpoint.summary,
            "tags": [endpoint.tag],
            "responses": {"200": {"description": "ok"}},
        });
    }
    json!({
        "openapi": "3.0.0",
        "info": {"title": title, "version": version},
        "paths": paths,
    })
}

// ------------------------------------------------------------------ middleware

pub async fn trace_mw(mut req: Request, next: Next) -> Response {
    let incoming = req
        .headers()
        .get("x-tps-trace-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let trace_id = incoming.unwrap_or_else(new_trace_id);
    req.extensions_mut().insert(BotTrace::default());
    let mut response = TRACE_ID
        .scope(trace_id.clone(), async move { next.run(req).await })
        .await;
    if let Ok(value) = HeaderValue::from_str(&trace_id) {
        response
            .headers_mut()
            .entry("x-tps-trace-id")
            .or_insert(value);
    }
    response
}

/// Log every simulated HTTP call as a frame, for the debugger page.
pub async fn sim_log_mw(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if path.starts_with("/media/")
        || path.starts_with("/avatars/")
        || path.starts_with("/docs")
        || path.starts_with("/openapi")
    {
        return next.run(req).await;
    }
    let method = req.method().clone();
    let query = req.uri().query().unwrap_or("").to_string();
    let bot_trace = req.extensions().get::<BotTrace>().cloned();
    let headers: Value = {
        let mut map = serde_json::Map::new();
        for (name, value) in req.headers() {
            let name = name.as_str().to_lowercase();
            if [
                "authorization",
                "content-type",
                "origin",
                "user-agent",
                "x-tps-trace-id",
            ]
            .contains(&name.as_str())
            {
                let text = value.to_str().unwrap_or("");
                map.insert(
                    name.clone(),
                    Value::String(if name == "authorization" {
                        "***".into()
                    } else {
                        text.into()
                    }),
                );
            }
        }
        Value::Object(map)
    };
    let (parts, body) = req.into_parts();
    let body_bytes =
        if [Method::POST, Method::PUT, Method::PATCH, Method::DELETE].contains(&parts.method) {
            to_bytes(body, 64 * 1024 * 1024).await.unwrap_or_default()
        } else {
            bytes::Bytes::new()
        };
    let mut request_body: Value = Value::Null;
    if !body_bytes.is_empty() {
        request_body = serde_json::from_slice(&body_bytes).unwrap_or_else(|_| {
            Value::String(
                String::from_utf8_lossy(&body_bytes)
                    .chars()
                    .take(2000)
                    .collect(),
            )
        });
        if let Some(object) = request_body.as_object_mut() {
            if object.contains_key("clientSecret") {
                object.insert("clientSecret".into(), Value::String("***".into()));
            }
        }
    }
    let request = Request::from_parts(parts, Body::from(body_bytes));
    let started = Instant::now();
    let response = next.run(request).await;
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    let status = response.status().as_u16();
    let rate = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());
    let bot_id = bot_trace.and_then(|trace| trace.lock().unwrap().clone());
    let mut summary = format!("{method} {path} -> {status} ({elapsed:.0}ms");
    if let Some(remaining) = rate {
        summary.push_str(&format!(", rl remaining={remaining}"));
    }
    summary.push(')');
    state.world.log_frame(
        "in",
        "http",
        &summary,
        &json!({
            "method": method.as_str(),
            "path": path,
            "query": query,
            "headers": headers,
            "request": request_body,
            "status": status,
            "duration_ms": (elapsed * 100.0).round() / 100.0,
            "rate_limit": rate.map(|remaining| json!({"remaining": remaining})),
            "trace_id": current_trace_id(),
        }),
        bot_id.as_deref(),
        None,
    );
    response
}

pub fn origin_is_allowed(state: &AppState, origin: &str) -> bool {
    let host = origin
        .split("://")
        .nth(1)
        .unwrap_or(origin)
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string();
    let mut allowed: HashSet<String> = ["127.0.0.1", "localhost", "::1", "0.0.0.0", "::"]
        .iter()
        .map(|v| v.to_string())
        .collect();
    if !state.config.public_host.is_empty() {
        allowed.insert(state.config.public_host.clone());
    }
    if !state.config.dial_host().is_empty() {
        allowed.insert(state.config.dial_host());
    }
    for extra in &state.config.control_origins {
        let host = extra
            .split("://")
            .nth(1)
            .unwrap_or(extra)
            .split(':')
            .next()
            .unwrap_or(extra);
        allowed.insert(host.to_string());
    }
    allowed.contains(&host)
}

pub async fn origin_guard_mw(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    if path.starts_with("/_mqs") || path == "/" {
        if let Some(origin) = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
        {
            if !origin_is_allowed(&state, origin) {
                return json_status(
                    StatusCode::FORBIDDEN,
                    json!({"code": 40040, "message": "origin not allowed", "trace_id": ""}),
                );
            }
        }
    }
    next.run(req).await
}

pub fn control_token(_state: &AppState, req: &Request) -> Option<String> {
    if let Some(token) = req
        .headers()
        .get("x-mqs-token")
        .and_then(|v| v.to_str().ok())
    {
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }
    let auth = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if let Some(rest) = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
    {
        return Some(rest.trim().to_string());
    }
    req.headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookie| {
            cookie.split(';').find_map(|part| {
                let part = part.trim();
                part.strip_prefix("mqs_session=")
                    .map(|value| value.trim().to_string())
            })
        })
}

pub fn is_authorized(state: &AppState, req: &Request) -> bool {
    if let Some(token) = control_token(state, req) {
        if token == state.config.control_token {
            return true;
        }
        if state.sessions.lock().unwrap().contains(&token) {
            return true;
        }
    }
    false
}

pub async fn require_control_mw(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    if !is_authorized(&state, &req) {
        return ApiError::new(11241, "missing or invalid MQS control token").into_response();
    }
    next.run(req).await
}
