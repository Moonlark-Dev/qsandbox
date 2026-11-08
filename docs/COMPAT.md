# SDK 兼容性与已知差异

> 文档快照：2026-11-07 · 对应 MQS v0.1.0

## 已实测

| SDK / 客户端 | 结果 | 说明 |
|---|---|---|
| **botpy（qq-botpy）** | ✅ 通过 | 2026-11-07 用 `examples/botpy_echo/bot.py` 实测：登录、网关鉴权、心跳、群 @ 收消息、被动回复全链路跑通 |
| MQS 内置 websockets 客户端 | ✅ 通过 | `tests/test_gateway.py`、`test_scenario.py` 覆盖 identify/heartbeat/resume/intents |
| curl / httpx | ✅ 通过 | 所有 REST 端点都有契约测试 |

Koishi / NoneBot / qq-bot-sdk 尚未实测（需要它们提供 base_url/ws_url 配置或各自的补丁）。

## botpy 垫片

botpy 没有 `base_url` 配置：`botpy/http.py:Route` 把 `https://api.sgroup.qq.com` 写死，`botpy/robot.py:Token.update_access_token` 把 `https://bots.qq.com/app/getAppAccessToken` 写死。因此：

```python
import mqs.compat.botpy   # 必须在创建 botpy.Client 之前 import
```

它会：

1. 把 `Route.SCHEME/DOMAIN/SANDBOX_DOMAIN` 指向 `MQS_BASE_URL`（默认 `http://127.0.0.1:5700`）；
2. 替换 `Token.update_access_token`，改请求 MQS 的 `POST /app/getAppAccessToken`。

网关地址不需要改：`GET /gateway/bot` 返回基于请求 Host 的 `ws://<host>:<port>/websocket`，botpy 直接使用。

其他 SDK 只要支持自定义 REST domain + token URL，或用同样的方式打补丁即可。

## 已知差异（按影响排序）

| # | 差异 | 影响 | 处理 |
|---|---|---|---|
| 1 | 错误码 40034/40054/40055/40057 等为**近似值**（集中在 `src/mqs/errors.py`） | 依赖具体 code 的分支可能需要调整 | 真机抓包后改一张表即可；HTTP 状态码按 HTTP 语义给 |
| 2 | 频道（guild）家族仅 **stub**：只返回结构，不做频道内事件/权限 | 频道机器人无法真正联调 | 见覆盖率表 §L2；v0.2 再做 |
| 3 | 群消息事件由该群里的「全量消息」开关（`bot_state.recv_msg_setting`）决定，与真实平台一致：`all`（默认）时**所有**群消息——含 @ 该 Bot 的——都以 `GROUP_MESSAGE_CREATE` 下发，@ 只由 `mentions[].is_you` 表示、`content` 不带前缀；`only_mention` 时只下发 @ 消息的 `GROUP_AT_MESSAGE_CREATE`（`content` 带 `<@!openid>` 前缀），普通消息不投递。两者共用 intent `1<<25` | 只认 `GROUP_AT_MESSAGE_CREATE` 的老 SDK（如 botpy 1.2.1）在全量消息开启时收不到 @ 消息，需要把开关关掉 | 群聊侧边栏「群管理」有全量消息开关（写 `recv_msg_setting`）；控制 API 的 `chat.auto_at_bot` 仍可让所有群消息都按 @ 处理 |
| 4 | `only_mention` 模式下 Bot 接收的群 @ 消息 `content` 带 `<@!openid>` 前缀 | 直接回显会把 @ 标记带出去 | 与真实 @ 语义一致；SDK 侧建议 `re.sub(r"<@![^>]+>", "", content)` |
| 5 | 未显式给 `msg_seq` 时**自动递增** | 比真实平台宽松（真实平台默认 1，重复会失败） | 显式传 `msg_seq` 时严格去重（40054） |
| 6 | `file_type=4`（普通文件）不支持 | 上传普通文件会 400 | 官方也标注“暂不开放” |
| 7 | 被动回复窗口 / 配额受**虚拟时钟**控制 | 快进时钟后 token 也会过期 | 这是设计目标，不是差异 |
| 8 | Webhook Ed25519 签名与 `plain_token` 握手按公开资料实现 | 与真机回调校验可能有细节偏差 | `test_webhook.py` 验证自洽性；真机需再对一次 |
| 9 | 撤回只标记 `recalled_at`，不推 `MESSAGE_DELETE` 事件 | 依赖撤回事件的 Bot 收不到通知 | 覆盖等级 `partial` |
| 10 | 频道/论坛/音视频事件不模拟 | — | `n/a` |

## 端口与域名假设

- botpy 默认 `is_sandbox=False`，垫片同时改 `DOMAIN` 和 `SANDBOX_DOMAIN`，两种都指向 MQS。
- access token 端点挂在**模拟端口**上（`POST /app/getAppAccessToken`），不是单独域名；垫片负责改地址。
- 网关 WebSocket 路径固定 `/websocket`（同时接受 `/websocket/`）。
- 媒体下载在 `/media/<sha256>`，`file_info` 形如 `mqs-file-<sha256>`。

## 回归方式

```bash
pytest tests/                      # 90 项，含网关/消息/限流/频道/场景/CLI/模糊测试
MQS_BASE_URL=http://127.0.0.1:5700 python examples/botpy_echo/bot.py
mqs script run smoke               # 需要上面这个 echo bot 在线
```

## 2026-11-07 第二轮补充

- 品牌：程序内显示 **QSandbox**（`mqs` 仍是 CLI 命令名，`MQS_`/`mqs-` 仍是假凭据前缀）。
- 被动消息窗口按场景区分：单聊 **60 分钟 / 每条 4 次**，群聊 **5 分钟 / 每条 5 次**，
  频道 **5 分钟**（次数不限）。
- 主动消息频控严格按官方“频率与时效规则”：单聊未认证 5/s & 30/min、已认证 10/s；
  群聊未认证 30/min、已认证 60/min；单关系 20/min；每日 1000/用户或群。
  个人认证与企业认证限额一致，已合并为 `certified` 一档。
- L2 频道家族已从 stub 升级为实现：默认 profile 带一个 “QSandbox 测试频道”，
  支持 guild/channel/member/role 查询、频道消息收发的 `MESSAGE_CREATE` / `AT_MESSAGE_CREATE`。
- 桌面端：按用户决定由 Tauri 改为 **Electron**（`desktop/`），sidecar 用 **Nuitka standalone**
  （onedir，安装包里是 `resources/sidecar/mqs`）；AppImage 已在 Xvfb 下实测启动/退出。
  早期用 onefile，它每次启动要把 ~200 MiB 解包到 `TMPDIR`，tmpfs 满时 sidecar 直接 exit 1，
  桌面端只能报 “QSandbox 启动失败”，因此改回 standalone。

## 接口白名单（内邀接口）

QQ 的“白名单”指**接口访问白名单**，不是 IP 白名单。例如：

- `GET /v2/groups/{group_openid}/members`（60 QPM）
- `GET /v2/groups/{group_openid}/members/{member_openid}`（30 QPM）

官方文档标注“该能力正在内邀接入中”，非白名单应用调用会返回 **11253 应用无接口访问权限**。
QSandbox 用两个独立开关模拟：

| 设置 | 作用 |
|---|---|
| `feature.whitelist` | 开启时，Bot 必须 `whitelisted=true` 才能调用内邀接口，否则 403/11253 |
| `feature.expose` | `public` = 内邀接口整体 404；`all` = 挂出来 |

Bot 的 `whitelisted` 可在配置页切换（或 `mqs bot whitelist <app_id> on`）。

## nonebot-adapter-qq

配置页“Bot → 接入信息”会显示并允许复制：

```dotenv
QQ_API_BASE=http://127.0.0.1:5700/
QQ_AUTH_BASE=http://127.0.0.1:5700/app/getAppAccessToken
QQ_CUSTOM_GATEWAY_URL=ws://127.0.0.1:5700/websocket
QQ_BOTS=[{"id":"10000","token":"MQS_...","secret":"mqs-secret-10000","intent":{...},"use_websocket":true}]
```

对应 `nonebot-adapter-qq` 的 `Config` 字段：`qq_api_base` / `qq_auth_base` /
`qq_custom_gateway_url` / `qq_bots`。

注意版本差异：`nonebot-adapter-qq` **1.0–1.6.x 的 `BotInfo` 要求 `token` 字段**
（否则报 `qq_bots.0.token Field required`）；1.7.3+ 改为只用 `secret` 换 token，
多余 `token` 字段会被忽略。所以 QSandbox 生成的 `QQ_BOTS` 同时带 `token` 与 `secret`，
两端都能解析。token 有效期 7200 秒，过期后重新打开“接入信息”弹窗获取。

群消息事件（`GROUP_AT_MESSAGE_CREATE` / `GROUP_MESSAGE_CREATE`）的 `author` 带
`bot` 与 `member_role`，`mentions` 按 `GroupMentionUser`（`scope="single"` +
`bot` / `is_you` / `username`）下发；被 @ 的 Bot 也会出现在 `mentions` 里（`bot=true`，
对收到事件的 Bot 自己 `is_you=true`，对同群其它 Bot `is_you=false`）。这是
`nonebot-adapter-qq` **1.7.2+ 的 `GroupMemberAuthor` / `GroupMention` 必需字段**
（1.7.2 之前没有这些字段，会忽略多余的键），与真实平台事件一致。缺字段时适配器会直接报
`author.bot Field required` 之类的校验错误。

「全量消息」按真实语义工作：`recv_msg_setting=all`（默认）的 Bot 收到的**每一条**群消息
都是 `GROUP_MESSAGE_CREATE`——包括 @ 它的消息，`content` 不带 `<@!openid>` 前缀，@ 由
`mentions[].is_you=true` 表示；`only_mention` 时只收 @ 消息的 `GROUP_AT_MESSAGE_CREATE`
（带前缀），普通消息不推。`mention_and_context` 目前按 `only_mention` 处理（沙箱不模拟上下文消息）。
botpy 1.2.1 只注册了 `GROUP_AT_MESSAGE_CREATE` 解析器，用 `examples/botpy_echo` 联调时
需要把该群的全量消息关掉，否则它只会打印 `_parser unknown event group_message_create`（连接不会断）。
