# AGENTS.md

给在本仓库工作的 Agent（以及人类贡献者）的说明书。先读完这页再动手。

## 1. 这是什么

QSandbox（MQS）是 **QQ Bot API v2 的本地沙箱**：一个 Rust 二进制同时提供

- **Simulate API**（默认 `:5700`）：QQ 兼容的 REST + 网关 WebSocket；
- **Control API**（默认 `:5701`）：GUI / CLI / MCP 共用，token + Origin 保护，并托管内嵌的 Vue 前端；
- **SQLite + media**：单进程、WAL、内容哈希寻址。

Tauri 桌面包只是窗口：它在**同一进程**里启动同一个 server crate，没有 sidecar。

## 2. 不可动摇的边界

1. **Python 只保留垫片**。`python/` 是 `mqs-compat`（botpy 猴补丁），因为 botpy 把域名和
   token URL 写死、只能用 Python 打补丁。其余一切（服务、CLI、MCP、场景、桌面端）都在 Rust 里。
   不要把业务逻辑塞回 Python，也不要把垫片编译进二进制。
2. **频道（guild/channel）家族已删除**，不要重新引入：`/guilds/*`、`/channels/*`、
   `/users/@me/dms`、`/dms/*`、好友（`FRIEND_*`）、频道身份组/权限位、频道事件与 intent
   （`1<<0/1/9/12/30`）都不属于本产品。新增接口前先看 `contract.rs` 的现有清单。
3. **协议保真优先于“方便”**。改消息/网关行为前先读 `docs/COMPAT.md` 与
   [腾讯 QQ 机器人文档](https://bot.q.qq.com/wiki/)；沙箱说“和真实平台一样”才有价值。
4. **不向下兼容旧版本**（旧 Python 实现、旧 schema、旧 Electron 壳都已移除）。
   数据库只有一份 `crates/mqs/src/schema.sql` 与一个 `SCHEMA_VERSION`，没有迁移脚本。
5. 所有假凭据带 `MQS_` / `mqs-` 前缀；仓库里不能出现真实 secret。

## 3. 目录

```
crates/mqs/
  build.rs        把 web/dist 内嵌进二进制（存在才内嵌）
  src/
    error.rs      QQ 错误封装（code/status/trace_id/Retry-After）
    http.rs       AppState、提取器（BotCtx / WhitelistedBot / QqJson）、中间件
    world.rs      世界模型 + 会话/消息/事件/媒体/帧/规则
    messages.rs   收发语义：被动窗口、msg_seq 去重、@、按钮、撤回、投递
    community.rs  群管理（内邀接口）
    gateway.rs    网关状态机（hello/identify/ready/heartbeat/resume/intents/shard）
    sim.rs        模拟 QQ API 路由（含 ROUTE_TABLE）
    control.rs    控制 API + 静态 GUI + 控制 WebSocket
    scenario.rs   YAML 场景引擎（内置模板 include 自 scenarios/）
    contract.rs   端点注册表 + 覆盖率/TS 生成
    cli.rs        `mqs` 命令行
    mcp.rs        stdio MCP server
    server.rs     双端口监听、runtime.json、生命周期
  tests/          集成测试（每个用例起一个进程内服务）
web/              Vue 3 + Vite（状态/配置/聊天/调试器/设置）
desktop/src-tauri Tauri 壳（图标、tauri.conf.json、capabilities）
python/           mqs-compat（唯一的 Python 代码）
scenarios/        YAML 场景（内置模板与文件同源）
examples/         botpy echo bot
docs/             COMPAT.md（已知差异）、COVERAGE.md（生成物）、UNIMPLEMENTED.md（缺口）
```

## 4. 常用命令

```bash
cargo fmt --all --check
cargo clippy --all-targets -p mqs -- -D warnings
cargo test -p mqs                       # 20 单元 + 78 集成
cargo run -q -p mqs -- serve            # 起沙箱
cargo run -q -p mqs -- status --json

cd web && npm ci && npm run typecheck && npm run build
cd desktop && npm install && npm run build   # 需要 WebKitGTK / WebView2 / WKWebView

# 契约生成物（改了注册表或模型后必须重跑，否则 CI 会 diff 失败）
cargo run -q -p mqs -- contract export
```

## 5. 加一个 QQ 接口的正确顺序

1. 在 `sim.rs` 写 handler（用现成的 `BotCtx` / `WhitelistedBot` / `QqJson` 提取器；
   错误一律返回 `ApiError`，不要手搓 JSON）。
2. 在 `sim.rs::sim_router` 挂路由，并把 `(METHOD, path)` 加进 `sim.rs::ROUTE_TABLE`。
3. 在 `contract.rs` 的 `REGISTRY` 添加一行 `ep!(...)`（key/method/path/summary/auth/layer/tag、
   scopes、errors、coverage、notes）。
4. 写集成测试（`crates/mqs/tests/`），断言状态码、`code`、`X-Tps-Trace-ID` 与关键字段。
5. `cargo run -q -p mqs -- contract export` 重新生成 `docs/COVERAGE.md` 与 `web/src/types/api.ts`。
6. 如果控制台也要用，改 `web/src/views/*.vue` 并按需在 `control.rs` 加对应控制端点。

`crates/mqs/tests/contract.rs` 会强制「注册表 ↔ 路由表」双向一致，漏一步就红。

## 6. 协议不变量（改代码前必须知道）

- **错误形状**：`{"code": <int>, "message": <str>, "trace_id": <hex16>}`，响应头 `X-Tps-Trace-ID`；
  绝不能出现 `{"detail": ...}` 或 500 裸异常。限流额外带 `Retry-After` + `X-RateLimit-*`。
- **鉴权**：`Authorization: QQBot <token>`（也接受 `Bearer`、`x-bot-token`），token 7200s、
  受**虚拟时钟**控制；错误码 11241/11242/11243/11244。
- **内邀接口**：`feature.whitelist` 打开时非白名单 Bot → 11253；`feature.expose=public` 时整族 404。
- **openid 按 Bot 隔离**：`openid_map(bot_id, kind, entity_id)`，同一 user 在两个 Bot 下 openid 不同。
- **被动回复**：窗口 群 5min/5 次、单聊 60min/4 次；`msg_id` 必须属于该 Bot 在该会话收到的入站消息；
  `msg_seq` 显式重复 → 40054；未给 `msg_seq` 时自动递增。
- **主动消息**：先过 Bot 级 / 群级开关（40040），再过滑动窗口限流（42900），再扣每日 1000 条配额（40055）；
  `require_online` 时未连网关 → 40056。
- **事件**：`events.seq` 全局自增，网关 `s` 用它；`messages.seq` 是会话内序号；
  RESUME 按 `seq > last_seq` 补发（最多 1000 条）。
- **群消息事件种类**由 `group_bots.recv_msg_setting` 决定：
  `all` → 所有消息 `GROUP_MESSAGE_CREATE`（@ 由 `mentions[].is_you` 表达，content 不带前缀）；
  否则只有被 @ 时 → `GROUP_AT_MESSAGE_CREATE`（content 带 `<@!openid>` 前缀）。两者共用 intent `1<<25`。
- **网关关闭码**：4002 非法 JSON、4004 鉴权失败、4006 无效 session、4009 心跳超时、4010 非法分片。
- **时间**：业务时间一律走 `world.clock`（虚拟时钟），`uptime` 用墙钟。

## 7. 数据库

- 单文件 `schema.sql`，`Database` 用一个 `Mutex<Connection>`；事务用 `db.tx(|conn| ...)`，
  **闭包里不能调用其它 `db.*` 方法**（会死锁）。
- 行是 `serde_json::Map`（`Row`），用 `db::s/i/b/json_col` 取值——这是刻意的动态模型，
  与真实 payload 的“未知字段原样保留”语义一致。
- 改 schema：直接改 `schema.sql` 并 bump `SCHEMA_VERSION` + 删掉旧 profile（不做迁移）。

## 8. 测试约定

- 集成测试用 `crates/mqs/tests/common/mod.rs` 的 `Test`：进程内起服务、随机端口、临时 `MQS_HOME`；
  不要连真实端口或真实 QQ。
- 需要“Bot 在线”才能主动发消息的用例，用 `test.settings(json!({"require_online": false}))`
  或在 `Test::connect_gateway` 建立真实 WS 连接。
- 时间相关（窗口/配额/token 过期）用控制 API 快进虚拟时钟，不要 `sleep` 真实时间。
- 场景测试用 `spawn_echo_bot` 起一个最小 SUT；`scenarios/smoke.yaml` 是验收基线。
- 新行为优先补集成测试，其次补单元测试；`cargo test -p mqs`（98 项）必须全绿。

## 9. 前端

- Vue 3 + `<script setup>` + TypeScript，深色优先、文案 zh-CN。
- 所有请求走 `web/src/api.ts`（自动带 `X-MQS-Token`/cookie，错误统一 `ApiError`）。
- 视图：`StatusView`（状态/接入信息）、`ConfigView`（用户/群/Bot/规则）、`ChatView`（会话/群管理）、
  `DebugView`（帧/事件）、`SettingsPanel`。没有频道页，别再加水。
- 每改完必须 `npm run typecheck && npm run build`；构建产物不提交（`web/dist` 被忽略）。

## 10. CI / 发布

`.github/workflows/ci.yml` 是唯一入口（可被 release.yml `workflow_call` 复用）：

| job | 内容 |
|---|---|
| `rust` | fmt、clippy `-D warnings`、全部测试、契约生成物无 drift |
| `web` | `npm ci`、`vue-tsc`、`vite build`，上传 `web-dist` artifact |
| `botpy-compat` | 真 botpy + `mqs-compat` 垫片跑 echo bot，再跑 `mqs script run smoke` |
| `desktop` | 三平台 Tauri 打包，校验安装包确实产出 |
| `cli` | 带内嵌 GUI 的 release 二进制冒烟 + 打包 |

`release.yml` 在 `v*` tag 上复用整份 CI，再把安装包/CLI 归档发到 GitHub Release；
`publish.yml` 只发布 Python 垫片（`mqs-compat`）到 PyPI。桌面端不分发 sidecar。

## 11. 汇报风格

- 说结果、说证据（测试名/命令/数字），不要只说“已完成”。
- 发现与真实 QQ 行为不一致时，先改代码对齐，改不了就写进 `docs/COMPAT.md` 的“已知差异”，
  不要静默糊过去。
- 不确定的接口/字段宁可不写，也不要编造；`docs/UNIMPLEMENTED.md` 是诚实的缺口清单。
