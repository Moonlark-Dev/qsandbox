# Moonlark QSandbox

[![CI](https://github.com/Moonlark-Dev/qsandbox/actions/workflows/ci.yml/badge.svg)](https://github.com/Moonlark-Dev/qsandbox/actions/workflows/ci.yml)
[![Release](https://github.com/Moonlark-Dev/qsandbox/actions/workflows/release.yml/badge.svg)](https://github.com/Moonlark-Dev/qsandbox/actions/workflows/release.yml)

Moonlark QSandbox 一个是 QQ Bot API 沙箱，您可以在此随意创建虚拟用户，并对您的 Bot 进行测试。

## 架构一览

```
QSandbox 服务
  →  Simulate API :5700   (QQ 兼容 /v2/*、网关 WebSocket)
  →  Control API  :5701   (GUI / CLI / MCP，token + Origin 保护)
  →  SQLite + media       (单进程、WAL、内容哈希寻址)

Electron 桌面端（desktop/） → 拉起 sidecar → 内嵌同一份 Vue GUI
CLI / MCP                  → 共用同一个控制 API，无第二套实现
```

![状态页](docs/screenshots/status.png)

## 快速开始

```bash
# 1. 安装（Python 3.11+）
python3 -m venv .venv && . .venv/bin/activate
pip install -e ".[dev]"

# 2. 启动（首次自动建 1 个可操作账号 + 3 个 npc + 1 群 + 1 频道）
mqs serve
#    Simulate API : http://127.0.0.1:5700
#    Gateway      : ws://127.0.0.1:5700/websocket
#    Control GUI  : http://127.0.0.1:5701/#t=...   ← 浏览器打开即可预览
```

**默认测试 Bot**

| 字段 | 值 |
|---|---|
| appid | `10000` |
| secret | `mqs-secret-10000` |
| token | `POST /app/getAppAccessToken` 获取，或 `mqs token` |

## 使用方式

### 使用 botpy

botpy 的 `Route.DOMAIN` / token URL 是硬编码的，使用仓库自带的垫片：

```python
import mqs.compat.botpy  # noqa: F401  ← 必须在 import botpy 之前
```

完整 echo bot 示例见 [examples/botpy_echo/bot.py](./examples/botpy_echo/bot.py)，已知差异见 [docs/COMPAT.md](./docs/COMPAT.md)。

### 命令行使用

```bash
mqs group create --name 测试群 --members 3
mqs send --as-user u_000001 --to group:g_000002 --text "你好"
mqs expect --event group_at_message_create --timeout 3s
mqs script run smoke            # 需要 echo bot 在线
mqs script run rate_limit       # 未认证个人单聊 5/s：第 6 条 429
mqs script run media            # multipart 上传 + msg_type=7
mqs status --json
```

### 桌面端（Electron）

```bash
cd web && npm install && npm run build
cd .. && pip install -e ".[build]" && python tools/build_sidecar.py   # Nuitka standalone -> build-sidecar/mqs.dist
cd desktop && npm install && npm run dist:linux   # release/QSandbox-*.AppImage
# 或开发模式：npm start（自动拉起 build-sidecar/mqs.dist/mqs）
```

安装包自带编译好的 sidecar（`resources/sidecar/mqs`，Nuitka standalone），用户机器**不需要**装 Python。
CI 会在三平台各跑一次 `tools/smoke_sidecar.py`，并解包检查 `tools/verify_desktop_bundle.py`，确认 mqs 真的进了安装包。

桌面端数据放在 `app.getPath('userData')/mqs-home`，不污染仓库内的 profile。更多见 [desktop/README.md](./desktop/README.md)。

## 频率与时效（严格对齐 QQ 官方文档）

| 类型 | 场景 | 规则 |
|---|---|---|
| 被动 | 单聊 | 60 分钟 / 每条消息 4 次回复 |
| 被动 | 群聊 | 5 分钟 / 每条消息 5 次回复 |
| 主动 | 单聊 · 未认证 | Bot 5/s 且 30/min；单关系 20/min；1000/天/用户 |
| 主动 | 单聊 · 已认证 | Bot 10/s；单关系 20/min；1000/天/用户 |
| 主动 | 群聊 · 未认证 | Bot 30/min；单关系 20/min；1000/天/群 |
| 主动 | 群聊 · 已认证 | Bot 60/min；单关系 20/min；1000/天/群 |
| 频道 | 所有消息 | 5/s/频道；主动 20/天/频道、2 频道/天 |

个人认证与企业认证限额完全一致，UI 合并为 **关 / 未认证个人 / 已认证（个人=企业）** 三档。
实现采用滑动窗口（非令牌桶），429 响应携带 `Retry-After` 与 `X-RateLimit-*`。

## 能力矩阵（v0.2）

| 层 | 已实现 |
|---|---|
| **鉴权** | `getAppAccessToken`、`Authorization: QQBot <token>`、7200s 过期、错误码 |
| **网关** | hello / identify / ready / heartbeat / resume / invalid session、intents、分片、断线补发 |
| **群 / C2C** | 文本/markdown/keyboard/ark/embed/媒体、被动窗口与次数、`msg_id+msg_seq` 去重、引用、撤回、按钮点击（指令/回调/链接）；**未选择 Bot 时默认不 @ 任何 Bot** |
| **群管理** | 群基本信息、机器人群内状态（主动消息开关）、成员禁言（add/update/del）、黑名单、入群申请列表与审批、批量移除 |
| **频道 L2** | guild / channel / member / role、频道消息、`MESSAGE_CREATE` / `AT_MESSAGE_CREATE`、频道频控；**子频道创建/改名/删除**、身份组 CRUD 与成员分配、成员禁言/踢出、全员禁言 |
| **频道私信** | `POST /users/@me/dms` 建会话、`/dms/{guild_id}/messages` 收发与撤回、`DIRECT_MESSAGE_CREATE`、2 条/天/用户 + 200 条/天/Bot 主动限额 |
| **好友** | `FRIEND_ADD` / `FRIEND_DEL` 事件、好友列表与关系校验（可选主动消息前置条件） |
| **用户富媒体** | 控制台上传图片/视频 → 群/单聊/频道/频道私信附件，支持文件选择、粘贴、拖拽，`msg_type=7` |
| **主动消息** | Bot 级 + 群级开关；关闭后主动发送返回 40040，被动回复不受影响 |
| **媒体** | `file_type` 1/2/3、url / multipart / base64 `file_data` / 分片上传（prepare → part → finish）、内容哈希下载 |
| **事件** | WebSocket 与 Webhook 双投递、Ed25519 握手、事件溯源 |
| **可观测** | 双向 HTTP/WS 帧、事件流、限流拒绝、吞吐、DB 大小、覆盖率实时统计 |
| **Agent** | CLI（`--json` / 稳定退出码）、MCP stdio（10 个工具）、10 个 YAML 场景 + JUnit/TAP |
| **GUI** | Electron 桌面 + 浏览器预览：状态/配置/聊天/频道/调试器/设置，规则引擎、群管理、频道角色与私信、私聊 Bot、账号切换 |
| **覆盖率** | 53 条端点：53 已实现（50 full + 3 partial），0 stub |

实时覆盖率查询：`GET /_mqs/contract`，或查看 [docs/COVERAGE.md](./docs/COVERAGE.md)（由 `mqs contract export` 生成）。

## 目录结构

```
src/mqs/
  api/           模拟 QQ API + 网关 WebSocket
  control/       控制 API（GUI/CLI/MCP 共用）
  domain/        世界模型、事件总线、限流（官方频率政策）、webhook
  gateway/       网关协议状态机
  contract/      端点注册表 + 覆盖率/TS 生成
  scenario/      YAML 场景引擎
  compat/        botpy 垫片
  cli.py         mqs 命令行
  mcp_server.py  MCP server
web/             Vue 3 + Vite 前端（Electron 与浏览器共用）
desktop/         Electron 主进程 + electron-builder
tools/           sidecar 构建脚本
tests/           pytest（契约/鉴权/网关/消息/限流/媒体/频道/场景/CLI/模糊测试）
```

## 开发

```bash
# 后端
.venv/bin/pytest        # 106 项
.venv/bin/ruff check src tests

# 前端
cd web && npm install && npm run dev    # 代理到 :5701
cd web && npm run build                 # 产物由控制 API / Electron 托管

# 契约生成物
mqs contract export
```

## 安全

- 默认只监听 `127.0.0.1`；`--host 0.0.0.0` 需显式提供 `--token`，并在 stderr 警告。
- 控制 API 要求 token 或 HttpOnly cookie；校验 `Origin`；前端通过 URL fragment 中的一次性 nonce 换取 cookie。
- 可选接口白名单（`feature.whitelist`，内邀接口返回 11253）/ scope 校验 / 媒体路径穿越防护；`feature.expose=public` 可将内邀接口整体隐藏（404）。
- 不做转发到真实 QQ 的代理；不落地任何真实用户数据；仓库不含真实 secret。

## 已知未做

- 钥匙串存储
- `--record` 可重放 fixture
- Koishi/NoneBot/qq-bot-sdk 兼容矩阵
- 频道角色权限位判定
- 论坛/音视频/日程/公告等频道内容接口

## 许可证

AGPL-3.0-or-later

> [!WARNING]
> 该项目与腾讯无任何关联。所有账号、openid、token 均为 QSandbox 随机生成的数据。
