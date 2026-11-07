# Moonlark QSandbox（MQS）实施计划 v0.2

> 本文是 `qsandbox.txt`（v0.1 草案）的落地版计划：把目标拆成可交付的里程碑，把草案里"看起来对但有坑"的地方改掉，并给出与代码一一对应的协议细节。
>
> 状态：v0.2（2026-11-07）· 对应代码 `src/mqs/`、`web/`

---

## 0. 与草案的差异（先看这里）

| # | 草案 | 实际做法 | 原因 |
|---|---|---|---|
| D1 | SQLAlchemy 2.0 + Alembic | **aiosqlite + `schema.sql` + `PRAGMA user_version` 迁移** | 单写进程、单文件库；本项目要大量"部分更新列 + 原子 seq 自增"，ORM 在这里只增加一层翻译。迁移只有 3 个函数，Alembic 的版本图完全用不上。后续 schema 真开始频繁变动再引入。 |
| D2 | `uvicorn` 两个 listener 同进程 `gather` | 同进程两个 `uvicorn.Server`，共享一个 `World`/`DB`，统一 `asyncio` 生命周期 | 保留草案结论；补充实现细节：两个 server 各自 `serve()`，主协程等两者，收到 SIGINT 时同时 `should_exit`。 |
| D3 | 控制 API token 放启动握手 | token 只出现在 **stdout 握手行 / `runtime.json` / 请求头**；前端用 **URL fragment 一次性 nonce 换 HttpOnly cookie** | 草案已说"不进 URL query"；fragment 不会进服务器日志/Referer，nonce 一次性且 60s 过期。 |
| D4 | 前端独立 dev server | 生产由控制 API 静态托管 `web/dist`，开发用 `vite dev` 代理到控制 API | 一个端口，避免 CORS/token 二套配置。 |
| D5 | "全量模拟 QQ Bot API v2" | **契约清单驱动**：v0.1 交付 L0 + L1 的核心 31 条端点；频道家族（L2）标 `stub`，控制台/未公开接口标 `n/a` | 全量不可交付。覆盖等级写进注册表，状态页实时统计，避免"永远做不完"变成"看起来做完了"。 |
| D6 | botpy 直接改 base_url | botpy 的 `Route.SCHEME/DOMAIN` 和 token URL 是硬编码的，**必须 monkeypatch**；因此提供 `mqs.compat.botpy` 垫片 | SDK 不提供 base_url 配置（已核对 botpy 源码：`botpy/http.py:Route`、`botpy/robot.py`）。把垫片放进仓库比写"请改 SDK 源码"靠谱。 |
| D7 | 网关 `s` 序号 | 使用 **全局事件序号**（每个 Bot 一条投递记录），RESUME 按 `seq > last_seq` 补发 | 与草案"事件溯源"一致，且缺口对客户端无影响（botpy 只记 max）。 |
| D8 | 被动回复去重 | `msg_id + msg_seq` 显式去重；**未显式给 `msg_seq` 时自动递增** | 真实平台是"相同 msg_id+msg_seq 重复发送失败"。但 botpy 的 `msg_seq` 默认值是 1，若严格按默认值，一条消息的第二次回复必失败——自动递增更接近 SDK 作者的实际预期，也保留显式去重分支。 |
| D9 | 媒体 `file_type` 1/2/3/4 | v0.1 支持 1（图片）/2（视频）/3（语音），4（文件）返回 `partial` 错误 | 官方文档标注"文件（暂不开放）"。 |
| D10 | 桌面 Tauri | **改用 Electron**（用户 2026-11-07 决定），`desktop/` 为主进程壳，Vue 前端不变 | Tauri 的运行时与打包器是 Rust，本机无 Rust 工具链；用户明确选择纯 TS/Node 的 Electron。发行物从 “Tauri + Server” 改为 “Electron + Server sidecar”。 |

---

## 1. 里程碑

| 里程碑 | 内容 | 验收 |
|---|---|---|
| **M0 骨架** | 工程结构、profile、DB、错误模型、时钟、种子随机 | `pytest tests/test_foundation.py` 绿 |
| **M1 协议 L0** | 双端口、AccessToken、群/C2C 收发、网关全流程（identify/heartbeat/resume/intents）、事件落库 | botpy echo bot 能收能回；RESUME 补发测试通过 |
| **M2 协议 L1** | markdown/keyboard/ark/embed/media、引用、撤回、msg_seq 去重、被动 5 分钟/5 次窗口、限流 | `tests/test_messages.py`、`tests/test_gateway.py` 绿 |
| **M3 控制面** | 控制 API（用户/群/Bot/成员/会话/消息/调试帧/时钟/规则/场景/快照）、控制 WS | 状态页能显示覆盖率和实时吞吐 |
| **M4 Agent 面** | CLI（`--json`/退出码）、MCP server、YAML 场景 + JUnit、`mqs doctor` | `mqs script run scenarios/smoke.yaml --junit` 在 CI 里过 |
| **M5 前端** | Vue 3 四页（状态/配置/聊天/调试器）+ 账号切换快捷键 | 浏览器里完成"建群→拉用户→发消息→看调试器"闭环 |

v0.1 验收（写死，对应草案 §9）：
1. 用 botpy 写 echo bot，走 `mqs.compat.botpy` 垫片指向 MQS，能收到群 @ 消息并回复；
2. `mqs script run scenarios/smoke.yaml` 一条命令完成回归并输出 JUnit。

---

## 2. 进程、端口与生命周期

```
mqs serve --profile dev --host 127.0.0.1 --port 0 --control-port 0
        │
        ├── Simulate API  (默认 5700)   /v2/*  /gateway  /gateway/bot  /app/getAppAccessToken  /media/*
        │      · 允许任意 Origin（SDK 不管 CORS）
        │      · 鉴权：Authorization: QQBot <app_access_token>
        │
        ├── Control API   (默认 5701)   /_mqs/*  /  (前端 dist)
        │      · X-MQS-Token 头 或 mqs_session cookie
        │      · Origin 校验（只允许 loopback / 同端口）
        │
        └── 共享：World + EventBus + GatewayManager + 虚拟时钟 + SQLite(WAL)
```

- **启动握手**：`mqs serve` 在监听成功后向 stdout 打一行 JSON：
  `{"mqs":1,"profile":"dev","sim_port":5700,"control_port":5701,"token":"...","base_url":"http://127.0.0.1:5700","ws_url":"ws://127.0.0.1:5700/websocket"}`
  同时写 `profiles/dev/runtime.json`（含 pid），CLI 靠它发现端口/token。
- **文件锁**：`profiles/<name>/profile.lock` 用 `fcntl.flock(LOCK_EX|LOCK_NB)`；第二个 `mqs serve` 直接报错退出码 4。
- **默认只绑 127.0.0.1**；`--host 0.0.0.0` 时 stderr 打醒目警告，且要求显式 `--token`（不接受自动生成）。
- **信号**：SIGINT/SIGTERM → 停两个 uvicorn → 关 WS → 关 DB → 删锁。

---

## 3. 数据模型（`src/mqs/schema.sql`）

草案 §4 的表全部保留，落地时补了 4 张表和 2 个约束：

```sql
profiles         -- 只在 profile.json，不入库
bots             -- id, app_id, secret, name, avatar, scopes(json), delivery(ws|webhook), created_at
users            -- id, nickname, avatar, gender, age, city, actor_type(human|npc|bot), seed, behavior, created_at
groups           -- id, name, owner_id, join_policy, created_at
group_members    -- (group_id, user_id), role, joined_at, mute_until
group_bots       -- (group_id, bot_id), joined_at          ★ 新增：Bot 在群里才收事件
conversations    -- id, type(dm|group), peer_id, bot_id, last_seq
openid_map       -- (bot_id, user_id), openid, union_openid   ★ 按 Bot 隔离（草案要求）
messages         -- id, conversation_id, seq, msg_id, event_id, direction(in|out), sender_id,
                 -- sender_type(user|bot), bot_id, msg_type, content, raw_json, sent_at
events           -- seq, type, payload_json, conversation_id, target_bot_id, created_at  ★ 每 Bot 一条投递记录
gateway_sessions -- session_id, bot_id, intents, last_seq, created_at, closed_at
app_tokens       -- token, bot_id, expires_at
media            -- hash, mime, size, path, created_at
rate_buckets     -- key, tokens, updated_at
reply_windows    -- (msg_id, bot_id), count, expires_at            ★ 被动回复窗口
reply_keys       -- (bot_id, msg_id, msg_seq)                       ★ 显式去重
active_quota     -- (bot_id, conversation_id, day), used            ★ 主动消息配额
frames           -- id, ts, direction(in|out), kind(http|ws), bot_id, session_id, summary, data_json
rules            -- id, event_type, condition_json, action_json, delay_ms, enabled
settings         -- key, value
```

**事件溯源规则**：所有世界变更先写 `events`（同一事务）再广播。`messages.seq` 是**会话内**单调序号（`conversations.last_seq` 原子自增）；`events.seq` 是全局自增，网关 `s` 用它。

**两条草案铁律**（代码里各有一个测试）：
1. "群内新建不可操作用户"= 全局 `users(actor_type=npc)` + `group_members`，同事务；`GET /_mqs/users?available=true` 只返回 `human`。
2. `openid` 只存在于 `openid_map`，同一 user 在两个 Bot 下拿到不同 openid。

---

## 4. 协议保真（P0，逐项测）

### 4.1 鉴权

- `POST /app/getAppAccessToken`，body `{"appId":"10000","clientSecret":"mqs-secret-10000"}` → `{"access_token":"MQS_...","expires_in":"7200"}`。
- 错误：appid 不存在/secret 不匹配 → HTTP 401 `{"code":11244,"message":"invalid app secret"}`。
- 访问 API：`Authorization: QQBot <token>`；token 过期 → 401 `code=11242`；无效 → 401 `code=11241`。
- token 存 `app_tokens`，受虚拟时钟控制（`clock advance 2h` 后过期，可测刷新分支）。

### 4.2 网关

握手（与 botpy 源码逐字段核对）：

| 阶段 | 方向 | 帧 |
|---|---|---|
| 连接后 | S→C | `{"op":10,"d":{"heartbeat_interval":41250}}` |
| 鉴权 | C→S | `{"op":2,"d":{"token":"QQBot ...","intents":33554432,"shard":[0,1]}}` |
| 成功 | S→C | `{"op":0,"s":1,"t":"READY","d":{"version":1,"session_id":"...","shard":[0,1],"user":{"id","username","bot":true}}}` |
| 心跳 | C→S | `{"op":1,"d":<last_s>}` → `{"op":11,"d":null}` |
| 断线恢复 | C→S | `{"op":6,"d":{"token","session_id","seq"}}` → `{"op":0,"t":"RESUMED","d":""}` + 补发 |
| 恢复失败 | S→C | `{"op":9,"d":false}`，随后 close 4004 |
| 事件 | S→C | `{"op":0,"s":N,"t":"GROUP_AT_MESSAGE_CREATE","id":"<event_id>","d":{...}}` |

- intents 位掩码：`public_messages=1<<25`、`public_guild_messages=1<<30`、`interaction=1<<26`、`guilds=1<<0`…，未订阅事件**不投递**；`intents=0` 按 botpy 行为视为 1（频道事件）。
- 心跳超时（`heartbeat_interval*2` 未收到）→ close 4009。
- 分片：`shard[0]*count + shard[1]` 决定会话事件归属，群消息按 `hash(conversation)%shard_count == shard_id` 过滤。
- 所有上下行帧写 `frames`（调试器数据源，保留最近 2000 条）。
- 支持 webhook 部署形态；`GET /_mqs/webhook/{bot_id}` 展示回调地址与 secret。

### 4.3 消息语义

- 事件：`GROUP_AT_MESSAGE_CREATE`、`C2C_MESSAGE_CREATE`、`GROUP_ADD_ROBOT`、`GROUP_DEL_ROBOT`、`FRIEND_ADD`、`MESSAGE_CREATE`（频道，stub）。
- 群 payload：`{id, content, group_openid, group_id, author:{id, member_openid, union_openid}, timestamp, msg_seq?, mentions, attachments}`。
- 单聊 payload：`{id, content, author:{id, user_openid, union_openid}, timestamp, attachments}`。
- 被动回复：`msg_id` 必须属于该 Bot 在该会话收到的消息，`now - sent_at <= 5min`，同一 `msg_id` 最多 5 次；`msg_seq` 显式重复 → `40054`。
- 引用：`message_reference.message_id` 必须存在，`ignore_get_message_error=false` 时不存在报 `40034`。
- `@` 解析：`content` 原样保留 `<@!openid>`，同时生成 `mentions`。
- 未知字段：请求体整体存 `raw_json`，响应只序列化已知字段；未知 segment 原样回传。
- 撤回：`DELETE /v2/groups/{group_openid}/messages/{message_id}` → 204；非本 Bot 消息 → `40034`。

### 4.4 错误形状

统一 `{"code": <int>, "message": "<str>", "trace_id": "<hex>"}`，响应头 `X-Tps-Trace-ID`（与 SDK 大小写不敏感）；**绝不外泄 FastAPI 的 `{"detail":...}`**（有全局 exception handler 兜底 + 测试）。HTTP 状态：参数/语义 400，鉴权 401，权限 403，不存在 404，限流 429，内部 500。

错误码表（`src/mqs/errors.py`，单一来源；标注 `partial` 的为近似值，待用真机抓包校正）：

| code | 含义 |
|---|---|
| 10001 | 参数错误 |
| 10002 | 内部错误 |
| 11241 / 11242 / 11243 / 11244 | token 无效 / 过期 / appid 无效 / secret 无效 |
| 40034 | 消息不存在或 msg_id 无效 |
| 40040 | 无操作权限 |
| 40054 | 重复回复（msg_id+msg_seq） |
| 40055 | 主动消息配额不足 |
| 40056 | 机器人不在线 |
| 40057 | 被动回复窗口过期 / 次数用尽 |

### 4.5 中间件顺序（固定）

`trace_id → token 校验 → 接口白名单（内邀）→ scope 校验 → 限流 → handler`

- 接口白名单/暴露范围/限流/scope 四个开关在配置页，映射到依赖/中间件，不散落 handler。
- 限流按 `(bot, route_class, target)` 令牌桶；三档 `strict/loose/off`；429 带 `Retry-After` 与 `X-RateLimit-*`；每次拒绝写 `frames` + 计数，状态页可见。
- 注意：UI 上"模拟腾讯接口白名单"（`feature.whitelist`，内邀接口 11253）与"沙箱暴露哪些接口"（`feature.expose`，public/all）是两个独立开关。

### 4.6 媒体

- `POST /v2/{groups/{g}|users/{u}}/files`：JSON `{file_type,url,srv_send_msg}` 或 multipart `file`。
- 返回 `{file_uuid, file_info:"mqs-file-<hash>", ttl:0}`；`file_info` 可在消息 `media.file_info` 里引用。
- 内容哈希寻址落 `<profile>/media/<sha256>`；`GET /media/<hash>` 提供下载，路径穿越防护。
- 类型/大小限制：图片 10MB、视频 30MB、语音 5MB；超限 `10001`。

---

## 5. 控制 API（`/_mqs/*`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health` | 无鉴权存活探针 |
| POST | `/session` | nonce 换 HttpOnly cookie（一次性，60s） |
| GET | `/status` | 端口、profile、运行时长、Bot 连接数、吞吐、限流拒绝、DB 大小、覆盖率 |
| GET/PATCH | `/settings` | 时钟模式、种子、限流档位、开关、语言、主题 |
| GET/POST/PATCH/DELETE | `/users` `/users/{id}` | 含 `?actor_type=human`；随机生成 `POST /users/random` |
| GET/POST/DELETE | `/groups` `/groups/{id}` | 含 `POST /groups/{id}/members`、`DELETE .../members/{uid}`、`PATCH .../members/{uid}`（role/mute） |
| GET/POST/PATCH/DELETE | `/bots` `/bots/{id}` | 含 scopes、delivery、`POST /bots/{id}/rotate-secret` |
| GET | `/conversations` `/conversations/{id}/messages` | 会话列表/消息分页 |
| POST | `/chat/send` | 以某用户身份在会话里发言（触发网关事件） |
| GET | `/debug/frames` `/debug/events` | 调试器数据（过滤：bot/session/kind/类型/错误） |
| POST | `/clock/advance` `/clock/set` | 虚拟时钟 |
| POST | `/rate/reset` | 清空令牌桶与计数 |
| GET/POST/PATCH/DELETE | `/rules` | 规则引擎 |
| GET | `/scenarios` / `POST /scenarios/run` | 内置模板 + 执行 YAML |
| POST | `/snapshot` `/restore` | profile 快照（zip） |
| GET | `/contract` `/contract/openapi.json` `/contract/types.ts` | 契约生成物 |
| WS | `/ws?ticket=` | 前端实时事件（ticket 一次性 30s） |

所有响应：成功直接返回资源；错误 `{code, message, trace_id}`；分页 `{items, next_cursor}`。

---

## 6. CLI / MCP

```
mqs serve [--profile dev] [--port 0] [--control-port 0] [--host 127.0.0.1]
mqs status [--json]
mqs token
mqs user create --name 小明 --random --seed 42
mqs user list --json
mqs group create --name 测试群 --members 10
mqs send --as-user u_1 --to group:g_1 --text "hi"
mqs expect --event group_at_message_create --timeout 3s [--content 你好]
mqs clock advance 6m | mqs clock set 2026-01-01T00:00:00
mqs script run scenarios/smoke.yaml [--junit out.xml] [--tap]
mqs logs -f [--kind http|ws]
mqs schema
mqs doctor
mqs contract export [--out docs/COVERAGE.md]
mqs profile export/import <file.mqsprofile>
mqs mcp                      # stdio MCP server
```

- 全部支持 `--json`、稳定错误 `{code,message,hint}`、非 TTY 去颜色、退出码：0 成功 / 2 用法 / 3 连接失败 / 4 端口或锁冲突 / 5 场景失败。
- MCP 工具：`get_status`、`send_as_user`、`expect_event`、`inspect_gateway`、`advance_clock`、`snapshot`、`list_conversations`、`create_random_users`。

---

## 7. 前端（Vue 3 + Vite + TS）

- 侧栏：状态 / 配置 / 聊天 / 调试器；设置右上角。
- 状态：接入信息卡（一键复制 base_url/ws_url/appid/token）、连接数、吞吐、限流拒绝、DB 大小、profile 切换。
- 配置：用户/群/Bot 三张表，随机生成器（seed 可见、可重掷），限流档位与两个"白名单"开关。
- 聊天：三栏（会话列表 / 消息流 / 操作面板）；账号切换 `Ctrl+Tab`、`Ctrl+1..9`，焦点在输入框也生效，带 HUD；成员管理、Bot 加入/移出。
- 调试器：`frames` 流（SSE/WS 实时），按会话/事件类型/错误过滤，点开看双向原始 JSON，可复制为 curl，`logs -f` 等价视图。
- 主题：深色优先，但遵循 `prefers-color-scheme`；文案 zh-CN。

---

## 8. 测试矩阵

| 类型 | 文件 | 关键断言 |
|---|---|---|
| 单元 | `test_foundation.py` | profile 锁、时钟快进、seed 可复现、错误形状、迁移幂等 |
| 契约 | `test_contract.py` | 注册表 ↔ 路由一一对应；生成物无 drift；覆盖率数字与状态页一致 |
| 鉴权 | `test_auth.py` | token 发放/过期/错误 secret；`QQBot` 头；`detail` 不外泄 |
| 网关 | `test_gateway.py` | hello→identify→ready→heartbeat→断线→resume 补发；intents 过滤；分片；心跳超时 |
| 消息 | `test_messages.py` | 用户发言→Bot 收事件；被动回复 5 分钟/5 次；msg_seq 去重；引用；撤回；未知字段 |
| 限流 | `test_ratelimit.py` | strict 档 429 + `Retry-After`；off 档不限；窗口恢复 |
| 媒体 | `test_media.py` | 上传→file_info→发消息→下载校验哈希；穿越防护 |
| 场景 | `test_scenario.py` | 内置 smoke 用内置 echo bot 跑通；JUnit 结构 |
| CLI | `test_cli.py` | `status --json` 稳定字段；退出码；`doctor` |

真实 SDK 兼容：`examples/botpy_echo/` + `docs/COMPAT.md`（垫片用法、已知差异）。CI 里跑 `test_scenario` 的 echo bot（纯 websockets 客户端模拟 botpy 行为），botpy 真机测试标 `@pytest.mark.sdk` 手动跑。

---

## 9. 安全与合规（草案 §10 落地）

- 默认 loopback；`0.0.0.0` 必须显式 token 且 stderr 警告。
- 控制端口校验 `Origin`（允许 `http://127.0.0.1:*`、`http://localhost:*`、以及 `--control-origin` 白名单）；不允许 `*`。
- 前端 token 存 `sessionStorage`，fragment nonce 用完即 `history.replaceState` 清除。
- 媒体静态路由 `resolve()` 后必须在 media 目录内；上传按 MIME 嗅探 + 大小限制。
- 所有假凭据以 `MQS_` / `mqs-` 前缀显眼标注；仓库不含真实 secret。
- README 与页面 footer 明确"非官方、与腾讯无关联"。

---

## 10. 未决 / 下一版

1. 错误码 40034/40054/40055/40057 为近似值，需真机抓包校正（已在 `errors.py` 集中，改一处即可）。
2. 频道（guild）家族仅 stub；论坛/音视频事件不模拟。
3. webhook 的 Ed25519 签名细节按公开资料实现，真机校验需补。
4. 桌面端（已改为 Electron）与 `.mqsprofile` 导入导出在 v0.2 完成。
5. 多进程/多 profile 并发未做；单 profile 单进程是硬约束。

---

## 11. 实施状态（2026-11-07）

v0.1 已落地并通过验收：

| 项 | 结果 |
|---|---|
| 测试 | `pytest` 90 项全绿（契约/鉴权/网关/消息/限流/媒体/频道/场景/CLI/模糊测试） |
| 真实 SDK | botpy（qq-botpy）echo bot 全链路通过：登录 → 网关鉴权 → 心跳 → 群 @ 收消息 → 被动回复 |
| 契约 | 23 条端点：12 已实现（9 full + 3 partial），11 stub；`mqs contract export` 生成 `docs/COVERAGE.md` 与 `web/src/types/api.ts` |
| GUI | Vue 3 四页（状态/配置/聊天/调试器）已构建并由控制 API 托管，Firefox 实测无 console error |
| CLI/MCP | 全部命令可跑；MCP 10 个工具注册成功；场景 JUnit/TAP 可用 |
| 分发 | `pip install -e .` + `mqs serve`；Electron 桌面端与 AppImage 已完成（v0.2） |

### 实施中新增/微调的决定

| # | 决定 | 原因 |
|---|---|---|
| I1 | 控制 API 同时托管 `/avatars/*` 与 `/media/*` | 前端与 Bot SDK 同源；避免 GUI 因跨端口拿不到头像/媒体 |
| I2 | 群消息只投递给**被 @ 的 Bot** | 真实群机器人语义；同时避免"未 @ 也收到"的假象。控制 API 有 `chat.auto_at_bot` 方便调试 |
| I3 | 会话列表返回 `member_ids` | GUI 需要判断"当前账号是否在群里"，否则发送会莫名失败 |
| I4 | 运行时长用墙钟，虚拟时钟只管业务时间 | 虚拟时钟冻结时 `uptime` 不应倒退/停摆 |
| I5 | `mqs expect` 默认只等**新**事件 | 与"等待下一个事件"语义一致；历史事件用 `mqs logs`/调试器查 |
| I6 | 限流响应的 `X-RateLimit-*` 在成功响应上也带 | SDK 可据此自适应；429 另带 `Retry-After` |

### 仍未做（v0.2+）

1. 频道（guild）家族真实行为、论坛/音视频事件；
2. 错误码真机校正（集中表已就位）；
3. Webhook 与真机签名细节对齐、失败重试可视化；
4. `.mqsprofile` 的**进程内恢复**（当前只支持导出 + 离线导入）；
5. Electron 桌面端已完成；`uvx` 一键运行、多 profile 并发仍未做。

---

## 12. v0.2 变更（2026-11-07 第二轮）

这一轮按"把文档规定的内容做完"继续推进，主要变化：

### 12.1 频率与时效严格对齐官方文档

来源：QQ 机器人文档 v2 → 消息收发概述 → **频率与时效规则**（2026-11-07 抓取原文）。

| 类型 | 场景 | 规则 |
|---|---|---|
| 被动 | 单聊 | **60 分钟 / 每条消息 4 次回复** |
| 被动 | 群聊 | **5 分钟 / 每条消息 5 次回复** |
| 被动 | 频道 | 5 分钟 |
| 主动 | 单聊（未认证） | Bot 5/s **且** 30/min；单关系 20/min；1000/天/用户 |
| 主动 | 单聊（个人/企业认证） | Bot 10/s；单关系 20/min；1000/天/用户 |
| 主动 | 群聊（未认证） | Bot 30/min；单关系 20/min；1000/天/群 |
| 主动 | 群聊（个人/企业认证） | Bot 60/min；单关系 20/min；1000/天/群 |
| 频道 | 所有消息 | 每子频道 5/s；主动默认 20/天/频道、2 个频道/天 |

**个人认证与企业认证限额完全一致，已合并为 `certified`**。因此 UI 只有三档：
`off`（关）/ `unverified`（未认证个人）/ `certified`（已认证，个人=企业）。

实现要点：
- 用**滑动窗口**而不是令牌桶（令牌桶会跨窗口边界漏放，与"每秒最多 N 条"不符）；
- 主动消息才计入配额，被动回复只看窗口/次数（官方也没有把被动算进主动频控）；
- 429 返回 `Retry-After` 与 `X-RateLimit-*`；
- 每日 1000 条持久化在 `active_quota`，跨重启有效。

### 12.2 L2 频道家族从 stub 升级为实现

新增 `guilds / channels / guild_members / guild_roles` 四张表（schema v2），默认 profile 自动创建
“QSandbox 测试频道”（2 个文字频道 + 1 个语音频道 + 全体成员/管理员角色）。

- REST：`/users/@me/guilds`、`/guilds/{id}`、`/guilds/{id}/channels|members|members/{uid}|roles`、
  `/channels/{id}`、`/channels/{id}/messages`（GET/POST）、`/channels/{id}/messages/{msg_id}`
- 网关：`MESSAGE_CREATE`（intent 1<<9）与 `AT_MESSAGE_CREATE`（intent 1<<30）
- 频道消息：5/s/频道、主动 20/天/频道、2 频道/天、被动 5 分钟
- 覆盖率：**31 条中 30 已实现**（27 full + 3 partial），仅 `/users/@me/dms` 保持 stub；新增内邀接口
  `GET /v2/groups/{group_openid}/members`（60 QPM）与 `.../members/{member_openid}`（30 QPM）

### 12.3 桌面端（Electron）

- `desktop/src/main.ts` 启动 MQS sidecar（`build-sidecar/mqs`，onefile），读 stdout
  JSON 握手，再用同一份 Vue 前端加载 GUI；退出时回收 sidecar（含 3.5s 兜底与 `os._exit` 清理）。
- 已构建并实测 `QSandbox-0.1.0-x86_64.AppImage`（142 MB），Xvfb 下启动、加载、截图、退出正常。
- Windows/macOS 由 `.github/workflows/desktop.yml` matrix 构建。
- sidecar 打包器 v0.2 用 PyInstaller，**v0.3 起改回 Nuitka**（`tools/build_sidecar.py`）。

### 12.4 GUI 修正

- 品牌统一为 **QSandbox**（窗口标题、侧栏、桌面产品名、默认 Bot/群/频道名）。
- **默认只有一个可操作账号**（1 个 human + 3 个 npc；账号切换列表只列 human）。
- **私聊 Bot 移入会话列表**（左栏“私聊 BOT”区），右栏只保留账号/成员/Bot 管理。
- 新增**设置面板**（右上角 ⚙）：主题（auto/dark/light）、语言、日志级别、遥测（默认关）、
  开机自启（桌面版）、虚拟时钟、profile 路径/端口、导出 profile、接口白名单与暴露范围。
- 配置页新增**规则引擎**页签（事件 → 条件 → 用户回复 + 延迟）、批量“加 3 个管理员”、
  随机 Bot 能力集、Bot 能力集编辑。
- 调试器新增“只看错误”“隐藏心跳”，状态页新增事件吞吐（/min）。

### 12.5 其它

- 场景模板：新增 `quote`（@/引用）、`permission`（权限不足）、`group_multi`（群聊多用户）、
  `media`（multipart 上传 + msg_type=7），共 10 个；`http` 步骤支持 multipart 与 `payload_contains`。
- 模糊测试：畸形 JSON/类型/网关帧必须返回 QQ 错误码或 4002/4010，不得 500。
- **接口白名单**落地：内邀接口（群成员列表/信息）按 Bot 的 `whitelisted` 开关返回 11253；
  `feature.expose=public` 时这些接口 404；60/30 QPM 读取限额。Bot 接入弹窗可显示/复制
  nonebot-adapter-qq 的 `QQ_API_BASE` / `QQ_AUTH_BASE` / `QQ_CUSTOM_GATEWAY_URL` / `QQ_BOTS`。
- `profiles.json` 全局索引 + `mqs profile list`；桌面端设置页/状态页可切换或新建 profile（重启 sidecar 并刷新窗口，`desktop.json` 记住选择）。
- 测试：**90 项全绿**；`ruff check` 干净。

### 12.6 仍未完成（诚实清单）

| 项 | 状态 |
|---|---|
| 系统钥匙串存 Bot secret / 控制 token（`--insecure-store`） | 未做，secret 仍在 SQLite；文档 §8 的安全增强 |
| `--record` 保存可重放 fixture、`--script x.ts/x.py` 逃生口 | 未做 |
| Koishi / NoneBot / qq-bot-sdk 兼容矩阵 | 只实测 botpy；CI 里有 botpy job；nonebot-adapter-qq 配置项已按源码提供但未起真实 NoneBot 实例 |
| 频道权限位（角色 → 细粒度 permissions） | 角色结构有了，权限判定未做 |
| Tauri 桌面壳 | 按用户决定改为 Electron |

---

## 13. v0.3 变更（2026-11-07 第三轮）

### 13.1 默认不 @ 任何 Bot

聊天页不再有“默认自动 @ 群内所有 Bot”的隐式行为：

- 未选择任何 Bot 时，前端显式发送 `at_bots: []`，消息只进入群会话，不产生 `GROUP_AT_MESSAGE_CREATE`；
- 选中 Bot 才显式 @，按钮指令仍通过 `implicit_bots` 只投递给发按钮的 Bot、不加 `<@!...>` 前缀；
- `chat.auto_at_bot` 保留为调试开关，默认 `false`；CLI/MCP/场景未显式给 `at_bots` 时同样不 @。

### 13.2 群管理（5 组接口）

| 接口 | 行为 |
|---|---|
| `GET /v2/groups/{group_openid}/info` | 群名、简介、分类、标签、人数（30 QPM） |
| `GET /v2/groups/{group_openid}/bot_state` | 机器人群内状态、`allow_proactive_msg`、`recv_msg_setting` |
| `GET/POST /v2/groups/{group_openid}/restrict_chat_setting` | 查询/设置成员禁言，`add/update/del`，单次 20 人，最长 30 天 |
| `GET/POST /v2/groups/{group_openid}/member_blacklist` | 黑名单查询与 add/del；仍在群内时加入失败 |
| `GET /v2/groups/{group_openid}/join_request_list` + `POST .../approval_join_request/{member_openid}` | 入群申请列表、approve/decline、拒绝时可直接拉黑 |
| `POST /v2/groups/{group_openid}/batch_remove_members` | 批量移除，可选同时拉黑 |

所有群管理接口与群成员接口一致，走“内邀白名单”校验（`11253`）。

### 13.3 频道写操作 + 角色成员

- 子频道：`POST /guilds/{guild_id}/channels`、`PATCH/DELETE /channels/{channel_id}`，分别触发
  `CHANNEL_CREATE/UPDATE/DELETE`；
- 身份组：`GET/POST /guilds/{guild_id}/roles`、`PATCH/DELETE .../roles/{role_id}`，
  `PUT/DELETE /guilds/{guild_id}/members/{user_id}/roles/{role_id}`；角色列表返回
  `number` / `member_ids` / `member_limit`；
- 成员：`DELETE /guilds/{guild_id}/members/{user_id}`、`PATCH .../members/{user_id}/mute`、
  `PATCH /guilds/{guild_id}/mute`，触发 `GUILD_MEMBER_UPDATE/REMOVE`。

### 13.4 频道私信 + 好友 + 主动消息

- `POST /users/@me/dms` 要求 Bot 与用户在同一频道，返回 `{guild_id, channel_id, create_time}`；
- `POST /dms/{guild_id}/messages` / `DELETE .../{message_id}`，被动不限条数；主动
  2 条/天/用户、200 条/天/Bot；
- 用户侧控制 API 发私信产生 `DIRECT_MESSAGE_CREATE`（intent `1<<12`）；
- 好友关系用 `POST/DELETE /_mqs/bots/{bot_id}/friends/{user_id}` 维护，触发
  `FRIEND_ADD` / `FRIEND_DEL`；`active_c2c_require_friend` 可开启“主动单聊需好友”；
- 主动消息开关：Bot 级 `allow_proactive_msg` + 群级覆盖；关闭后群/单聊/频道/频道私信的
  主动发送统一返回 `40040`，被动回复不检查。

### 13.5 用户富媒体与 GUI

- 控制 API `POST /_mqs/media` 接收 multipart 图片/视频，落内容哈希并返回 `{hash,url}`；
- `ChatSend.attachments` 支持群/单聊/频道/私信；正文为空且有附件时自动 `msg_type=7`；
- 聊天页支持文件选择、剪贴板粘贴图片、拖拽上传，发送前可预览/删除；
- 新增“频道”页：子频道、身份组、成员禁言/踢出、全员禁言、频道私信、好友关系、主动消息开关；
- 群聊右栏新增群管理：群信息、成员禁言/解禁、黑名单、入群申请审批、群级主动消息开关。

### 13.6 测试与覆盖率

- 测试：**106 项全绿**（新增群管理 5、频道/私信/好友 6、富媒体 3）；
- `ruff check src tests` 干净；
- 端点：**53 条，53 已实现（50 full + 3 partial），0 stub**；`docs/COVERAGE.md` 与
  `web/src/types/api.ts` 已重新生成。

### 13.7 仍未完成（诚实清单）

| 项 | 状态 |
|---|---|
| 入群自动审批策略（`join_approval_strategy` 5 条接口） | 未做；仅实现申请列表与人工审批 |
| 频道内容接口：公告、日程、置顶、论坛、音视频/直播成员 | 未做 |
| 子频道权限覆盖（`channel_permissions` / 身份组细粒度 permissions） | 未做；身份组结构有，权限判定未做 |
| 频道成员申请/频道黑名单 | 未做 |
| 系统钥匙串存 Bot secret / 控制 token | 未做，secret 仍在 SQLite |
| `--record` 可重放 fixture、`--script x.ts/x.py` 逃生口 | 未做 |
| Koishi / NoneBot / qq-bot-sdk 兼容矩阵 | 只实测 botpy；nonebot-adapter-qq 仅提供配置 |
| Webhook Ed25519 真机签名校验 | 仅按公开资料实现握手 |
| 错误码 40034/40054/40055/40057 真机抓包校正 | 未做，集中表已就位 |
| Tauri 桌面壳 | 用 Electron 替代 |

### 13.8 构建与发布（2026-11-07）

- sidecar 打包从 PyInstaller 换回 **Nuitka onefile**（`nuitka[onefile]`；Linux 额外装 `patchelf`）。
  `tools/build_sidecar.py` 会把 `web/dist` 与 `schema*.sql` 一起打进单文件。
- CI 新增 `build` job：构建前端 → Nuitka 编译 sidecar → 起进程做握手冒烟 → 上传
  `qsandbox-sidecar-linux` artifact。
- `desktop.yml` 改为只在 **push 到 main** 和三平台打包（产物作为 Actions artifact）。
- 新增 `release.yml`：push `v*` tag 时三平台打包，`publish` job 汇总后上传到 GitHub Release。
