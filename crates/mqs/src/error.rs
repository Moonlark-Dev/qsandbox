//! QQ-shaped error envelope, error catalog and rate-limit exceptions.

use std::fmt::Display;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

tokio::task_local! {
    /// Per-request trace id (the Rust equivalent of the Python contextvar).
    pub static TRACE_ID: String;
}

pub fn new_trace_id() -> String {
    crate::ids::short_token(16).to_lowercase()
}

pub fn current_trace_id() -> String {
    TRACE_ID
        .try_with(|v| v.clone())
        .unwrap_or_else(|_| new_trace_id())
}

/// code -> (http status, default message)
pub fn catalog(code: i64) -> (u16, &'static str) {
    match code {
        10001 => (400, "invalid parameter"),
        10002 => (500, "internal error"),
        11241 => (401, "invalid token"),
        11242 => (401, "token expired"),
        11243 => (401, "invalid app id"),
        11244 => (401, "invalid app secret"),
        11253 => (403, "no interface access permission"),
        40034 => (404, "message not found or msg_id invalid"),
        40040 => (403, "no permission to perform this action"),
        40054 => (400, "duplicate reply: msg_id + msg_seq already used"),
        40055 => (429, "active message quota exhausted"),
        40056 => (403, "bot is not online on any gateway"),
        40057 => (400, "passive reply window expired or reply count exhausted"),
        40063 => (400, "captcha / message audit required"),
        42900 => (429, "rate limit exceeded"),
        50001 => (500, "upstream service error"),
        404 => (404, "endpoint is not exposed by this sandbox"),
        _ => (500, "internal error"),
    }
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub code: i64,
    pub status: u16,
    pub message: String,
    pub hint: Option<String>,
    pub retry_after: Option<f64>,
    pub limit: i64,
    pub remaining: i64,
    /// Server-side detail for logs/debugging; never serialized into the response.
    pub detail: Option<String>,
}

impl Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        let (status, default) = catalog(code);
        let message = message.into();
        Self {
            code,
            status,
            message: if message.is_empty() {
                default.to_string()
            } else {
                message
            },
            hint: None,
            retry_after: None,
            limit: 0,
            remaining: 0,
            detail: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn body(&self) -> serde_json::Value {
        // QQ is inconsistent across the docs: the access-token endpoint documents
        // `code`, the message APIs document `err_code`. Emit both so an SDK that
        // looks for either field keeps working.
        let mut body = serde_json::json!({
            "code": self.code,
            "err_code": self.code,
            "message": self.message,
            "trace_id": current_trace_id(),
        });
        if let Some(hint) = &self.hint {
            body["hint"] = serde_json::Value::String(hint.clone());
        }
        if let Some(retry_after) = self.retry_after {
            body["retry_after"] = serde_json::json!((retry_after * 1000.0).round() / 1000.0);
        }
        body
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            axum::Json(self.body()),
        )
            .into_response();
        let headers = response.headers_mut();
        if let Ok(value) = HeaderValue::from_str(&current_trace_id()) {
            headers.insert("x-tps-trace-id", value);
        }
        if self.code == 42900 {
            if let Some(retry) = self.retry_after {
                if let Ok(value) = HeaderValue::from_str(&format!("{retry:.2}")) {
                    headers.insert(header::RETRY_AFTER, value);
                }
                headers.insert("x-ratelimit-limit", HeaderValue::from(self.limit));
                headers.insert(
                    "x-ratelimit-remaining",
                    HeaderValue::from(self.remaining.max(0)),
                );
                if let Ok(value) = HeaderValue::from_str(&format!("{retry:.2}")) {
                    headers.insert("x-ratelimit-reset", value);
                }
            }
        }
        response
    }
}

pub type MqsResult<T> = Result<T, ApiError>;

pub fn invalid_param(message: impl Into<String>) -> ApiError {
    ApiError::new(10001, message)
}

pub fn internal(err: impl Display) -> ApiError {
    let detail = err.to_string();
    tracing::error!("internal error: {detail}");
    let mut error = ApiError::new(10002, "internal error");
    error.detail = Some(detail);
    error
}

pub fn invalid_token(message: impl Into<String>) -> ApiError {
    ApiError::new(11241, message)
}

pub fn token_expired(message: impl Into<String>) -> ApiError {
    ApiError::new(11242, message)
}

pub fn invalid_app_id(message: impl Into<String>) -> ApiError {
    ApiError::new(11243, message)
}

pub fn invalid_secret(message: impl Into<String>) -> ApiError {
    ApiError::new(11244, message)
}

pub fn no_permission(message: impl Into<String>) -> ApiError {
    ApiError::new(40040, message)
}

pub fn message_not_found(message: impl Into<String>) -> ApiError {
    ApiError::new(40034, message)
}

pub fn duplicate_reply(message: impl Into<String>) -> ApiError {
    ApiError::new(40054, message)
}

pub fn quota_exhausted(message: impl Into<String>) -> ApiError {
    ApiError::new(40055, message)
}

pub fn bot_offline(message: impl Into<String>) -> ApiError {
    ApiError::new(40056, message)
}

pub fn passive_window_expired(message: impl Into<String>) -> ApiError {
    ApiError::new(40057, message)
}

pub fn no_interface_access() -> ApiError {
    ApiError::new(11253, "应用无接口访问权限").with_hint(
        "该接口仅白名单机器人可用，请联系平台运营申请权限；或在 QSandbox 里把 Bot 标记为白名单",
    )
}

pub fn endpoint_not_exposed() -> ApiError {
    ApiError::new(404, "this endpoint is not exposed by the sandbox")
}

pub fn rate_limited(retry_after: f64, limit: i64, remaining: i64, bucket: &str) -> ApiError {
    let mut err = ApiError::new(
        42900,
        format!("rate limit exceeded for {bucket}, retry after {retry_after:.2}s"),
    );
    err.retry_after = Some(retry_after);
    err.limit = limit;
    err.remaining = remaining;
    err
}

impl From<rusqlite::Error> for ApiError {
    fn from(err: rusqlite::Error) -> Self {
        internal(format!("sqlite: {err}"))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        internal(format!("io: {err}"))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(err: serde_json::Error) -> Self {
        internal(format!("json: {err}"))
    }
}
