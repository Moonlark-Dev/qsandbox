# QSandbox 设计参考

> 这是实现说明，不是计划书：文中每一条都对应 `crates/mqs/` 里的代码。里程碑、版本历史与实施状态已经移除，进度只以测试和 [docs/COVERAGE.md](docs/COVERAGE.md) 为准。

## 0. 技术选型

| 主题 | 选择 | 原因 |
|---|---|---|
| 服务端 | Rust（axum 0.8 + tokio） | 单个自动二进制；桌面端可把 server 作为库内嵌，不再需要 sidecar |
| 桌面端 | Tauri v2 | 运行时是系统 WebView，安装包小；同进程启动 server，没有 Nuitka/Python |
| 存储 | rusqlite（bundled）+ 单写连接 | 单进程、单文件库；"部分更新列 + 原子 seq 自增"用 SQL 直写最简单 |
| SDK 垫片 | Python（`python/`） | botpy 把域名/token URL 写死，只能用 Python 猴补丁；不编译进二进制 |
| 前端 | Vue 3 + Vite | 桌面与浏览器共用一份 dist，构建产物编译期内嵌进二进制 |
| 契约 | 静态 `REGISTRY` + `ROUTE_TABLE` | 覆盖率、文档、TS 类型、路由三者由测试强制一致 |

## 1. 进程、端口与生命周期

```
mqs serve --profile dev --host 127.0.0.1 --port 0 --control-port 0
        │
        ├── Simulate API  (默认 5700)   /v2/*  /gateway  /app/getAppAccessToken  /media/*  /avatars/*
        │      · 允许任意 Origin（SDK 不管 CORS）
        │      · 鉴权：Authorization: QQBot <app_access_token>
        │
        ├── Control API   (默认 5701)   /_mqs/*  /  (内嵌 Vue dist)
        │      · X-MQS-Token 头 / bearer / mqs_session cookie
        │      · Origin 校验（只允许 loopback 与显式白名单）
        │
        └── 共享：World + Hub + GatewayManager + 虚拟时钟 + SQLite(WAL) + media
```

- **启动握手**：监听成功后向 stdout 打一行 JSON：
  `{"mqs":1,"version":"…","profile":"dev","sim_port":5700,"control_port":5701,"token":"…","base_url":"…","ws_url":"…","gui_url":"…"}`，
  同时写 `profiles/<name>/runtime.json`（CLI/MCP/垫片靠它发现端口与 token）。
- **端口 0** 表示随机端口；实际端口在绑定后回填进握手与状态页。
- **文件锁**：`profiles/<name>/profile.lock` 用 `flock(LOCK_EX|LOCK_NB)`；第二个进程退出码 4。
- **默认只绑 127.0.0.1**；`--host 0.0.0.0` 要求显式 `--token`，并在 stderr 打警告。
- **信号**：SIGINT/SIGTERM → 两个 listener 优雅关闭 → 关网关连接 → 删 runtime.json → 释放锁。

## 2. 数据模型（`crates/mqs/src/schema.sql`，schema v1）

```sql
settings          -- key/value（JSON 编码）
bots              -- id(=app_id), secret, scopes(json), delivery(ws|webhook), whitelisted, allow_proactive_msg
users             -- id, nickname, actor_type(human|npc), behavior, seed
groups            -- id, name, owner_id, join_policy, finger_memo, class_text, tags(json)
group_members     -- (group_id,user_id), role(owner|admin|member), mute_until
group_bots        -- (group_id,bot_id), allow_proactive_msg, recv_msg_setting(all|only_mention|mention_and_context)
group_blacklist   -- (group_id,user_id), banned_at
group_join_requests -- id, group_id, user_id, status(pending|approved|declined), …
conversations     -- id(group:<gid>|dm:<bot>:<uid>), type, peer_id, last_seq
openid_map        -- (bot_id,kind,entity_id) → openid, union_openid   ★ 按 Bot 隔离
messages          -- 会话内自增 seq, msg_id, direction(in|out), content, raw_json, sent_at, recalled_at
events            -- 全局自增 seq, event_id, type, payload_json, target_bot_id   ★ 每个 Bot 一条投递记录
gateway_sessions  -- session_id, intents, last_seq, shard, closed_at
webhook_endpoints -- bot_id, url, secret
app_tokens        -- token → bot_id, expires_at（受虚拟时钟控制）
media             -- (hash) → mime, size, path
reply_windows     -- (msg_id,bot_id) → count, expires_at
reply_keys        -- (bot_id,msg_id,msg_seq)  ★ 显式去重
active_quota      -- (bot_id,conversation_id,day) → used
frames            -- 调试器数据（双向 HTTP/WS，保留最近 2000 条）
rules             -- 事件 → 条件 → 用户回复 + 延迟
```

**事件溯源**：所有世界变更先写 `events`（同事务）再广播。`messages.seq` 是会话内序号；
`events.seq` 是全局序号，网关的 `s` 用它。

**两条铁律**（都有测试）：
1. 群内新建用户 = 全局 `users(actor_type=npc)` + `group_members`，同事务；账号切换列表只列 `human`。
2. `openid` 只存在于 `openid_map`，同一 user 在两个 Bot 下 openid 不同。

## 3. 协议保真

### 3.1 鉴权

- `POST /app/getAppAccessToken`，body `{"appId":"10000","clientSecret":"mqs-secret-10000"}`
  → `{"access_token":"MQS_…","expires_in":"7200"}`。
- 错误：appid 不存在 → 401 `{"code":11243}`；secret 不匹配 → 401 `{"code":11244}`。
- 访问 API：`Authorization: QQBot <token>`；无效 → 11241；过期 → 11242（`clock advance 2h` 可测）。

### 3.2 网关

| 阶段 | 方向 | 帧 |
|---|---|---|
| 连接后 | S→C | `{"op":10,"d":{"heartbeat_interval":41250}}` |
| 鉴权 | C→S | `{"op":2,"d":{"token":"QQBot …","intents":33554432,"shard":[0,1]}}` |
| 成功 | S→C | `{"op":0,"s":0,"t":"READY","d":{"version":1,"session_id":"…","shard":[0,1],"user":{…}}}` |
| 心跳 | C→S | `{"op":1,"d":<last_s>}` → `{"op":11,"d":null}` |
| 恢复 | C→S | `{"op":6,"d":{"token","session_id","seq"}}` → `{"op":0,"t":"RESUMED","d":""}` + 补发 |
| 恢复失败 | S→C | `{"op":9,"d":false}`，随后 close 4006 |
| 事件 | S→C | `{"op":0,"s":N,"t":"GROUP_AT_MESSAGE_CREATE","id":"…","d":{…}}` |

- intents：群/C2C 消息与群机器人事件 `1<<25`，交互 `1<<26`；`intents=0` 按 botpy 行为视为 1。
- 未订阅事件不投递；分片按 `crc32(conversation_id) % shard_count == shard_id` 过滤。
- 心跳超时（`heartbeat_interval*2` 未收到）→ close 4009；非法 JSON → 4002；非法分片 → 4010。
- 所有上下行帧写 `frames`（调试器数据源）。

### 3.3 消息语义

- 事件：`GROUP_MESSAGE_CREATE` / `GROUP_AT_MESSAGE_CREATE` / `C2C_MESSAGE_CREATE` /
  `GROUP_ADD_ROBOT` / `GROUP_DEL_ROBOT` / `INTERACTION_CREATE`。
- 群事件种类由 `group_bots.recv_msg_setting` 决定：
  - `all`（默认）：所有群消息都投 `GROUP_MESSAGE_CREATE`，@ 由 `mentions[].is_you` 表达，
    `content` 保持用户输入；
  - `only_mention` / `mention_and_context`：只有被 @ 时投 `GROUP_AT_MESSAGE_CREATE`，
    `content` 前加 `<@!openid> `。
- 群 payload：`{id, content, group_openid, group_id, author:{id, member_openid, union_openid, bot, member_role}, timestamp, msg_seq, mentions, attachments}`。
- 单聊 payload：`{id, content, author:{id, user_openid, union_openid}, timestamp, …}`。
- 被动回复：`msg_id` 必须属于该 Bot 在该会话收到的消息；群 5min/5 次、单聊 60min/4 次；
  显式 `msg_seq` 重复 → 40054；未给时自动递增。
- 引用：`message_reference.message_id` 必须存在（40034）。
- 撤回：`DELETE` → 204；非本 Bot 消息 → 40034；只标记 `recalled_at`（`partial`）。
- 未知字段：请求体整体存 `raw_json`，`message_out` 原样带回（`markdown`/`keyboard`/`ark`/`embed` 等）。

### 3.4 错误形状

统一 `{"code": <int>, "message": "<str>", "trace_id": "<hex16>"}`，响应头 `X-Tps-Trace-ID`；
绝不外泄框架的 `{"detail":…}`（提取器错误统一映射成 10001）。
HTTP 状态：参数/语义 400，鉴权 401，权限/白名单 403，不存在 404，限流 429，内部 500。

| code | 含义 |
|---|---|
| 10001 / 10002 | 参数错误 / 内部错误 |
| 11241 / 11242 / 11243 / 11244 | token 无效 / 过期 / appid 无效 / secret 无效 |
| 11253 | 无接口访问权限（内邀） |
| 40034 | 消息不存在或 msg_id 无效 |
| 40040 | 无操作权限（含主动消息被关闭） |
| 40054 | 重复回复（msg_id+msg_seq） |
| 40055 | 主动消息配额不足 |
| 40056 | 机器人不在线 |
| 40057 | 被动回复窗口过期 / 次数用尽 |
| 42900 | 限流（带 `Retry-After`、`X-RateLimit-*`） |

### 3.5 限流（官方频率政策）

被动回复不看频控；主动消息才计入。滑动窗口（不是令牌桶）。
`off` / `unverified`（5/s+30/min C2C、30/min 群）/ `certified`（10/s C2C、60/min 群），
单关系 20/min，每日 1000 条（`active_quota` 持久化）。

### 3.6 媒体

- `POST /v2/{groups/{g}|users/{u}}/files`：JSON `{file_type,url,srv_send_msg}` / multipart `file` /
  base64 `file_data` / `upload_id` 收尾。
- 分片：`upload_prepare` → `PUT /v2/uploads/{id}/parts/{i}` → `upload_part_finish`（校验 md5/块大小）
  → `files` 带 `upload_id`。
- 返回 `{file_uuid, file_info:"mqs-file-<sha256>", ttl:0}`；`GET /media/<sha256>` 下载。
- 限制：图片 10MB、视频 30MB、语音 5MB；`file_type=4`（普通文件）明确不支持（官方标注暂不开放）。

## 4. 控制 API（`/_mqs/*`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health` | 无鉴权存活探针 |
| POST | `/session` | nonce 换 HttpOnly cookie（一次性） |
| GET | `/status` | 端口、profile、运行时长、计数、吞吐、限流拒绝、DB 大小、覆盖率 |
| GET/PATCH | `/settings` | 时钟、种子、限流档位、白名单/暴露开关、语言/主题 |
| CRUD | `/users` `/users/random` | 含 `?available=true`（只列 human） |
| CRUD | `/groups` `.../members` `.../bots` | 群、成员、Bot 加入/移出 |
| GET/PATCH | `/groups/{id}/info` `/bot-state` `.../bots/{bot}` | 群信息、群内 Bot 开关（全量消息/主动消息） |
| CRUD | `/groups/{id}/blacklist` `/join-requests` | 黑名单、入群申请与审批 |
| CRUD | `/bots` `.../token` `.../rotate-secret` `.../webhook` | Bot、token、回调 |
| POST | `/media` | 控制台上传图片/视频/音频（富媒体消息用） |
| GET | `/conversations` `/conversations/{id}/messages` | 会话与消息分页 |
| POST | `/chat/send` | 以某用户身份发言（触发网关事件） |
| POST | `/messages/{id}/click` | 模拟点击按钮（指令/回调/链接） |
| GET | `/debug/frames` `/debug/events` | 调试器数据 |
| POST | `/clock/advance` `/clock/set` `/rate/reset` | 虚拟时钟与限流计数 |
| CRUD | `/rules` | 规则引擎（事件 → 条件 → 用户回复 + 延迟） |
| GET/POST | `/scenarios` `/scenarios/run` | 内置/本地场景与执行 |
| POST | `/snapshot` | profile 快照（zip：db + profile.json + media） |
| GET | `/contract` `/contract/types.ts` `/contract/coverage.md` | 契约生成物 |
| POST/WS | `/ws-ticket` `/ws?ticket=` | 前端实时事件（一次性 ticket） |

## 5. CLI / MCP

CLI 是控制 API 的薄客户端（`--json`、稳定退出码 0/2/3/4/5）。
MCP stdio 提供 10 个工具：`get_status`、`list_conversations`、`read_messages`、`send_as_user`、
`expect_event`、`inspect_gateway`、`advance_clock`、`create_random_users`、`snapshot`、`run_scenario`。

## 6. 前端（Vue 3 + Vite + TS）

- 侧栏：状态 / 配置 / 聊天 / 调试器；设置右上角。**没有频道页**。
- 状态：接入信息（base_url/ws_url/appid/token）、连接数、吞吐、限流拒绝、DB 大小、覆盖率、profile 切换。
- 配置：用户/群/Bot 三张表、随机生成器、限流档位、白名单/暴露开关、规则引擎、Bot 接入配置（nonebot）。
- 聊天：三栏（会话/消息流/操作面板）；账号切换 `Ctrl+Tab`、`Ctrl+1..9`；群管理面板；
  文件选择、粘贴、拖拽上传。
- 调试器：帧流（WS 实时）、按 Bot/类型过滤、双向原始 JSON、只看错误/隐藏心跳。
- 主题：深色优先，遵循 `prefers-color-scheme`；文案 zh-CN。

## 7. 测试矩阵

| 类型 | 文件 | 关键断言 |
|---|---|---|
| 单元 | `src/*::tests` | 时钟/时长、seed 可复现、openid 作用域、错误形状、限流窗口、schema 幂等、契约生成 |
| 契约 | `tests/contract.rs` | 注册表 ↔ 路由表双向一致；无频道家族；生成物不含未解析引用 |
| 鉴权 | `tests/auth.rs` | token 发放/过期/错误 secret；QQBot 头；`detail` 不外泄 |
| 网关 | `tests/gateway.rs` | hello→identify→ready→heartbeat→resume 补发；intents；4004/4006 |
| 消息 | `tests/messages.rs` | 投递与 @ 语义、openid 隔离、被动窗口/次数、msg_seq 去重、撤回、配额、按钮 |
| 限流 | `tests/ratelimit.rs` | 5/s、10/s、30/min、20/min、每日配额、off、被动不受限 |
| 媒体 | `tests/media.rs` | multipart/base64/url/分片、下载哈希、超限、穿越 |
| 群管理 | `tests/community.rs` | 群信息、禁言、黑名单、入群审批、主动消息开关 |
| 白名单 | `tests/whitelist.rs` | 11253、关闭白名单、public 暴露、游标分页 |
| 场景 | `tests/scenario.rs` | 10 个内置、echo smoke、JUnit/TAP、http 步骤、recv_msg_setting |
| 真实 SDK | `tests` + CI `botpy-compat` | botpy echo bot 全链路；`mqs script run smoke` |
| CLI | `tests/cli.rs` | `status --json`、token、用户/群、场景、doctor、退出码、离线契约导出 |
| 模糊 | `tests/fuzz.rs` | 畸形 JSON/类型/网关帧 → QQ 错误码，绝不 500 |

## 8. 安全

- 默认 loopback；`0.0.0.0` 必须显式 token 且 stderr 警告。
- 控制端口校验 `Origin`（loopback + `--control-origin`），不允许 `*`。
- token 走 header 或 HttpOnly cookie；URL fragment 里的一次性 nonce 用完即从地址栏清除。
- 媒体静态路由 `resolve()` 后必须在 media 目录内；上传按 MIME 嗅探 + 大小限制。
- 所有假凭据以 `MQS_` / `mqs-` 前缀显眼标注；仓库不含真实 secret。

## 9. 未决 / 下一版

见 [docs/UNIMPLEMENTED.md](docs/UNIMPLEMENTED.md)：频道家族（已删）、论坛/音视频/日程/公告、
入群自动审批策略、file_type=4、错误码真机校正、webhook 真机签名细节、钥匙串存储、
`--record` 可重放 fixture、多 profile 并发、Koishi/NoneBot/qq-bot-sdk 实测矩阵。
