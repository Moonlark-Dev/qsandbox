//! Request models for the control API and the simulated QQ surface.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppAccessTokenRequest {
    #[serde(rename = "appId")]
    pub app_id: Value,
    #[serde(rename = "clientSecret")]
    pub client_secret: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MessageSendRequest {
    #[serde(flatten)]
    pub body: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MediaUploadRequest {
    #[serde(flatten)]
    pub body: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct UserCreate {
    pub nickname: Option<String>,
    pub avatar: Option<String>,
    pub gender: Option<i64>,
    pub age: Option<i64>,
    pub city: Option<String>,
    pub actor_type: Option<String>,
    pub behavior: Option<String>,
    pub seed: Option<i64>,
    pub random: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct UserPatch {
    pub nickname: Option<String>,
    pub avatar: Option<String>,
    pub gender: Option<i64>,
    pub age: Option<i64>,
    pub city: Option<String>,
    pub behavior: Option<String>,
    pub actor_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct GroupCreate {
    pub name: Option<String>,
    pub owner_id: Option<String>,
    pub member_ids: Option<Vec<String>>,
    pub bot_ids: Option<Vec<String>>,
    pub join_policy: Option<String>,
    pub random_members: Option<i64>,
    pub seed: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct GroupPatch {
    pub name: Option<String>,
    pub owner_id: Option<String>,
    pub join_policy: Option<String>,
    pub finger_memo: Option<String>,
    pub class_text: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemberAdd {
    pub user_id: String,
    pub role: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MemberPatch {
    pub role: Option<String>,
    pub mute_seconds: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct BotCreate {
    pub app_id: Option<String>,
    pub secret: Option<String>,
    pub name: Option<String>,
    pub avatar: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub delivery: Option<String>,
    pub random: Option<bool>,
    pub whitelisted: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct BotPatch {
    pub name: Option<String>,
    pub avatar: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub delivery: Option<String>,
    pub whitelisted: Option<bool>,
    pub allow_proactive_msg: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct GroupBotPatch {
    pub allow_proactive_msg: Option<bool>,
    pub recv_msg_setting: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatSend {
    pub as_user: String,
    pub conversation_id: Option<String>,
    pub to: Option<String>,
    pub content: Option<String>,
    pub msg_type: Option<i64>,
    pub mentions: Option<Vec<String>>,
    pub at_bots: Option<Vec<String>>,
    pub reply_to: Option<String>,
    pub attachments: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ClockAdvance {
    pub duration: Option<String>,
    pub ms: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ClockSet {
    pub iso: Option<String>,
    pub ms: Option<i64>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SettingsPatch {
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct RuleCreate {
    pub name: Option<String>,
    pub event_type: Option<String>,
    pub condition: Option<Value>,
    pub action: Option<Value>,
    pub delay_ms: Option<i64>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ScenarioRunRequest {
    pub script: Option<String>,
    pub path: Option<String>,
    pub junit: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct RandomUsersRequest {
    pub count: Option<i64>,
    pub seed: Option<i64>,
    pub behavior: Option<String>,
    pub actor_type: Option<String>,
    pub group_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KeyboardClick {
    pub user_id: String,
    pub index: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JoinRequestCreate {
    pub user_id: String,
    pub verify_message: Option<String>,
    pub source: Option<String>,
    pub invited_by: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JoinRequestDecision {
    pub op: String,
    pub join_request_id: Option<String>,
    pub reject_reason: Option<String>,
    pub add_to_blacklist: Option<bool>,
}

/// Convert a model into the `Map` the world layer consumes (nulls are ignored).
pub fn to_map<T: serde::Serialize>(value: &T) -> Map<String, Value> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}
