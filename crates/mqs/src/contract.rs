//! The contract registry: one row per simulated QQ endpoint.
//!
//! This is the single source of truth for coverage, docs and the contract test.
//! The guild/channel family (L2 predecessor) is intentionally not part of the
//! product any more — the sandbox targets group + C2C messaging, group
//! management, media and the gateway.

use serde_json::{json, Value};

pub const DOC_DATE: &str = "2026-11-09";

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub key: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub summary: &'static str,
    pub auth: &'static str,
    pub scopes: &'static [&'static str],
    pub errors: &'static [i64],
    pub coverage: &'static str,
    pub layer: &'static str,
    pub tag: &'static str,
    pub notes: &'static str,
}

macro_rules! ep {
    (
        $key:literal, $method:literal, $path:literal, $summary:literal,
        $auth:literal, $layer:literal, $tag:literal,
        [$($scope:literal),*], [$($err:literal),*], $cov:literal, $notes:literal
    ) => {
        Endpoint {
            key: $key,
            method: $method,
            path: $path,
            summary: $summary,
            auth: $auth,
            scopes: &[$($scope),*],
            errors: &[$($err),*],
            coverage: $cov,
            layer: $layer,
            tag: $tag,
            notes: $notes,
        }
    };
}

pub static REGISTRY: &[Endpoint] = &[
    ep!(
        "app.access_token",
        "POST",
        "/app/getAppAccessToken",
        "获取 App Access Token（7200s）",
        "app",
        "L0",
        "auth",
        [],
        [11243, 11244],
        "full",
        ""
    ),
    ep!(
        "gateway.url",
        "GET",
        "/gateway",
        "获取网关 WebSocket 地址",
        "bot",
        "L0",
        "gateway",
        [],
        [11241, 11242, 11243],
        "full",
        ""
    ),
    ep!(
        "gateway.bot",
        "GET",
        "/gateway/bot",
        "获取网关地址与分片建议",
        "bot",
        "L0",
        "gateway",
        [],
        [11241, 11242],
        "full",
        ""
    ),
    ep!(
        "bot.me",
        "GET",
        "/users/@me",
        "获取机器人自身信息",
        "bot",
        "L0",
        "bot",
        [],
        [],
        "full",
        ""
    ),
    ep!(
        "group.send_message",
        "POST",
        "/v2/groups/{group_openid}/messages",
        "发送群聊消息（被动 / 主动）",
        "bot",
        "L0",
        "message",
        ["group_message"],
        [10001, 40034, 40040, 40054, 40055, 40056, 40057],
        "full",
        ""
    ),
    ep!(
        "c2c.send_message",
        "POST",
        "/v2/users/{openid}/messages",
        "发送单聊消息（被动 / 主动）",
        "bot",
        "L0",
        "message",
        ["c2c_message"],
        [10001, 40034, 40040, 40054, 40055, 40056, 40057],
        "full",
        ""
    ),
    ep!(
        "group.delete_message",
        "DELETE",
        "/v2/groups/{group_openid}/messages/{message_id}",
        "撤回群聊消息",
        "bot",
        "L1",
        "message",
        [],
        [40034, 40040],
        "partial",
        "MQS 只标记撤回位，不投递 PUBLIC_MESSAGE_DELETE 事件"
    ),
    ep!(
        "c2c.delete_message",
        "DELETE",
        "/v2/users/{openid}/messages/{message_id}",
        "撤回单聊消息",
        "bot",
        "L1",
        "message",
        [],
        [40034, 40040],
        "partial",
        ""
    ),
    ep!(
        "group.upload_prepare",
        "POST",
        "/v2/groups/{group_openid}/upload_prepare",
        "群聊富媒体分片上传：预上传",
        "bot",
        "L1",
        "media",
        ["group_message", "media"],
        [10001, 40040],
        "full",
        ""
    ),
    ep!(
        "c2c.upload_prepare",
        "POST",
        "/v2/users/{openid}/upload_prepare",
        "单聊富媒体分片上传：预上传",
        "bot",
        "L1",
        "media",
        ["c2c_message", "media"],
        [10001, 40040],
        "full",
        ""
    ),
    ep!(
        "upload.put_part",
        "PUT",
        "/v2/uploads/{upload_id}/parts/{index}",
        "富媒体分片上传：上传分片（预签名 URL）",
        "none",
        "L1",
        "media",
        [],
        [],
        "full",
        "MQS 用一次性 upload_id 鉴权，官方为对象存储预签名 URL"
    ),
    ep!(
        "group.upload_part_finish",
        "POST",
        "/v2/groups/{group_openid}/upload_part_finish",
        "群聊富媒体分片上传：完成分片",
        "bot",
        "L1",
        "media",
        ["group_message", "media"],
        [],
        "full",
        ""
    ),
    ep!(
        "c2c.upload_part_finish",
        "POST",
        "/v2/users/{openid}/upload_part_finish",
        "单聊富媒体分片上传：完成分片",
        "bot",
        "L1",
        "media",
        ["c2c_message", "media"],
        [],
        "full",
        ""
    ),
    ep!(
        "group.upload_file",
        "POST",
        "/v2/groups/{group_openid}/files",
        "上传群聊富媒体文件",
        "bot",
        "L1",
        "media",
        ["group_message", "media"],
        [10001, 40040],
        "full",
        ""
    ),
    ep!(
        "c2c.upload_file",
        "POST",
        "/v2/users/{openid}/files",
        "上传单聊富媒体文件",
        "bot",
        "L1",
        "media",
        ["c2c_message", "media"],
        [10001, 40040],
        "full",
        ""
    ),
    ep!(
        "interaction.ack",
        "PUT",
        "/v2/interactions/{interaction_id}",
        "确认按钮回调结果（code 0 成功）",
        "bot",
        "L1",
        "message",
        [],
        [40034],
        "full",
        ""
    ),
    ep!(
        "media.download",
        "GET",
        "/media/{hash}",
        "沙箱媒体下载（内容哈希寻址）",
        "none",
        "L1",
        "media",
        [],
        [],
        "full",
        "MQS 自建入口，非 QQ 接口"
    ),
    ep!(
        "group.info",
        "GET",
        "/v2/groups/{group_openid}/info",
        "获取群基本信息",
        "bot",
        "L2",
        "group",
        ["member"],
        [11253, 40040],
        "full",
        "内邀接口，需白名单；30 QPM"
    ),
    ep!(
        "group.bot_state",
        "GET",
        "/v2/groups/{group_openid}/bot_state",
        "获取机器人群内状态（含主动消息开关）",
        "bot",
        "L2",
        "group",
        ["member"],
        [11253, 40040],
        "full",
        "内邀接口，需白名单；30 QPM"
    ),
    ep!(
        "group.mute.get",
        "GET",
        "/v2/groups/{group_openid}/restrict_chat_setting",
        "查询群禁言状态",
        "bot",
        "L2",
        "group",
        ["member"],
        [11253, 40040],
        "full",
        "需要群管理员身份；30 QPM"
    ),
    ep!(
        "group.mute.set",
        "POST",
        "/v2/groups/{group_openid}/restrict_chat_setting",
        "设置群成员禁言（add/update/del，最长 30 天）",
        "bot",
        "L2",
        "group",
        ["member"],
        [11253, 40040],
        "full",
        "单次最多 20 人；60 QPM"
    ),
    ep!(
        "group.blacklist.list",
        "GET",
        "/v2/groups/{group_openid}/member_blacklist",
        "查询群黑名单",
        "bot",
        "L2",
        "group",
        ["member"],
        [11253, 40040],
        "full",
        "内邀接口；30 QPM"
    ),
    ep!(
        "group.blacklist.set",
        "POST",
        "/v2/groups/{group_openid}/member_blacklist",
        "群黑名单操作（add/del，单次最多 20 人）",
        "bot",
        "L2",
        "group",
        ["member"],
        [11253, 40040],
        "full",
        "目标仍在群内时无法拉黑；60 QPM"
    ),
    ep!(
        "group.batch_remove",
        "POST",
        "/v2/groups/{group_openid}/batch_remove_members",
        "群成员批量移除（可同时拉黑）",
        "bot",
        "L2",
        "group",
        ["member"],
        [11253, 40040],
        "full",
        "单次最多 20 人；30 QPM"
    ),
    ep!(
        "group.join_request.list",
        "GET",
        "/v2/groups/{group_openid}/join_request_list",
        "拉取入群申请列表",
        "bot",
        "L2",
        "group",
        ["member"],
        [11253, 40040],
        "full",
        "需要群管理员身份；30 QPM"
    ),
    ep!(
        "group.join_request.approve",
        "POST",
        "/v2/groups/{group_openid}/approval_join_request/{member_openid}",
        "审批入群申请（approve/decline）",
        "bot",
        "L2",
        "group",
        ["member"],
        [11253, 40040],
        "full",
        "60 QPM"
    ),
    ep!(
        "group.members.list",
        "GET",
        "/v2/groups/{group_openid}/members",
        "获取群成员列表（内邀 / 白名单接口）",
        "bot",
        "L3",
        "member",
        ["member"],
        [11253, 40040],
        "full",
        "官方 60 QPM；需白名单，错误码 11253"
    ),
    ep!(
        "group.members.get",
        "GET",
        "/v2/groups/{group_openid}/members/{member_openid}",
        "获取群成员信息（内邀 / 白名单接口）",
        "bot",
        "L3",
        "member",
        ["member"],
        [11253, 40040],
        "full",
        "官方 30 QPM；需白名单，错误码 11253"
    ),
    ep!(
        "webhook.verify",
        "POST",
        "/webhook/{bot_id}",
        "Webhook 回调入口（含 URL 校验握手）",
        "none",
        "L3",
        "webhook",
        [],
        [],
        "partial",
        "MQS 自建入口，用于模拟 webhook 部署形态"
    ),
];

pub fn sim_endpoints() -> &'static [Endpoint] {
    REGISTRY
}

pub fn summary() -> Value {
    let mut levels: std::collections::BTreeMap<&str, i64> = Default::default();
    let mut layers: std::collections::BTreeMap<&str, std::collections::BTreeMap<&str, i64>> =
        Default::default();
    for endpoint in sim_endpoints() {
        *levels.entry(endpoint.coverage).or_default() += 1;
        *layers
            .entry(endpoint.layer)
            .or_default()
            .entry(endpoint.coverage)
            .or_default() += 1;
    }
    let implemented =
        levels.get("full").copied().unwrap_or(0) + levels.get("partial").copied().unwrap_or(0);
    let ascii = format!(
        "{} endpoints — {} implemented, {} stubs, {} n/a",
        sim_endpoints().len(),
        implemented,
        levels.get("stub").copied().unwrap_or(0),
        levels.get("n/a").copied().unwrap_or(0)
    );
    json!({
        "total": sim_endpoints().len(),
        "by_level": levels,
        "by_layer": layers,
        "ascii": ascii,
    })
}

pub fn coverage_markdown() -> String {
    let data = summary();
    let mut lines = vec![
        "# QSandbox 接口覆盖率".to_string(),
        String::new(),
        format!("> 自动生成 · 文档快照 {DOC_DATE} · 请勿手改"),
        String::new(),
        format!("**{}**", data["ascii"].as_str().unwrap_or("")),
        String::new(),
        "| 层级 | 总数 | full | partial | stub | n/a |".to_string(),
        "|---|---|---|---|---|---|".to_string(),
    ];
    if let Some(layers) = data["by_layer"].as_object() {
        for (layer, counts) in layers {
            let total: i64 = counts
                .as_object()
                .map(|m| m.values().filter_map(|v| v.as_i64()).sum())
                .unwrap_or(0);
            lines.push(format!(
                "| {} | {} | {} | {} | {} | {} |",
                layer,
                total,
                counts.get("full").and_then(|v| v.as_i64()).unwrap_or(0),
                counts.get("partial").and_then(|v| v.as_i64()).unwrap_or(0),
                counts.get("stub").and_then(|v| v.as_i64()).unwrap_or(0),
                counts.get("n/a").and_then(|v| v.as_i64()).unwrap_or(0),
            ));
        }
    }
    lines.extend([
        String::new(),
        "## 明细".to_string(),
        String::new(),
        "| 方法 | 路径 | 覆盖 | 鉴权 | 分层 | 说明 |".to_string(),
        "|---|---|---|---|---|---|".to_string(),
    ]);
    let mut endpoints: Vec<&Endpoint> = sim_endpoints().iter().collect();
    endpoints.sort_by(|a, b| (a.layer, a.path, a.method).cmp(&(b.layer, b.path, b.method)));
    for endpoint in endpoints {
        let auth = match endpoint.auth {
            "bot" => "Bot Token",
            "app" => "appid+secret",
            _ => "无",
        };
        lines.push(format!(
            "| `{}` | `{}` | {} | {} | {} | {} |",
            endpoint.method,
            endpoint.path,
            endpoint.coverage,
            auth,
            endpoint.layer,
            endpoint.summary
        ));
    }
    lines.push(String::new());
    lines.push("覆盖等级：`full` 行为保真 · `partial` 字段或分支不全 · `stub` 仅返回结构 · `n/a` 明确不模拟。".to_string());
    lines.push(String::new());
    lines.join("\n")
}

pub fn routes_table() -> Vec<Value> {
    sim_endpoints()
        .iter()
        .map(|endpoint| {
            json!({
                "key": endpoint.key,
                "method": endpoint.method,
                "path": endpoint.path,
                "coverage": endpoint.coverage,
                "auth": endpoint.auth,
                "layer": endpoint.layer,
                "summary": endpoint.summary,
                "errors": endpoint.errors,
                "doc_date": DOC_DATE,
            })
        })
        .collect()
}

/// TypeScript view of the request/response models the GUI talks to.
pub fn typescript() -> String {
    let mut out = String::from(
        "// AUTO-GENERATED by `mqs contract export` — do not edit by hand.\n\
         // Source: serde models in crates/mqs/src/models.rs\n\n",
    );
    out.push_str(
        r#"export interface AppAccessTokenRequest {
  appId: string | number;
  clientSecret: string;
}

export interface AppAccessTokenResponse {
  access_token: string;
  expires_in: string;
}

export interface ErrorPayload {
  code: number;
  message: string;
  trace_id: string;
  hint?: string;
}

export interface MediaInfo {
  file_uuid?: string | null;
  file_info?: string | null;
  ttl?: number;
}

export interface MessageReference {
  message_id: string;
  ignore_get_message_error?: boolean;
}

export interface MessageSendRequest {
  content?: string | null;
  msg_type?: number;
  markdown?: Record<string, any> | null;
  keyboard?: Record<string, any> | null;
  ark?: Record<string, any> | null;
  embed?: Record<string, any> | null;
  media?: MediaInfo | null;
  message_reference?: MessageReference | null;
  event_id?: string | null;
  msg_id?: string | null;
  msg_seq?: number | null;
  timestamp?: number | null;
}

export interface MessageResponse {
  id: string;
  timestamp: string;
  seq?: number | null;
}

export interface MediaUploadRequest {
  file_type?: number;
  url?: string | null;
  file_data?: string | null;
  srv_send_msg?: boolean;
}

export interface MediaUploadResponse {
  file_uuid: string;
  file_info: string;
  ttl?: number;
}

export interface BotUser {
  id: string;
  username: string;
  bot?: boolean;
}

export interface ReadyData {
  version?: number;
  session_id: string;
  shard?: Array<number>;
  user: BotUser;
}

export interface IdentifyData {
  token: string;
  intents?: number;
  shard?: Array<number>;
}

export interface ResumeData {
  token: string;
  session_id: string;
  seq?: number;
}

export interface GatewayInfo {
  url: string;
  shards?: number;
  session_start_limit?: Record<string, number>;
}

export interface UserPayload {
  id: string;
  member_openid?: string | null;
  user_openid?: string | null;
  union_openid?: string | null;
  username?: string | null;
}

export interface MessagePayload {
  id: string;
  content?: string;
  group_openid?: string | null;
  group_id?: string | null;
  author: UserPayload;
  timestamp: string;
  msg_seq?: number | null;
  mentions?: Array<Record<string, any>>;
  attachments?: Array<Record<string, any>>;
  message_reference?: Record<string, any> | null;
}

export interface GatewayFrame {
  op: number;
  s?: number | null;
  t?: string | null;
  id?: string | null;
  d?: any;
}

export interface WebhookAck {
  plain_token: string;
  signature: string;
}

export interface UserCreate {
  nickname?: string | null;
  avatar?: string | null;
  gender?: number;
  age?: number;
  city?: string;
  actor_type?: "human" | "npc" | "bot";
  behavior?: "silent" | "normal" | "chatty" | "spammer";
  seed?: number | null;
  random?: boolean;
}

export interface UserPatch {
  nickname?: string | null;
  avatar?: string | null;
  gender?: number | null;
  age?: number | null;
  city?: string | null;
  behavior?: "silent" | "normal" | "chatty" | "spammer" | null;
  actor_type?: "human" | "npc" | "bot" | null;
}

export interface GroupCreate {
  name?: string | null;
  owner_id?: string | null;
  member_ids?: Array<string>;
  bot_ids?: Array<string>;
  join_policy?: string;
  random_members?: number;
}

export interface GroupPatch {
  name?: string | null;
  owner_id?: string | null;
  join_policy?: string | null;
  finger_memo?: string | null;
  class_text?: string | null;
  tags?: Array<string> | null;
}

export interface MemberAdd {
  user_id: string;
  role?: "owner" | "admin" | "member";
}

export interface MemberPatch {
  role?: "owner" | "admin" | "member" | null;
  mute_seconds?: number | null;
}

export interface BotCreate {
  app_id?: string | null;
  secret?: string | null;
  name?: string | null;
  avatar?: string | null;
  scopes?: Array<string>;
  delivery?: "ws" | "webhook";
  random?: boolean;
  whitelisted?: boolean;
}

export interface BotPatch {
  name?: string | null;
  avatar?: string | null;
  scopes?: Array<string> | null;
  delivery?: "ws" | "webhook" | null;
  whitelisted?: boolean | null;
  allow_proactive_msg?: boolean | null;
}

export interface GroupBotPatch {
  allow_proactive_msg?: boolean | null;
  recv_msg_setting?: "all" | "only_mention" | "mention_and_context" | null;
}

export interface ChatSend {
  as_user: string;
  conversation_id?: string | null;
  to?: string | null;
  content?: string;
  msg_type?: number;
  mentions?: Array<string>;
  at_bots?: Array<string> | null;
  reply_to?: string | null;
  attachments?: Array<Record<string, any>>;
}

export interface ClockAdvance {
  duration?: string | null;
  ms?: number | null;
}

export interface ClockSet {
  iso?: string | null;
  ms?: number | null;
  mode?: "real" | "virtual" | null;
}

export interface SettingsPatch {
  clock_mode?: "real" | "virtual" | null;
  seed?: number | null;
  rate_mode?: string | null;
  require_online?: boolean | null;
  feature_whitelist?: boolean | null;
  feature_expose?: string | null;
  chat_auto_at_bot?: boolean | null;
  active_quota_per_day?: number | null;
  passive_window_c2c_ms?: number | null;
  passive_reply_limit_c2c?: number | null;
  passive_window_group_ms?: number | null;
  passive_reply_limit_group?: number | null;
  theme?: string | null;
  telemetry?: boolean | null;
  autostart?: boolean | null;
  language?: string | null;
  log_level?: string | null;
}

export interface RuleCreate {
  name?: string;
  event_type?: string;
  condition?: Record<string, any>;
  action?: Record<string, any>;
  delay_ms?: number;
  enabled?: boolean;
}

export interface ScenarioRunRequest {
  script?: string | null;
  path?: string | null;
  junit?: boolean;
}

export interface RandomUsersRequest {
  count?: number;
  seed?: number | null;
  behavior?: "silent" | "normal" | "chatty" | "spammer" | null;
  actor_type?: "human" | "npc";
  group_id?: string | null;
}

export interface KeyboardClick {
  user_id: string;
  index?: number;
}

export interface JoinRequestCreate {
  user_id: string;
  verify_message?: string;
  source?: "self_apply" | "invited";
  invited_by?: string;
}

export interface JoinRequestDecision {
  op: "approve" | "decline";
  join_request_id?: string;
  reject_reason?: string;
  add_to_blacklist?: boolean;
}
"#,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_summary_is_consistent() {
        let data = summary();
        assert_eq!(data["total"], sim_endpoints().len());
        let sum: i64 = data["by_level"]
            .as_object()
            .unwrap()
            .values()
            .filter_map(|v| v.as_i64())
            .sum();
        assert_eq!(sum as usize, sim_endpoints().len());
        assert!(data["ascii"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{} endpoints", sim_endpoints().len())));
    }

    #[test]
    fn coverage_document_mentions_every_path() {
        let markdown = coverage_markdown();
        for endpoint in sim_endpoints() {
            assert!(
                markdown.contains(endpoint.path),
                "missing {}",
                endpoint.path
            );
        }
        assert!(markdown.contains("接口覆盖率"));
    }

    #[test]
    fn typescript_generation() {
        let ts = typescript();
        assert!(ts.contains("export interface MessageSendRequest"));
        assert!(ts.contains("export interface GatewayFrame"));
        assert!(ts.contains("export interface ChatSend"));
        assert!(ts.contains("AUTO-GENERATED"));
        assert!(!ts.contains("$defs"));
    }

    #[test]
    fn routes_table_matches_registry() {
        let table = routes_table();
        assert_eq!(table.len(), REGISTRY.len());
        assert!(table.iter().any(|row| row["key"] == "group.send_message"));
        assert!(table.iter().any(|row| row["key"] == "app.access_token"));
    }
}
