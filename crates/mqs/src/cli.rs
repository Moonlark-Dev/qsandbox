//! The `mqs` command line interface.
//!
//! Every command is a thin client of the control API — there is no second
//! implementation path.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use crate::clock::{hm, parse_duration};
use crate::config::{read_runtime, resolve_home, ProcessConfig, ProfilePaths, VERSION};

pub const EXIT_USAGE: i32 = 2;
pub const EXIT_CONN: i32 = 3;
pub const EXIT_LOCK: i32 = 4;
pub const EXIT_FAIL: i32 = 5;

#[derive(Parser)]
#[command(name = "mqs", version = VERSION, about = "QSandbox — QQ Bot API v2 sandbox")]
struct Cli {
    #[arg(long, global = true, env = "MQS_PROFILE", default_value = "default")]
    profile: String,
    #[arg(long, global = true, env = "MQS_HOME")]
    home: Option<String>,
    /// 以 JSON 输出（含错误：失败时 stdout 也是单个 JSON 文档）
    #[arg(long, global = true)]
    json: bool,
    /// `bot list`/`bot create` 输出 AppSecret 明文（默认脱敏为 ***）
    #[arg(long, global = true)]
    show_secret: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 启动沙箱（模拟 API + 控制 API + 网关）
    Serve(ServeArgs),
    /// 显示运行状态、接入信息与覆盖率
    Status,
    /// 打印 Bot 的 App Access Token（7200s）
    Token {
        #[arg(long, default_value = "")]
        bot: String,
    },
    /// 虚拟用户
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    /// 群
    Group {
        #[command(subcommand)]
        command: GroupCommand,
    },
    /// Bot
    Bot {
        #[command(subcommand)]
        command: BotCommand,
    },
    /// 以虚拟用户身份发送消息（触发网关事件）
    ///
    /// `--to`/`--conversation` 的会话语法（`--to` 也接受裸形式）：
    /// `group:<group_id>`（群聊）或 `dm:<app_id>:<user_id>`（单聊）。
    ///
    /// 发完想等 Bot 回复，加 `--wait-reply 10s`（Bot 出站消息不会进入事件流）。
    #[command(
        long_about = "以虚拟用户身份发送消息（触发网关事件）\n\n\
            `--to`/`--conversation` 的会话语法（`--to` 也接受裸形式）：\n  \
            group:<group_id>         群聊，例如 group:g_000002\n  \
            dm:<app_id>:<user_id>    单聊，例如 dm:10000:u_000001\n\n\
            发完想等 Bot 回复，加 `--wait-reply 10s`（Bot 出站消息不会进入事件流）。",
        after_help = "示例:\n  \
            mqs send --as-user u_000001 --to group:g_000002 --text \"你好\"\n  \
            mqs send --as-user u_000001 --to dm:10000:u_000001 --text \"你好\" --wait-reply 10s"
    )]
    Send(SendArgs),
    /// 查看会话消息（Bot 的出站回复也在里面）
    Messages {
        #[command(subcommand)]
        command: MessagesCommand,
    },
    /// 等待一个入站事件（超时退出码 5）
    ///
    /// 只轮询 `/_mqs/debug/events`，因此只匹配「入站」事件。Bot 发出（出站）的
    /// 回复不会进事件流：用 `mqs messages list` 或 `mqs send --wait-reply` 查看。
    #[command(long_about = "等待一个入站事件（超时退出码 5）\n\n\
            只轮询 `/_mqs/debug/events`，因此只匹配「入站」事件。Bot 发出（出站）的\n\
            回复不会进事件流：用 `mqs messages list` 或 `mqs send --wait-reply` 查看。")]
    Expect(ExpectArgs),
    /// 虚拟时钟
    Clock {
        #[command(subcommand)]
        command: ClockCommand,
    },
    /// 场景脚本
    Script {
        #[command(subcommand)]
        command: ScriptCommand,
    },
    /// 查看调试帧（HTTP/WS 双向）
    ///
    /// 默认每个帧一行摘要；加 `--json` 输出完整帧（含 `data.request` 请求体）。
    /// 想看 Bot 回复的正文，用 `mqs messages list --json` 更直接。
    #[command(long_about = "查看调试帧（HTTP/WS 双向）\n\n\
            默认每个帧一行摘要；加 `--json` 输出完整帧（含 `data.request` 请求体）。\n\
            想看 Bot 回复的正文，用 `mqs messages list --json` 更直接。")]
    Logs {
        /// 持续跟踪新帧（Ctrl-C 退出）
        #[arg(short = 'f', long)]
        follow: bool,
        /// 只显示某类帧：http / ws / internal
        #[arg(long, default_value = "")]
        kind: String,
        /// 只显示该 Bot 的帧（app_id）
        #[arg(long, default_value = "")]
        bot: String,
        /// 首次回溯的帧数
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// 输出控制 API 的 OpenAPI
    Schema,
    /// 环境自检：目录、DB、端口、运行时
    Doctor,
    /// 以 stdio MCP server 方式运行（给 Agent 用）
    Mcp,
    /// 契约生成物
    Contract {
        #[command(subcommand)]
        command: ContractCommand,
    },
    /// profile 快照
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
}

#[derive(Args)]
struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 5700)]
    port: u16,
    #[arg(long, default_value_t = 5701)]
    control_port: u16,
    #[arg(long, default_value = "")]
    token: String,
    #[arg(long, default_value = "info")]
    log_level: String,
    #[arg(long)]
    no_docs: bool,
}

#[derive(Subcommand)]
enum UserCommand {
    /// 创建虚拟用户（--random 走种子生成器）
    Create {
        #[arg(long, default_value = "")]
        name: String,
        #[arg(long)]
        random: bool,
        #[arg(long, default_value_t = 1)]
        count: i64,
        #[arg(long)]
        seed: Option<i64>,
        #[arg(long)]
        behavior: Option<String>,
        #[arg(long, default_value = "human")]
        actor_type: String,
        #[arg(long, default_value = "")]
        group: String,
    },
    /// 列出虚拟用户
    List {
        #[arg(long)]
        available: bool,
    },
}

#[derive(Subcommand)]
enum GroupCommand {
    /// 创建群（默认把所有 Bot 拉进来）
    Create {
        #[arg(long, default_value = "")]
        name: String,
        #[arg(long, default_value_t = 0)]
        members: i64,
        #[arg(long = "bot")]
        bots: Vec<String>,
        #[arg(long, default_value = "")]
        owner: String,
    },
    /// 列出群
    List,
}

#[derive(Subcommand)]
enum BotCommand {
    /// 列出 Bot
    List,
    /// 创建 Bot
    Create {
        #[arg(long, default_value = "")]
        app_id: String,
        #[arg(long, default_value = "")]
        name: String,
        #[arg(long, default_value = "")]
        secret: String,
        #[arg(long, default_value = "ws")]
        delivery: String,
        #[arg(long)]
        random: bool,
        #[arg(long)]
        whitelisted: bool,
    },
    /// 设置 Bot 的内邀接口白名单（模拟腾讯 11253）
    Whitelist { app_id: String, state: String },
}

#[derive(Args)]
struct SendArgs {
    /// 发送者用户 id（如 u_000001），必须在该群/单聊里存在
    #[arg(long)]
    as_user: String,
    /// 消息正文（支持 QQ Markdown 文本交互标签）
    #[arg(long, default_value = "")]
    text: String,
    /// 目标会话：`group:<group_id>` 或 `dm:<app_id>:<user_id>`，也接受裸群号 / 裸 appid:userid
    #[arg(long, value_name = "SESSION")]
    to: Option<String>,
    /// 会话 id（与 --to 等价，优先级更高），例如 group:g_000001
    #[arg(long, value_name = "SESSION")]
    conversation: Option<String>,
    /// 发送时 @ 的 Bot（app_id），可重复
    #[arg(long = "at-bot")]
    at_bot: Vec<String>,
    /// 不 @ 任何 Bot（默认按 `chat.auto_at_bot` 设置决定是否 @ 会话内全部 Bot）
    #[arg(long)]
    no_at: bool,
    /// 发送后等待 Bot 出站回复；可省略值（默认 10s），例如 --wait-reply 5s
    #[arg(long, value_name = "TIMEOUT", num_args = 0..=1, default_missing_value = "10s")]
    wait_reply: Option<String>,
}

#[derive(Subcommand)]
enum MessagesCommand {
    /// 列出会话消息（按 seq 升序；Bot 回复的 direction=out）
    List {
        /// 会话 id，例如 group:g_000001 或 dm:10000:u_000001
        #[arg(long, value_name = "SESSION")]
        conversation: String,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        /// 只取 seq 小于该值的消息（向前翻页）
        #[arg(long)]
        before: Option<i64>,
    },
    /// 查看单条消息（含 markdown / keyboard / media 等原始字段）
    Show {
        /// 消息 id（MQS_MSG_...）
        message_id: String,
    },
}

#[derive(Args)]
struct ExpectArgs {
    /// 事件类型（大小写不敏感）；留空匹配任意事件
    #[arg(long, default_value = "")]
    event: String,
    /// 超时时间，例如 3s / 500ms（超时退出码 5）
    #[arg(long, default_value = "3s")]
    timeout: String,
    /// payload 里必须包含的字符串
    #[arg(long, default_value = "")]
    content: String,
    /// 只匹配该会话（group:... / dm:...）
    #[arg(long, default_value = "")]
    conversation: String,
}

#[derive(Subcommand)]
enum ClockCommand {
    /// 快进虚拟时钟（例如 6m / 2h / 30s）
    Advance { duration: String },
    /// 把虚拟时钟设为指定时间
    Set { iso: String },
}

#[derive(Subcommand)]
enum ScriptCommand {
    /// 列出内置与本地场景
    List,
    /// 运行场景（失败退出码 5）
    Run {
        script: String,
        #[arg(long, default_value = "")]
        junit: String,
        #[arg(long)]
        tap: bool,
    },
}

#[derive(Subcommand)]
enum ContractCommand {
    /// 生成覆盖率文档与前端 TS 类型（不依赖运行中的服务）
    Export {
        #[arg(long, default_value = "docs/COVERAGE.md")]
        out: String,
        #[arg(long = "types", default_value = "web/src/types/api.ts")]
        types: String,
    },
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// 列出本机 profile（读 profiles.json 索引）
    List,
    /// 导出当前 profile 快照（zip）
    Export {
        #[arg(default_value = "mqs-profile.zip")]
        path: String,
    },
    /// 导入 profile 快照（服务需已停止）
    Import {
        path: String,
        #[arg(long, default_value = "")]
        name: String,
        #[arg(long)]
        force: bool,
    },
}

// ------------------------------------------------------------------ client

struct Client {
    base: String,
    token: String,
    runtime: Value,
    http: reqwest::Client,
    out: Output,
}

/// 命令输出通道。`--json` 模式下 stdout 永远是单个 JSON 文档：成功是数据本身，
/// 失败是 `{"ok": false, "exit_code": ...}`（脚本可以直接 `json.load`）。
#[derive(Clone, Copy)]
struct Output {
    json: bool,
    show_secret: bool,
}

impl Output {
    fn new(json: bool, show_secret: bool) -> Self {
        Self { json, show_secret }
    }

    /// 诊断信息一律走 stderr，不污染 `--json` 的 stdout。
    fn note(&self, message: &str) {
        eprintln!("{message}");
    }

    fn print(&self, value: &Value) {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        );
    }

    /// 失败出口；返回退出码，调用处写 `return Err(out.fail(...))`。
    fn fail(&self, exit: i32, body: Value, hint: Option<&str>) -> i32 {
        if self.json {
            let mut body = body;
            if let Some(object) = body.as_object_mut() {
                object.insert("ok".into(), Value::Bool(false));
                object.insert("exit_code".into(), Value::from(exit));
                if let Some(hint) = hint {
                    object.insert("hint".into(), Value::String(hint.to_string()));
                }
            }
            self.print(&body);
        } else if body.get("code").is_some() {
            // 控制 API / QQ 错误保留原始信封（code/message/trace_id）。
            eprintln!("{}", serde_json::to_string(&body).unwrap_or_default());
        } else {
            eprintln!("error: {}", text(&body, "message"));
        }
        if let Some(hint) = hint {
            self.note(&format!("hint: {hint}"));
        }
        exit
    }

    fn error(&self, exit: i32, message: impl Into<String>, hint: Option<&str>) -> i32 {
        self.fail(exit, json!({"message": message.into()}), hint)
    }
}

impl Client {
    fn connect(home: Option<&str>, profile: &str, out: Output) -> Result<Client, i32> {
        let home = resolve_home(home);
        let runtime = match read_runtime(&home, profile) {
            Ok(runtime) => runtime,
            Err(err) => {
                return Err(out.error(
                    EXIT_CONN,
                    err.message,
                    Some("先运行 `mqs serve`，或用 --profile 指定已运行的 profile"),
                ));
            }
        };
        let port = runtime["control_port"].as_u64().unwrap_or(0);
        let token = runtime["token"].as_str().unwrap_or("").to_string();
        Ok(Client {
            base: format!("http://127.0.0.1:{port}"),
            token,
            runtime,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .unwrap_or_default(),
            out,
        })
    }

    async fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, i32> {
        let mut request = self
            .http
            .request(
                method.parse().unwrap_or(reqwest::Method::GET),
                format!("{}{}", self.base, path),
            )
            .header("x-mqs-token", &self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(err) => {
                return Err(self.out.error(
                    EXIT_CONN,
                    format!("cannot reach MQS control API at {}: {err}", self.base),
                    Some("先运行 `mqs serve`，或用 --profile 指定已运行的 profile"),
                ));
            }
        };
        if response.status().as_u16() == 204 {
            return Ok(Value::Null);
        }
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or_else(|_| {
            json!({"code": status.as_u16(), "message": text.chars().take(300).collect::<String>()})
        });
        if !status.is_success() {
            return Err(self.out.fail(EXIT_FAIL, value, None));
        }
        Ok(value)
    }

    async fn get(&self, path: &str) -> Result<Value, i32> {
        self.request("GET", path, None).await
    }

    async fn post(&self, path: &str, body: Option<&Value>) -> Result<Value, i32> {
        self.request("POST", path, body).await
    }

    async fn patch(&self, path: &str, body: Option<&Value>) -> Result<Value, i32> {
        self.request("PATCH", path, body).await
    }
}

fn table(headers: &[&str], rows: Vec<Vec<String>>) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    let line = |cells: &[String]| {
        cells
            .iter()
            .enumerate()
            .map(|(index, cell)| {
                let pad = widths[index].saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(pad))
            })
            .collect::<Vec<_>>()
            .join("  ")
    };
    println!(
        "{}",
        line(&headers.iter().map(|h| h.to_string()).collect::<Vec<_>>())
    );
    println!(
        "{}",
        widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for row in rows {
        println!("{}", line(&row));
    }
}

fn text(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// 消息的可读正文：普通 content 优先，其次 Markdown 正文，最后按消息形态兜底。
fn message_text(message: &Value) -> String {
    let content = text(message, "content");
    if !content.is_empty() {
        return content;
    }
    if let Some(markdown) = message
        .get("markdown")
        .and_then(|value| value.get("content"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return markdown.to_string();
    }
    if message.get("media").is_some() || message.get("attachments").is_some() {
        return "[媒体消息]".into();
    }
    if message.get("keyboard").is_some() {
        return "[按钮消息]".into();
    }
    if message.get("ark").is_some() || message.get("embed").is_some() {
        return "[卡片消息]".into();
    }
    String::new()
}

fn sender_label(message: &Value) -> String {
    let kind = text(message, "sender_type");
    let sender = &message["sender"];
    let name = text(sender, "name");
    let id = if name.is_empty() {
        text(sender, "id")
    } else {
        name
    };
    if id.is_empty() {
        kind
    } else if kind.is_empty() {
        id
    } else {
        format!("{kind}:{id}")
    }
}

/// 默认把 AppSecret 打码；`--show-secret` 才输出明文。
fn hide_secrets(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(hide_secrets),
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if key.eq_ignore_ascii_case("secret") && item.is_string() {
                    *item = Value::String("***".into());
                } else {
                    hide_secrets(item);
                }
            }
        }
        _ => {}
    }
}

// ------------------------------------------------------------------ entry

pub async fn main() -> i32 {
    let cli = Cli::parse();
    let out = Output::new(cli.json, cli.show_secret);
    match run(cli, out).await {
        Ok(code) => code,
        Err(code) => code,
    }
}

async fn run(cli: Cli, out: Output) -> Result<i32, i32> {
    let home = cli.home.clone();
    let profile = cli.profile.clone();
    let json = out.json;
    match cli.command {
        Command::Serve(args) => serve(args, home.as_deref(), &profile, out).await,
        Command::Status => {
            let client = Client::connect(home.as_deref(), &profile, out)?;
            let data = client.get("/_mqs/status").await?;
            if json {
                out.print(&data);
                return Ok(0);
            }
            println!(
                "profile    {}   uptime {:.0}s   clock {}",
                text(&data, "profile"),
                data["uptime_ms"].as_f64().unwrap_or(0.0) / 1000.0,
                text(&data["clock"], "mode")
            );
            println!("simulate   {}", text(&data, "base_url"));
            let local = text(&data, "local_base_url");
            if !local.is_empty() && local != text(&data, "base_url") {
                println!("loopback   {local}");
            }
            println!("gateway    {}", text(&data, "ws_url"));
            println!(
                "control    http://127.0.0.1:{}   token {}",
                text(&data, "control_port"),
                client.token
            );
            println!("gui        {}", text(&data, "gui_url"));
            let counts = &data["counts"];
            println!(
                "world      users={} bots={} groups={} messages={} events={}",
                counts["users"],
                counts["bots"],
                counts["groups"],
                counts["messages"],
                counts["events"]
            );
            println!(
                "gateway    {} connection(s) {}",
                data["gateway"]["connections"], data["gateway"]["bots"]
            );
            println!(
                "rate       {}  allowed={} rejected={}",
                text(&data["rate"], "mode"),
                data["rate"]["allowed"],
                data["rate"]["rejected"]
            );
            println!(
                "storage    {:.0} KiB  {}",
                data["db_size"].as_f64().unwrap_or(0.0) / 1024.0,
                text(&data, "profile_dir")
            );
            println!("contract   {}", text(&data["coverage"], "ascii"));
            Ok(0)
        }
        Command::Token { bot } => {
            let client = Client::connect(home.as_deref(), &profile, out)?;
            let bots = client.get("/_mqs/bots").await?;
            let Some(list) = bots.as_array().filter(|list| !list.is_empty()) else {
                return Err(out.error(EXIT_FAIL, "no bot in this profile", None));
            };
            let app_id = if bot.is_empty() {
                text(&list[0], "id")
            } else {
                bot
            };
            let data = client
                .post(&format!("/_mqs/bots/{app_id}/token"), None)
                .await?;
            if json {
                let mut output = data.clone();
                output["app_id"] = Value::String(app_id);
                out.print(&output);
            } else {
                println!("{}", text(&data, "access_token"));
                out.note(&format!(
                    "# app_id={app_id} expires_in={}",
                    text(&data, "expires_in")
                ));
            }
            Ok(0)
        }
        Command::User { command } => {
            let client = Client::connect(home.as_deref(), &profile, out)?;
            match command {
                UserCommand::Create {
                    name,
                    random,
                    count,
                    seed,
                    behavior,
                    actor_type,
                    group,
                } => {
                    let data = if random || count > 1 {
                        client
                            .post(
                                "/_mqs/users/random",
                                Some(&json!({
                                    "count": count,
                                    "seed": seed,
                                    "behavior": behavior,
                                    "actor_type": actor_type,
                                    "group_id": if group.is_empty() { Value::Null } else { Value::String(group) },
                                })),
                            )
                            .await?
                    } else {
                        client
                            .post(
                                "/_mqs/users",
                                Some(&json!({
                                    "nickname": if name.is_empty() { Value::Null } else { Value::String(name) },
                                    "actor_type": actor_type,
                                    "seed": seed,
                                    "behavior": behavior.unwrap_or_else(|| "normal".into()),
                                })),
                            )
                            .await?
                    };
                    if json {
                        out.print(&data);
                    } else {
                        let items: Vec<Value> = match data.as_array() {
                            Some(items) => items.clone(),
                            None => vec![data.clone()],
                        };
                        let rows = items
                            .iter()
                            .map(|user| {
                                vec![
                                    text(user, "id"),
                                    text(user, "nickname"),
                                    text(user, "actor_type"),
                                    text(user, "behavior"),
                                ]
                            })
                            .collect();
                        table(&["id", "nickname", "actor_type", "behavior"], rows);
                    }
                }
                UserCommand::List { available } => {
                    let data = client
                        .get(&format!("/_mqs/users?available={available}"))
                        .await?;
                    if json {
                        out.print(&data);
                    } else {
                        let rows = data
                            .as_array()
                            .map(|items| {
                                items
                                    .iter()
                                    .map(|user| {
                                        vec![
                                            text(user, "id"),
                                            text(user, "nickname"),
                                            text(user, "actor_type"),
                                            text(user, "behavior"),
                                        ]
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        table(&["id", "nickname", "actor_type", "behavior"], rows);
                    }
                }
            }
            Ok(0)
        }
        Command::Group { command } => {
            let client = Client::connect(home.as_deref(), &profile, out)?;
            match command {
                GroupCommand::Create {
                    name,
                    members,
                    bots,
                    owner,
                } => {
                    let data = client
                        .post(
                            "/_mqs/groups",
                            Some(&json!({
                                "name": if name.is_empty() { Value::Null } else { Value::String(name) },
                                "owner_id": if owner.is_empty() { Value::Null } else { Value::String(owner) },
                                "random_members": members,
                                "bot_ids": bots,
                            })),
                        )
                        .await?;
                    if json {
                        out.print(&data);
                    } else {
                        println!(
                            "created {}  {}  members={}  bots={}",
                            text(&data, "id"),
                            text(&data, "name"),
                            data["member_count"],
                            data["bot_ids"]
                        );
                    }
                }
                GroupCommand::List => {
                    let data = client.get("/_mqs/groups").await?;
                    if json {
                        out.print(&data);
                    } else {
                        let rows = data
                            .as_array()
                            .map(|items| {
                                items
                                    .iter()
                                    .map(|group| {
                                        vec![
                                            text(group, "id"),
                                            text(group, "name"),
                                            text(group, "member_count"),
                                            group["bot_ids"]
                                                .as_array()
                                                .map(|bots| {
                                                    bots.iter()
                                                        .filter_map(|b| b.as_str())
                                                        .collect::<Vec<_>>()
                                                        .join(",")
                                                })
                                                .unwrap_or_default(),
                                        ]
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        table(&["id", "name", "members", "bots"], rows);
                    }
                }
            }
            Ok(0)
        }
        Command::Bot { command } => {
            let client = Client::connect(home.as_deref(), &profile, out)?;
            match command {
                BotCommand::List => {
                    let mut data = client.get("/_mqs/bots").await?;
                    if !out.show_secret {
                        hide_secrets(&mut data);
                    }
                    if json {
                        out.print(&data);
                    } else {
                        let rows = data
                            .as_array()
                            .map(|items| {
                                items
                                    .iter()
                                    .map(|bot| {
                                        vec![
                                            text(bot, "app_id"),
                                            text(bot, "name"),
                                            text(bot, "secret"),
                                            bot["online"].to_string(),
                                            if bot["whitelisted"].as_bool().unwrap_or(false) {
                                                "yes".into()
                                            } else {
                                                "no".into()
                                            },
                                            bot["scopes"]
                                                .as_array()
                                                .map(|scopes| {
                                                    scopes
                                                        .iter()
                                                        .filter_map(|s| s.as_str())
                                                        .collect::<Vec<_>>()
                                                        .join(",")
                                                })
                                                .unwrap_or_default(),
                                        ]
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        table(
                            &["app_id", "name", "secret", "online", "whitelist", "scopes"],
                            rows,
                        );
                        if !out.show_secret {
                            out.note("hint: secret 已脱敏；`--show-secret` 显示明文");
                        }
                    }
                }
                BotCommand::Create {
                    app_id,
                    name,
                    secret,
                    delivery,
                    random,
                    whitelisted,
                } => {
                    let mut data = client
                        .post(
                            "/_mqs/bots",
                            Some(&json!({
                                "app_id": if app_id.is_empty() { Value::Null } else { Value::String(app_id) },
                                "name": if name.is_empty() { Value::Null } else { Value::String(name) },
                                "secret": if secret.is_empty() { Value::Null } else { Value::String(secret) },
                                "delivery": delivery,
                                "random": random,
                                "whitelisted": whitelisted,
                            })),
                        )
                        .await?;
                    if !out.show_secret {
                        hide_secrets(&mut data);
                    }
                    if json {
                        out.print(&data);
                    } else {
                        println!(
                            "created bot {} secret={}",
                            text(&data, "app_id"),
                            text(&data, "secret")
                        );
                        if !out.show_secret {
                            out.note("hint: secret 已脱敏；`--show-secret` 显示明文");
                        }
                    }
                }
                BotCommand::Whitelist { app_id, state } => {
                    let enabled =
                        ["on", "true", "1", "yes"].contains(&state.to_lowercase().as_str());
                    let data = client
                        .patch(
                            &format!("/_mqs/bots/{app_id}"),
                            Some(&json!({"whitelisted": enabled})),
                        )
                        .await?;
                    if json {
                        out.print(&data);
                    } else {
                        println!(
                            "{} whitelisted={}",
                            text(&data, "app_id"),
                            data["whitelisted"]
                        );
                    }
                }
            }
            Ok(0)
        }
        Command::Send(args) => {
            let client = Client::connect(home.as_deref(), &profile, out)?;
            // 先校验 --wait-reply，避免消息已经发出去了才发现超时时间写错。
            let wait_timeout = args
                .wait_reply
                .as_deref()
                .map(parse_duration)
                .transpose()
                .map_err(|err| {
                    out.error(EXIT_USAGE, format!("invalid --wait-reply: {err}"), None)
                })?;
            let at_bots = if args.no_at {
                Value::Array(vec![])
            } else if args.at_bot.is_empty() {
                Value::Null
            } else {
                Value::Array(
                    args.at_bot
                        .iter()
                        .map(|b| Value::String(b.clone()))
                        .collect(),
                )
            };
            let data = client
                .post(
                    "/_mqs/chat/send",
                    Some(&json!({
                        "as_user": args.as_user,
                        "content": args.text,
                        "to": args.to,
                        "conversation_id": args.conversation,
                        "at_bots": at_bots,
                    })),
                )
                .await?;
            let sent_at = Instant::now();
            let sent = &data["message"];
            if !json {
                println!(
                    "{}  {}  seq={}",
                    text(sent, "msg_id"),
                    text(sent, "conversation_id"),
                    sent["seq"]
                );
            }
            let Some(timeout_ms) = wait_timeout else {
                if json {
                    out.print(&data);
                }
                return Ok(0);
            };
            let timeout = args.wait_reply.as_deref().unwrap_or("");
            let conversation_id = text(sent, "conversation_id");
            let baseline = sent["seq"].as_i64().unwrap_or(0);
            let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
            let mut reply: Option<Value> = None;
            while Instant::now() < deadline {
                let list = client
                    .get(&format!(
                        "/_mqs/conversations/{conversation_id}/messages?limit=100"
                    ))
                    .await?;
                reply = list["items"].as_array().and_then(|items| {
                    items
                        .iter()
                        .rev()
                        .find(|message| {
                            message["sender_type"].as_str() == Some("bot")
                                && message["seq"].as_i64().unwrap_or(0) > baseline
                        })
                        .cloned()
                });
                if reply.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let Some(reply) = reply else {
                return Err(out.error(
                    EXIT_FAIL,
                    format!("timeout: no bot reply within {timeout}"),
                    Some("Bot 需在线（ws/webhook）并订阅事件；也可以 `mqs messages list` 手动查看"),
                ));
            };
            let waited_ms = sent_at.elapsed().as_millis() as u64;
            if json {
                out.print(&json!({
                    "sent": data,
                    "reply": reply,
                    "waited_ms": waited_ms,
                }));
            } else {
                println!("--- bot reply ({waited_ms}ms) ---");
                println!(
                    "{}  {}  seq={}",
                    text(&reply, "msg_id"),
                    text(&reply, "conversation_id"),
                    reply["seq"]
                );
                println!("{}", message_text(&reply));
            }
            Ok(0)
        }
        Command::Messages { command } => {
            let client = Client::connect(home.as_deref(), &profile, out)?;
            match command {
                MessagesCommand::List {
                    conversation,
                    limit,
                    before,
                } => {
                    let mut path =
                        format!("/_mqs/conversations/{conversation}/messages?limit={limit}");
                    if let Some(before) = before {
                        path.push_str(&format!("&before_seq={before}"));
                    }
                    let data = client.get(&path).await?;
                    if json {
                        out.print(&data);
                    } else {
                        let rows = data["items"]
                            .as_array()
                            .map(|items| {
                                items
                                    .iter()
                                    .map(|message| {
                                        vec![
                                            message["seq"].to_string(),
                                            text(message, "direction"),
                                            sender_label(message),
                                            message_text(message)
                                                .chars()
                                                .take(80)
                                                .collect::<String>(),
                                            text(message, "msg_id"),
                                        ]
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        table(&["seq", "dir", "from", "content", "msg_id"], rows);
                    }
                }
                MessagesCommand::Show { message_id } => {
                    let data = client.get(&format!("/_mqs/messages/{message_id}")).await?;
                    if json {
                        out.print(&data);
                    } else {
                        println!(
                            "{}  {}  seq={}  {}",
                            text(&data, "msg_id"),
                            text(&data, "conversation_id"),
                            data["seq"],
                            sender_label(&data)
                        );
                        println!("{}", message_text(&data));
                    }
                }
            }
            Ok(0)
        }
        Command::Expect(args) => {
            let client = Client::connect(home.as_deref(), &profile, out)?;
            let state = client.get("/_mqs/status").await?;
            let mut baseline = state["max_event_seq"].as_i64().unwrap_or(0);
            let timeout = parse_duration(&args.timeout)
                .map_err(|err| out.error(EXIT_USAGE, format!("invalid --timeout: {err}"), None))?;
            let deadline = std::time::Instant::now() + Duration::from_millis(timeout as u64);
            let needle = args.event.to_uppercase();
            while std::time::Instant::now() < deadline {
                let data = client
                    .get(&format!(
                        "/_mqs/debug/events?since_seq={baseline}&limit=200"
                    ))
                    .await?;
                for event in data["items"].as_array().cloned().unwrap_or_default() {
                    baseline = baseline.max(event["seq"].as_i64().unwrap_or(0));
                    if !needle.is_empty() && text(&event, "type") != needle {
                        continue;
                    }
                    if !args.conversation.is_empty()
                        && text(&event, "conversation_id") != args.conversation
                    {
                        continue;
                    }
                    if !args.content.is_empty()
                        && !event["payload"].to_string().contains(&args.content)
                    {
                        continue;
                    }
                    if json {
                        out.print(&event);
                    } else {
                        println!(
                            "{}  seq={}  {}  {}",
                            text(&event, "type"),
                            event["seq"],
                            text(&event, "conversation_id"),
                            event["payload"]
                                .to_string()
                                .chars()
                                .take(200)
                                .collect::<String>()
                        );
                    }
                    return Ok(0);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(out.error(
                EXIT_FAIL,
                format!("timeout: no event matched within {}", args.timeout),
                Some("只匹配入站事件；Bot 的回复用 `mqs messages list` 或 `mqs send --wait-reply`"),
            ))
        }
        Command::Clock { command } => {
            let client = Client::connect(home.as_deref(), &profile, out)?;
            match command {
                ClockCommand::Advance { duration } => {
                    let data = client
                        .post("/_mqs/clock/advance", Some(&json!({"duration": duration})))
                        .await?;
                    if json {
                        out.print(&data["clock"]);
                    } else {
                        println!(
                            "clock now {} ({})",
                            text(&data["clock"], "now"),
                            text(&data["clock"], "mode")
                        );
                    }
                }
                ClockCommand::Set { iso } => {
                    let payload = if iso.chars().all(|c| c.is_ascii_digit()) {
                        json!({"ms": iso.parse::<i64>().unwrap_or(0)})
                    } else {
                        json!({"iso": iso})
                    };
                    let data = client.post("/_mqs/clock/set", Some(&payload)).await?;
                    if json {
                        out.print(&data);
                    } else {
                        println!("clock now {}", text(&data["clock"], "now"));
                    }
                }
            }
            Ok(0)
        }
        Command::Script { command } => {
            let client = Client::connect(home.as_deref(), &profile, out)?;
            match command {
                ScriptCommand::List => {
                    let data = client.get("/_mqs/scenarios").await?;
                    if json {
                        out.print(&data);
                    } else {
                        let rows = data
                            .as_array()
                            .map(|items| {
                                items
                                    .iter()
                                    .map(|scenario| {
                                        vec![
                                            text(scenario, "name"),
                                            text(scenario, "source"),
                                            text(scenario, "steps"),
                                            text(scenario, "description"),
                                        ]
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        table(&["name", "source", "steps", "description"], rows);
                    }
                }
                ScriptCommand::Run { script, junit, tap } => {
                    let payload = match crate::scenario::builtin(&script) {
                        Some(text) => json!({"script": text}),
                        None => {
                            let path = PathBuf::from(&script);
                            if !path.exists() {
                                return Err(out.error(
                                    EXIT_FAIL,
                                    format!("scenario not found: {script}"),
                                    Some("`mqs script list` 查看内置场景"),
                                ));
                            }
                            json!({"path": path.canonicalize().unwrap_or(path).to_string_lossy()})
                        }
                    };
                    let data = client.post("/_mqs/scenarios/run", Some(&payload)).await?;
                    let passed = data["ok"].as_bool().unwrap_or(false);
                    if json {
                        let mut output = data.clone();
                        if let Some(object) = output.as_object_mut() {
                            object.remove("junit");
                            object.insert(
                                "exit_code".into(),
                                Value::from(if passed { 0 } else { EXIT_FAIL }),
                            );
                        }
                        out.print(&output);
                    } else {
                        for step in data["steps"].as_array().cloned().unwrap_or_default() {
                            let ok = step["ok"].as_bool().unwrap_or(false);
                            println!(
                                "[{}] {}  ({}ms)",
                                if ok { "PASS" } else { "FAIL" },
                                text(&step, "name"),
                                step["duration_ms"]
                            );
                            if !ok && !text(&step, "detail").is_empty() {
                                println!("       {}", text(&step, "detail"));
                            }
                        }
                        println!(
                            "\n{}: {}  {}ms",
                            text(&data, "name"),
                            if data["ok"].as_bool().unwrap_or(false) {
                                "ok"
                            } else {
                                "FAILED"
                            },
                            data["duration_ms"]
                        );
                        if tap {
                            println!("{}", text(&data, "tap"));
                        }
                    }
                    if !junit.is_empty() {
                        if let Some(parent) = Path::new(&junit).parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        std::fs::write(&junit, text(&data, "junit")).map_err(|err| {
                            out.error(EXIT_FAIL, format!("cannot write {junit}: {err}"), None)
                        })?;
                        out.note(&format!("junit written to {junit}"));
                    }
                    if !data["ok"].as_bool().unwrap_or(false) {
                        return Err(EXIT_FAIL);
                    }
                }
            }
            Ok(0)
        }
        Command::Logs {
            follow,
            kind,
            bot,
            limit,
        } => {
            let client = Client::connect(home.as_deref(), &profile, out)?;
            let state = client.get("/_mqs/status").await?;
            let mut since = (state["max_frame_id"].as_i64().unwrap_or(0) - limit).max(0);
            loop {
                let mut path = format!("/_mqs/debug/frames?since_id={since}&limit=200");
                if !kind.is_empty() {
                    path.push_str(&format!("&kind={kind}"));
                }
                if !bot.is_empty() {
                    path.push_str(&format!("&bot_id={bot}"));
                }
                let data = client.get(&path).await?;
                for frame in data["items"].as_array().cloned().unwrap_or_default() {
                    since = since.max(frame["id"].as_i64().unwrap_or(0));
                    if json {
                        println!("{}", serde_json::to_string(&frame).unwrap_or_default());
                    } else {
                        println!(
                            "{} {:6} {:8} {}",
                            hm(frame["ts"].as_i64().unwrap_or(0)),
                            text(&frame, "kind"),
                            text(&frame, "direction"),
                            text(&frame, "summary")
                        );
                    }
                }
                if !follow {
                    return Ok(0);
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        Command::Schema => {
            let client = Client::connect(home.as_deref(), &profile, out)?;
            let port = client.runtime["control_port"].as_u64().unwrap_or(0);
            let response = client
                .http
                .get(format!("http://127.0.0.1:{port}/openapi.json"))
                .send()
                .await
                .map_err(|err| out.error(EXIT_CONN, format!("cannot reach MQS: {err}"), None))?;
            let data: Value = response.json().await.unwrap_or(Value::Null);
            out.print(&data);
            Ok(0)
        }
        Command::Doctor => doctor(home.as_deref(), &profile, out).await,
        Command::Mcp => {
            crate::mcp::serve_stdio().await;
            Ok(0)
        }
        Command::Contract { command } => match command {
            ContractCommand::Export {
                out: coverage,
                types,
            } => {
                let out_path = PathBuf::from(&coverage);
                let types_path = PathBuf::from(&types);
                if let Some(parent) = out_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Some(parent) = types_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&out_path, crate::contract::coverage_markdown()).map_err(|err| {
                    out.error(EXIT_FAIL, format!("cannot write {coverage}: {err}"), None)
                })?;
                std::fs::write(&types_path, crate::contract::typescript()).map_err(|err| {
                    out.error(EXIT_FAIL, format!("cannot write {types}: {err}"), None)
                })?;
                if json {
                    out.print(&json!({
                        "coverage": coverage,
                        "types": types,
                        "summary": crate::contract::summary(),
                    }));
                } else {
                    println!("wrote {coverage} and {types}");
                    println!("{}", text(&crate::contract::summary(), "ascii"));
                }
                Ok(0)
            }
        },
        Command::Profile { command } => {
            profile_command(command, home.as_deref(), &profile, out).await
        }
    }
}

async fn serve(
    args: ServeArgs,
    home: Option<&str>,
    profile: &str,
    out: Output,
) -> Result<i32, i32> {
    let local = ["127.0.0.1", "localhost", "::1"].contains(&args.host.as_str());
    if !local && args.token.is_empty() {
        return Err(out.error(EXIT_USAGE, "--host 非本机地址时必须显式提供 --token", None));
    }
    if !local {
        out.note(&format!(
            "mqs: listening on {}: 任何能访问该地址的人都能连接沙箱（已显式提供 token）",
            args.host
        ));
    }
    let config = ProcessConfig {
        profile: profile.to_string(),
        home: resolve_home(home),
        host: args.host,
        sim_port: args.port,
        control_port: args.control_port,
        control_token: if args.token.is_empty() {
            crate::ids::short_token(32)
        } else {
            args.token
        },
        log_level: args.log_level,
        control_origins: Vec::new(),
        expose_docs: !args.no_docs,
        gui_url: String::new(),
        public_host: String::new(),
    };
    match crate::server::serve(config).await {
        Ok(()) => Ok(0),
        Err(err) => {
            let hint = if err.message.contains("cannot bind") {
                Some("端口被占用：同机并行多个 profile 时给后启动的 profile 指定 --port/--control-port（见 docs/UNIMPLEMENTED.md §7）")
            } else {
                None
            };
            let exit = if err.message.contains("locked") {
                EXIT_LOCK
            } else {
                EXIT_CONN
            };
            Err(out.error(exit, err.message, hint))
        }
    }
}

async fn doctor(home: Option<&str>, profile: &str, out: Output) -> Result<i32, i32> {
    let home_path = resolve_home(home);
    let paths = ProfilePaths::new(home_path.clone(), profile);
    let mut checks: Vec<(&str, bool, String)> = Vec::new();
    checks.push(("rust binary", true, VERSION.to_string()));
    match paths.ensure() {
        Ok(()) => {
            let probe = paths.directory().join(".doctor");
            let ok = std::fs::write(&probe, "ok").is_ok() && std::fs::remove_file(&probe).is_ok();
            checks.push((
                "profile dir writable",
                ok,
                paths.directory().to_string_lossy().into_owned(),
            ));
        }
        Err(err) => checks.push(("profile dir writable", false, err.message)),
    }
    if paths.db().exists() {
        let ok = rusqlite::Connection::open(paths.db())
            .map(|conn| {
                conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                    .map(|value| value == "ok")
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        checks.push((
            "sqlite integrity",
            ok,
            paths.db().to_string_lossy().into_owned(),
        ));
    } else {
        checks.push((
            "sqlite integrity",
            true,
            "no db yet (first run will create it)".into(),
        ));
    }
    match read_runtime(&home_path, profile) {
        Ok(runtime) => {
            let port = runtime["control_port"].as_u64().unwrap_or(0);
            let alive = reqwest::Client::new()
                .get(format!("http://127.0.0.1:{port}/_mqs/health"))
                .timeout(Duration::from_secs(2))
                .send()
                .await
                .map(|response| response.status().is_success())
                .unwrap_or(false);
            checks.push(("running server", alive, format!("control port {port}")));
        }
        Err(_) => checks.push(("running server", true, "not running".into())),
    }
    let mut failed = 0;
    if out.json {
        let items: Vec<Value> = checks
            .iter()
            .map(|(label, ok, detail)| json!({"check": label, "ok": ok, "detail": detail}))
            .collect();
        for (_, ok, _) in &checks {
            if !ok {
                failed += 1;
            }
        }
        out.print(&json!({
            "ok": failed == 0,
            "exit_code": if failed == 0 { 0 } else { EXIT_CONN },
            "profile": profile,
            "checks": items,
        }));
    } else {
        for (label, ok, detail) in &checks {
            println!(
                "[{}] {:<22} {}",
                if *ok { "ok " } else { "ERR" },
                label,
                detail
            );
            if !ok {
                failed += 1;
            }
        }
    }
    if failed > 0 {
        Err(EXIT_CONN)
    } else {
        Ok(0)
    }
}

async fn profile_command(
    command: ProfileCommand,
    home: Option<&str>,
    profile: &str,
    out: Output,
) -> Result<i32, i32> {
    let home_path = resolve_home(home);
    match command {
        ProfileCommand::List => {
            let index_path = home_path.join("profiles.json");
            let mut entries: Vec<Value> = Vec::new();
            if let Ok(text) = std::fs::read_to_string(&index_path) {
                if let Ok(data) = serde_json::from_str::<Value>(&text) {
                    if let Some(profiles) = data["profiles"].as_object() {
                        entries = profiles.values().cloned().collect();
                    }
                }
            }
            if entries.is_empty() {
                if let Ok(read) = std::fs::read_dir(home_path.join("profiles")) {
                    for entry in read.flatten() {
                        if entry.path().is_dir() {
                            entries.push(json!({
                                "name": entry.file_name().to_string_lossy(),
                                "path": entry.path().to_string_lossy(),
                            }));
                        }
                    }
                }
            }
            if out.json {
                out.print(&Value::Array(entries));
            } else {
                let rows = entries
                    .iter()
                    .map(|entry| {
                        vec![
                            text(entry, "name"),
                            text(entry, "path"),
                            text(entry, "last_opened_at"),
                        ]
                    })
                    .collect();
                table(&["name", "path", "last_opened"], rows);
            }
            Ok(0)
        }
        ProfileCommand::Export { path } => {
            let client = Client::connect(home, profile, out)?;
            let response = client
                .http
                .post(format!("{}/_mqs/snapshot", client.base))
                .header("x-mqs-token", &client.token)
                .send()
                .await
                .map_err(|err| out.error(EXIT_CONN, format!("cannot reach MQS: {err}"), None))?;
            if !response.status().is_success() {
                let text = response.text().await.unwrap_or_default();
                return Err(out.fail(
                    EXIT_FAIL,
                    serde_json::from_str(&text).unwrap_or_else(
                        |_| json!({"message": text.chars().take(300).collect::<String>()}),
                    ),
                    None,
                ));
            }
            let data = response.bytes().await.map_err(|err| {
                out.error(EXIT_FAIL, format!("cannot read snapshot: {err}"), None)
            })?;
            std::fs::write(&path, &data)
                .map_err(|err| out.error(EXIT_FAIL, format!("cannot write {path}: {err}"), None))?;
            if out.json {
                out.print(&json!({"path": path, "bytes": data.len()}));
            } else {
                println!("wrote {} ({} bytes)", path, data.len());
            }
            Ok(0)
        }
        ProfileCommand::Import { path, name, force } => {
            let source = PathBuf::from(&path);
            if !source.exists() {
                return Err(out.error(EXIT_FAIL, format!("not found: {}", source.display()), None));
            }
            let target_name = if name.is_empty() {
                source
                    .file_stem()
                    .map(|stem| {
                        stem.to_string_lossy()
                            .trim_end_matches(".mqsprofile")
                            .to_string()
                    })
                    .unwrap_or_else(|| "imported".into())
            } else {
                name
            };
            let target = ProfilePaths::new(home_path, target_name.clone()).directory();
            if target.exists() && !force {
                return Err(out.error(
                    EXIT_USAGE,
                    format!("profile already exists: {} (use --force)", target.display()),
                    None,
                ));
            }
            let staging = target.with_extension("importing");
            let _ = std::fs::remove_dir_all(&staging);
            std::fs::create_dir_all(&staging).map_err(|err| {
                out.error(
                    EXIT_FAIL,
                    format!("cannot create {}: {err}", staging.display()),
                    None,
                )
            })?;
            let file = std::fs::File::open(&source).map_err(|err| {
                out.error(
                    EXIT_FAIL,
                    format!("cannot read {}: {err}", source.display()),
                    None,
                )
            })?;
            let mut archive = zip::ZipArchive::new(file)
                .map_err(|err| out.error(EXIT_FAIL, format!("invalid snapshot: {err}"), None))?;
            archive
                .extract(&staging)
                .map_err(|err| out.error(EXIT_FAIL, format!("invalid snapshot: {err}"), None))?;
            if !staging.join("db.sqlite").exists() {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(out.error(EXIT_FAIL, "invalid snapshot: db.sqlite missing", None));
            }
            let _ = std::fs::remove_dir_all(&target);
            std::fs::rename(&staging, &target).map_err(|err| {
                out.error(EXIT_FAIL, format!("cannot move snapshot: {err}"), None)
            })?;
            if !target.join("profile.json").exists() {
                let _ = std::fs::write(
                    target.join("profile.json"),
                    json!({"name": target_name, "schema_version": 1}).to_string(),
                );
            }
            if out.json {
                out.print(&json!({
                    "imported": target_name,
                    "path": target.to_string_lossy(),
                }));
            } else {
                println!("imported profile {} -> {}", target_name, target.display());
            }
            Ok(0)
        }
    }
}
