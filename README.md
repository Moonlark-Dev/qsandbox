# MQS — Moonlark QSandbox

> QQ Bot API v2 的本地沙箱：把 `base_url` / `ws_url` 指过来，Bot 就能跑完整收发。
> **非官方项目，与腾讯无任何关联。** 所有账号、openid、token 都是 MQS 生成的假数据。

MQS 是给 [Moonlark](https://github.com/Moonlark-Dev/Moonlark) 生态和 QQ Bot 开发者用的"平台替身"：不需要 AppID 权限、不会触发真机频控、世界可复现、全程可观测。设计文档见 [PLAN.md](./PLAN.md)。

```
mqs serve  →  Simulate API :5700   (QQ 兼容 /v2/*、网关 WebSocket)
           →  Control API  :5701   (GUI / CLI / MCP，token + Origin 保护)
           →  SQLite + media       (单进程、WAL、内容哈希寻址)
```

## 快速开始

```bash
# 1. 安装（Python 3.11+）
python3 -m venv .venv && . .venv/bin/activate
pip install -e ".[dev]"

# 2. 启动（首次自动建一个测试 Bot / 用户 / 群）
mqs serve
#    Simulate API : http://127.0.0.1:5700
#    Gateway      : ws://127.0.0.1:5700/websocket
#    Control GUI  : http://127.0.0.1:5701/#t=...   ← 浏览器打开
```

默认测试 Bot：

| 字段 | 值 |
|---|---|
| appid | `10000` |
| secret | `mqs-secret-10000` |
| token | `POST /app/getAppAccessToken` 获取，或 `mqs token` |

### 把 botpy 指过来

botpy 的 `Route.DOMAIN` / token URL 是硬编码的，用仓库自带的垫片：

```python
import mqs.compat.botpy  # noqa: F401  ← 必须在 import botpy 之前
```

完整 echo bot 见 [examples/botpy_echo/bot.py](./examples/botpy_echo/bot.py)，已知差异见 [docs/COMPAT.md](./docs/COMPAT.md)。

### 不用写代码，先玩一下

```bash
mqs group create --name 测试群 --members 3
mqs send --as-user u_000001 --to group:g_000002 --text "你好"
mqs expect --event group_at_message_create --timeout 3s
mqs script run smoke            # 内置场景（需要 echo bot 在线）
mqs script run rate_limit       # 验证 strict 限流：第 3 次 429
mqs status --json
```

## 能力矩阵（v0.1）

| 层 | 已实现 |
|---|---|
| 鉴权 | `getAppAccessToken`、`Authorization: QQBot <token>`、7200s 过期、token 失效/过期错误码 |
| 网关 | hello / identify / ready / heartbeat / resume / invalid session、intents 位掩码过滤、分片、断线补发 |
| 群/C2C | 收发文本/markdown/keyboard/ark/embed/媒体、被动回复 5 分钟 + 5 次、`msg_id+msg_seq` 去重、引用、撤回 |
| 媒体 | `file_type` 1/2/3、multipart 与 url 上传、`file_info` 引用、内容哈希下载 |
| 事件 | WebSocket 与 Webhook 双投递、URL 校验握手（Ed25519）、事件溯源 |
| 保真 | QQ 风格错误体 `{code,message,trace_id}`、`X-Tps-Trace-ID`、令牌桶限流三档 |
| 可观测 | 双向 HTTP/WS 帧、事件流、限流拒绝、DB 大小、覆盖率实时统计 |
| Agent | CLI（`--json` / 稳定退出码）、MCP stdio server、YAML 场景 + JUnit/TAP |
| GUI | 状态 / 配置 / 聊天 / 调试器 四页，账号切换快捷键 |

覆盖率实时数字：`GET /_mqs/contract`，或看 [docs/COVERAGE.md](./docs/COVERAGE.md)（`mqs contract export` 生成）。

## 目录结构

```
src/mqs/
  api/           模拟 QQ API + 网关 WebSocket
  control/       控制 API（GUI/CLI/MCP 共用）
  domain/        世界模型、事件总线、限流、webhook
  gateway/       网关协议状态机
  contract/      端点注册表 + 覆盖率/TS 生成
  scenario/      YAML 场景引擎
  compat/        botpy 垫片
  cli.py         mqs 命令行
  mcp_server.py  MCP server
web/             Vue 3 + Vite 前端
tests/           pytest（契约/鉴权/网关/消息/限流/场景）
```

## 开发

```bash
# 后端
.venv/bin/pytest
# 前端
cd web && npm install && npm run dev    # 代理到 :5701
cd web && npm run build                 # 产物由控制 API 托管
# 契约生成物
mqs contract export
```

## 安全

- 默认只听 `127.0.0.1`；`--host 0.0.0.0` 需要显式 `--token`，并会在 stderr 警告。
- 控制 API 要求 token 或 HttpOnly cookie；校验 `Origin`；前端用 URL fragment 里的一次性 nonce 换 cookie（fragment 不会进服务器日志）。
- 不做转发到真实 QQ 的代理；不落任何真实用户数据；仓库不含真实 secret。

## 许可证

AGPL-3.0-or-later
