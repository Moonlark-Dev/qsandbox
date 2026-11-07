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
| 3 | 群消息事件只有 `GROUP_AT_MESSAGE_CREATE`；群内未 @ Bot 的消息不投递 | 与真实 QQ 群机器人一致（群机器人只能收 @ 消息） | 控制 API 有 `chat.auto_at_bot` 开关方便 GUI 调试 |
| 4 | Bot 接收的群消息 `content` 带 `<@!openid>` 前缀 | 直接回显会把 @ 标记带出去 | 与真实 @ 语义一致；SDK 侧建议 `re.sub(r"<@![^>]+>", "", content)` |
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
pytest tests/                      # 81 项，含网关/消息/限流/频道/场景/CLI/模糊测试
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
- 桌面端：按用户决定由 Tauri 改为 **Electron**（`desktop/`），sidecar 用 PyInstaller onefile；
  AppImage 已在 Xvfb 下实测启动/退出。
