//! The `mqs` command line interface.
//!
//! Every command is a thin client of the control API — there is no second
//! implementation path.

use std::path::{Path, PathBuf};
use std::time::Duration;

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
    #[arg(long, global = true)]
    json: bool,
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
    Send(SendArgs),
    /// 等待一个事件（超时退出码 5）
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
    Logs {
        #[arg(short = 'f', long)]
        follow: bool,
        #[arg(long, default_value = "")]
        kind: String,
        #[arg(long, default_value = "")]
        bot: String,
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
    #[arg(long)]
    as_user: String,
    #[arg(long, default_value = "")]
    text: String,
    #[arg(long, default_value = "")]
    to: String,
    #[arg(long, default_value = "")]
    conversation: String,
    #[arg(long = "at-bot")]
    at_bot: Vec<String>,
    #[arg(long)]
    no_at: bool,
}

#[derive(Args)]
struct ExpectArgs {
    #[arg(long, default_value = "")]
    event: String,
    #[arg(long, default_value = "3s")]
    timeout: String,
    #[arg(long, default_value = "")]
    content: String,
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
}

impl Client {
    fn connect(home: Option<&str>, profile: &str) -> Result<Client, i32> {
        let home = resolve_home(home);
        let runtime = match read_runtime(&home, profile) {
            Ok(runtime) => runtime,
            Err(err) => {
                eprintln!("error: {}", err.message);
                eprintln!("hint: 先运行 `mqs serve`，或用 --profile 指定已运行的 profile");
                return Err(EXIT_CONN);
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
                eprintln!(
                    "error: cannot reach MQS control API at {}: {err}",
                    self.base
                );
                return Err(EXIT_CONN);
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
            eprintln!("{}", serde_json::to_string(&value).unwrap_or_default());
            return Err(EXIT_FAIL);
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

fn emit(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
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

// ------------------------------------------------------------------ entry

pub async fn main() -> i32 {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(code) => code,
        Err(code) => code,
    }
}

async fn run(cli: Cli) -> Result<i32, i32> {
    let home = cli.home.clone();
    let profile = cli.profile.clone();
    let json = cli.json;
    match cli.command {
        Command::Serve(args) => serve(args, home.as_deref(), &profile).await,
        Command::Status => {
            let client = Client::connect(home.as_deref(), &profile)?;
            let data = client.get("/_mqs/status").await?;
            if json {
                emit(&data);
                return Ok(0);
            }
            println!(
                "profile    {}   uptime {:.0}s   clock {}",
                text(&data, "profile"),
                data["uptime_ms"].as_f64().unwrap_or(0.0) / 1000.0,
                text(&data["clock"], "mode")
            );
            println!("simulate   {}", text(&data, "base_url"));
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
            let client = Client::connect(home.as_deref(), &profile)?;
            let bots = client.get("/_mqs/bots").await?;
            let Some(list) = bots.as_array().filter(|list| !list.is_empty()) else {
                eprintln!("no bot in this profile");
                return Err(EXIT_FAIL);
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
                emit(&output);
            } else {
                println!("{}", text(&data, "access_token"));
                eprintln!("# app_id={app_id} expires_in={}", text(&data, "expires_in"));
            }
            Ok(0)
        }
        Command::User { command } => {
            let client = Client::connect(home.as_deref(), &profile)?;
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
                        emit(&data);
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
                        emit(&data);
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
            let client = Client::connect(home.as_deref(), &profile)?;
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
                        emit(&data);
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
                        emit(&data);
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
            let client = Client::connect(home.as_deref(), &profile)?;
            match command {
                BotCommand::List => {
                    let data = client.get("/_mqs/bots").await?;
                    if json {
                        emit(&data);
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
                    let data = client
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
                    if json {
                        emit(&data);
                    } else {
                        println!(
                            "created bot {} secret={}",
                            text(&data, "app_id"),
                            text(&data, "secret")
                        );
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
                    println!(
                        "{} whitelisted={}",
                        text(&data, "app_id"),
                        data["whitelisted"]
                    );
                }
            }
            Ok(0)
        }
        Command::Send(args) => {
            let client = Client::connect(home.as_deref(), &profile)?;
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
                        "to": if args.to.is_empty() { Value::Null } else { Value::String(args.to) },
                        "conversation_id": if args.conversation.is_empty() { Value::Null } else { Value::String(args.conversation) },
                        "at_bots": at_bots,
                    })),
                )
                .await?;
            if json {
                emit(&data);
            } else {
                let message = &data["message"];
                println!(
                    "{}  {}  seq={}",
                    text(message, "msg_id"),
                    text(message, "conversation_id"),
                    message["seq"]
                );
            }
            Ok(0)
        }
        Command::Expect(args) => {
            let client = Client::connect(home.as_deref(), &profile)?;
            let state = client.get("/_mqs/status").await?;
            let mut baseline = state["max_event_seq"].as_i64().unwrap_or(0);
            let timeout = parse_duration(&args.timeout).map_err(|err| {
                eprintln!("error: {err}");
                EXIT_USAGE
            })?;
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
                        emit(&event);
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
            eprintln!("timeout: no event matched within {}", args.timeout);
            Err(EXIT_FAIL)
        }
        Command::Clock { command } => {
            let client = Client::connect(home.as_deref(), &profile)?;
            match command {
                ClockCommand::Advance { duration } => {
                    let data = client
                        .post("/_mqs/clock/advance", Some(&json!({"duration": duration})))
                        .await?;
                    if json {
                        emit(&data["clock"]);
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
                        emit(&data);
                    } else {
                        println!("clock now {}", text(&data["clock"], "now"));
                    }
                }
            }
            Ok(0)
        }
        Command::Script { command } => {
            let client = Client::connect(home.as_deref(), &profile)?;
            match command {
                ScriptCommand::List => {
                    let data = client.get("/_mqs/scenarios").await?;
                    if json {
                        emit(&data);
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
                                eprintln!("scenario not found: {script}");
                                return Err(EXIT_FAIL);
                            }
                            json!({"path": path.canonicalize().unwrap_or(path).to_string_lossy()})
                        }
                    };
                    let data = client.post("/_mqs/scenarios/run", Some(&payload)).await?;
                    if json {
                        let mut output = data.clone();
                        if let Some(object) = output.as_object_mut() {
                            object.remove("junit");
                        }
                        emit(&output);
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
                            eprintln!("error: cannot write {junit}: {err}");
                            EXIT_FAIL
                        })?;
                        eprintln!("junit written to {junit}");
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
            let client = Client::connect(home.as_deref(), &profile)?;
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
            let client = Client::connect(home.as_deref(), &profile)?;
            let port = client.runtime["control_port"].as_u64().unwrap_or(0);
            let response = client
                .http
                .get(format!("http://127.0.0.1:{port}/openapi.json"))
                .send()
                .await
                .map_err(|err| {
                    eprintln!("error: {err}");
                    EXIT_CONN
                })?;
            let data: Value = response.json().await.unwrap_or(Value::Null);
            emit(&data);
            Ok(0)
        }
        Command::Doctor => doctor(home.as_deref(), &profile).await,
        Command::Mcp => {
            crate::mcp::serve_stdio().await;
            Ok(0)
        }
        Command::Contract { command } => match command {
            ContractCommand::Export { out, types } => {
                let out_path = PathBuf::from(&out);
                let types_path = PathBuf::from(&types);
                if let Some(parent) = out_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Some(parent) = types_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&out_path, crate::contract::coverage_markdown()).map_err(|err| {
                    eprintln!("error: cannot write {out}: {err}");
                    EXIT_FAIL
                })?;
                std::fs::write(&types_path, crate::contract::typescript()).map_err(|err| {
                    eprintln!("error: cannot write {types}: {err}");
                    EXIT_FAIL
                })?;
                if json {
                    emit(&json!({
                        "coverage": out,
                        "types": types,
                        "summary": crate::contract::summary(),
                    }));
                } else {
                    println!("wrote {out} and {types}");
                    println!("{}", text(&crate::contract::summary(), "ascii"));
                }
                Ok(0)
            }
        },
        Command::Profile { command } => {
            profile_command(command, home.as_deref(), &profile, json).await
        }
    }
}

async fn serve(args: ServeArgs, home: Option<&str>, profile: &str) -> Result<i32, i32> {
    let local = ["127.0.0.1", "localhost", "::1"].contains(&args.host.as_str());
    if !local && args.token.is_empty() {
        eprintln!("error: --host 非本机地址时必须显式提供 --token");
        return Err(EXIT_USAGE);
    }
    if !local {
        eprintln!(
            "mqs: listening on {}: 任何能访问该地址的人都能连接沙箱（已显式提供 token）",
            args.host
        );
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
            eprintln!("error: {}", err.message);
            if err.message.contains("locked") {
                Err(EXIT_LOCK)
            } else {
                Err(EXIT_CONN)
            }
        }
    }
}

async fn doctor(home: Option<&str>, profile: &str) -> Result<i32, i32> {
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
    json: bool,
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
            if json {
                emit(&Value::Array(entries));
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
            let client = Client::connect(home, profile)?;
            let response = client
                .http
                .post(format!("{}/_mqs/snapshot", client.base))
                .header("x-mqs-token", &client.token)
                .send()
                .await
                .map_err(|err| {
                    eprintln!("error: {err}");
                    EXIT_CONN
                })?;
            if !response.status().is_success() {
                eprintln!("{}", response.text().await.unwrap_or_default());
                return Err(EXIT_FAIL);
            }
            let data = response.bytes().await.map_err(|_| EXIT_FAIL)?;
            std::fs::write(&path, &data).map_err(|err| {
                eprintln!("error: cannot write {path}: {err}");
                EXIT_FAIL
            })?;
            println!("wrote {} ({} bytes)", path, data.len());
            Ok(0)
        }
        ProfileCommand::Import { path, name, force } => {
            let source = PathBuf::from(&path);
            if !source.exists() {
                eprintln!("not found: {}", source.display());
                return Err(EXIT_FAIL);
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
                eprintln!("profile already exists: {} (use --force)", target.display());
                return Err(EXIT_USAGE);
            }
            let staging = target.with_extension("importing");
            let _ = std::fs::remove_dir_all(&staging);
            std::fs::create_dir_all(&staging).map_err(|_| EXIT_FAIL)?;
            let file = std::fs::File::open(&source).map_err(|_| EXIT_FAIL)?;
            let mut archive = zip::ZipArchive::new(file).map_err(|err| {
                eprintln!("invalid snapshot: {err}");
                EXIT_FAIL
            })?;
            archive.extract(&staging).map_err(|err| {
                eprintln!("invalid snapshot: {err}");
                EXIT_FAIL
            })?;
            if !staging.join("db.sqlite").exists() {
                let _ = std::fs::remove_dir_all(&staging);
                eprintln!("invalid snapshot: db.sqlite missing");
                return Err(EXIT_FAIL);
            }
            let _ = std::fs::remove_dir_all(&target);
            std::fs::rename(&staging, &target).map_err(|err| {
                eprintln!("cannot move snapshot: {err}");
                EXIT_FAIL
            })?;
            if !target.join("profile.json").exists() {
                let _ = std::fs::write(
                    target.join("profile.json"),
                    json!({"name": target_name, "schema_version": 1}).to_string(),
                );
            }
            println!("imported profile {} -> {}", target_name, target.display());
            Ok(0)
        }
    }
}
