# SDK 兼容性与已知差异

> 文档快照：2026-11-09 · 对应 QSandbox v0.3（Rust + Tauri）

## 已实测

| SDK / 客户端 | 结果 | 说明 |
|---|---|---|
| **botpy（qq-botpy）** | ✅ 通过 | CI 的 `botpy-compat` job：`examples/botpy_echo/bot.py` 登录 → 网关鉴权 → 心跳 → 群 @ 收消息 → 被动回复 → `mqs script run smoke` 全链路 |
| 裸 WebSocket 客户端 | ✅ 通过 | `tests/gateway.rs` 覆盖 identify/heartbeat/resume/分片/intents/关闭码 |
| curl / httpx | ✅ 通过 | 所有 REST 端点都有集成测试（`tests/*.rs`） |

Koishi / NoneBot / qq-bot-sdk 未实测：它们需要各自的 `base_url`/`ws_url` 配置或补丁。
控制台“Bot 接入”弹窗会按 nonebot-adapter-qq 的配置项生成 `QQ_API_BASE` / `QQ_AUTH_BASE` /
`QQ_CUSTOM_GATEWAY_URL` / `QQ_BOTS` 片段，但仓库里没有真实 NoneBot 实例的回归。

## botpy 垫片（唯一的 Python 部分）

botpy 没有 `base_url` 配置：`botpy/http.py:Route` 把 `https://api.sgroup.qq.com` 写死，
`botpy/robot.py:Token.update_access_token` 把 `https://bots.qq.com/app/getAppAccessToken` 写死。
因此：

```bash
pip install qq-botpy ./python        # 安装 qq-botpy 和 mqs-compat
```

```python
import mqs.compat.botpy   # 必须在创建 botpy.Client 之前 import
```

它会：

1. 把 `Route.SCHEME/DOMAIN/SANDBOX_DOMAIN` 指向 `MQS_BASE_URL`（默认 `http://127.0.0.1:5700`）；
2. 替换 `Token.update_access_token`，改请求沙箱的 `POST /app/getAppAccessToken`。

`patch_from_runtime()` 可读取 `mqs serve` 写的 `profiles/<name>/runtime.json` 自动定位端口。
网关地址不需要改：`GET /gateway/bot` 返回基于请求 Host 的 `ws://<host>:<port>/websocket`。

其他 SDK 只要支持自定义 REST domain + token URL，或用同样的方式打补丁即可。

## 已知差异（按影响排序）

| # | 差异 | 影响 | 处理 |
|---|---|---|---|
| 1 | **频道（guild/channel）家族整体不支持**：`/guilds/*`、`/channels/*`、`/users/@me/dms`、`/dms/*`、好友与频道事件都不再提供（返回 404） | 频道机器人无法联调 | 这是产品决定：沙箱只覆盖群聊 + 单聊 + 群管理 + 媒体 |
| 2 | 错误码 40034/40054/40055/40057 等为**近似值**（集中在 `crates/mqs/src/error.rs`） | 依赖具体 code 的分支可能需要调整 | 真机抓包后改一张表即可；HTTP 状态按 HTTP 语义给 |
| 3 | 群消息事件由群内 Bot 的「全量消息」开关（`group_bots.recv_msg_setting`）决定：`all`（默认）时**所有**群消息都以 `GROUP_MESSAGE_CREATE` 下发，@ 只由 `mentions[].is_you` 表示、`content` 不带前缀；`only_mention` 时只下发 @ 消息的 `GROUP_AT_MESSAGE_CREATE`（`content` 带 `<@!openid>` 前缀） | 只认 `GROUP_AT_MESSAGE_CREATE` 的老 SDK（如 botpy 1.2.1）在全量消息开启时收不到 @ 消息，需要把开关关掉 | 群聊右栏「群管理」有全量消息开关；`chat.auto_at_bot` 调试开关仍可让所有群消息按 @ 处理 |
| 4 | `only_mention` 模式下 Bot 收到的群 @ 消息 `content` 带 `<@!openid>` 前缀 | 直接回显会把 @ 标记带出去 | 与真实 @ 语义一致；SDK 侧建议 `re.sub(r"<@![^>]+>", "", content)` |
| 5 | 未显式给 `msg_seq` 时**自动递增** | 比真实平台宽松（真实平台默认 1，重复会失败） | 显式传 `msg_seq` 时严格去重（40054） |
| 6 | `file_type=4`（普通文件）不支持 | 上传普通文件会 400 | 官方也标注“暂不开放” |
| 7 | 被动回复窗口 / 配额 / token 过期受**虚拟时钟**控制 | 快进时钟后 token 也会过期 | 这是设计目标，不是差异 |
| 8 | Webhook Ed25519 签名与 `plain_token` 握手按公开资料实现 | 与真机回调校验可能有细节偏差 | `tests/webhook.rs` 验证自洽性；真机需再对一次 |
| 9 | 撤回只标记 `recalled_at`，不推 `PUBLIC_MESSAGE_DELETE` 事件（已实现官方 2 分钟限制） | 依赖撤回事件的 Bot 收不到通知 | 覆盖等级 `partial` |
| 10 | 分片只做事件归属过滤，不做“多连接并发上限” | 压测多分片时行为偏宽松 | 官方限制在文档里没有精确数值，沙箱不模拟 |
| 11 | 错误体同时输出 `code` 与 `err_code`（同值） | 无影响，纯加字段 | 官方 token 接口用 `code`、message 类接口用 `err_code`，两者都兼容 |
| 12 | 被动回复除 `msg_id` 外也接受 `event_id`（按钮回调等事件） | 无影响 | 官方行为；沙箱未校验 event 白名单，比真机宽松 |
| 13 | `INTERACTION_CREATE.chat_type`：群=1、单聊=2（官方 0=频道） | 频道不在本产品内 | 与官方数值一致 |
| 14 | Markdown 文本交互标签（`<qqbot-at-user>`/`<qqbot-at-everyone>`/`<qqbot-cmd-*>`）由 GUI 模拟：`at_users` 是控制 API 返回的额外字段，点击指令走 `POST /_mqs/messages/{id}/cmd` | 直接断言 REST 事件的 `mentions` 不会因 Markdown 标签自动补齐；标签只在 GUI 里可见/可点 | 官方由客户端解析并渲染；沙箱把 openid→昵称放进 `at_users`，推送语义（群里 @Bot、`reference` 引用）与控制 API 保持一致 |

## 端口与域名假设

- botpy 默认 `is_sandbox=False`，垫片同时改 `DOMAIN` 和 `SANDBOX_DOMAIN`，两种都指向沙箱。
- access token 端点挂在**模拟端口**上（`POST /app/getAppAccessToken`），不是单独域名；垫片负责改地址。
- 网关 WebSocket 路径固定 `/websocket`（同时接受 `/websocket/`）。
- 媒体下载在 `/media/<sha256>`，`file_info` 形如 `mqs-file-<sha256>`。

## 回归方式

```bash
cargo test -p mqs                                   # 单元 + 集成（98 项）
cargo run -q -p mqs -- serve                        # 或 ./target/debug/mqs serve
MQS_BASE_URL=http://127.0.0.1:5700 python examples/botpy_echo/bot.py
mqs script run smoke                                # 需要上面这个 echo bot 在线
```
