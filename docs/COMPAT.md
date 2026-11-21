# SDK 兼容性与已知差异

> 文档快照：2026-11-21 · 对应 QSandbox v0.6（Rust + Tauri）

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
| 15 | QQ 客户端的 Markdown 渲染器把 `\(…\)` 当**行内公式定界符**（见 Moonlark-Dev/Moonlark#1556），GUI 同样按公式渲染：定界符被吃掉，内容放进 `<span class="qq-math">` | 复杂 TeX（`\frac` 等）在 GUI 里显示的是源码而不是排版后的公式 | 预览层差异：真机由客户端排版，沙箱渲染器仍保持零依赖、只做公式样式；`markdownPlain` 列表预览同样去掉定界符 |
| 16 | **`union_openid` / `union_group_openid` 是按内部 id 派生的稳定假值**（`sha256("union:<id>")` 前 32 位大写），不是真机的跨应用标识；官方语义是「绑定 3 方应用时才有效」 | 只能断言“存在且稳定、同一实体在所有 Bot 下一致”，不能与真机数据比对 | 沙箱本来就不接开放平台账号体系；`union_group_openid` 用在群信息与进群/退群/成员增减/群消息事件里 |
| 17 | `GET /v2/users/{openid}/info` **没有好友关系模型**：只要这个 openid 曾由该 Bot 生成过（`openid_map` 有记录）就返回资料，未知 openid 返回 `40184001`（HTTP 403 是沙箱自定，官方只给 code）；openid 解析出来是群/机器人时返回 `10001` | 复现不了“有 openid 但无好友”的拒绝分支（真实平台的前提是成为好友/主动会话过） | 官方该码就是「无好友关系」，沙箱拿它表示「本 Bot 不认识这个 openid」 |
| 18 | `users.info` 的 `allow_proactive_msg` 反映 **Bot 侧**主动消息开关（`bots.allow_proactive_msg`），不是用户偏好 | 用 `PATCH /_mqs/bots/{id}` 切换即可测 true/false 两个分支 | 沙箱没有用户级推送偏好列；这个值正是 `send_as_bot` 主动消息真正校验的那个 |
| 19 | 群事件的 `member_nickname` 等于用户昵称（沙箱没有独立群名片），`mentions[].member_nickname` 同理 | 断言“等于 username”即可 | 官方 member_nickname 是群内昵称（card） |
| 20 | 事件/接口里的 `avatar` 是沙箱自托管的**绝对 URL**（`<sim_base>/avatars/<id>.svg?label=…&kind=…`） | SDK 可以直接下载，但图是本地生成的 SVG 占位头像，不是 QQ 头像 CDN | 官方是 `thirdqq.qlogo.cn` 一类的 https 链接；沙箱不联网 |
| 21 | **`GROUP_MEMBER_ADD` / `GROUP_MEMBER_REMOVE` 已投递**（官方 intent `1<<24`），触发点是控制 API 加/删群成员、审批通过入群申请、群成员批量移除接口、`POST /_mqs/users/random` 带 `group_id` 批量造人（每个新成员一条）；`GROUP_JOIN_REQUEST` 仍未投递 | 群成员增减可以联调，但批量造人时会一次喷 N 条 ADD 事件；事件驱动的「验群」闭环（申请事件 → 自动审批策略）仍缺 | 见 `docs/UNIMPLEMENTED.md` §5；`timestamp` 按官方是 Unix 秒整数 |
| 22 | `GROUP_MEMBER_REMOVE` 的 `user_openid` 与 `member_openid` 同值（官方语义是“用户 openid，可能为空”） | 断言两者相等即可；沙箱没有“群内 member openid ↔ 用户 openid”双层体系 | 官方示例里两者恰好相同 |

| 23 | **素材图片的预签名 URL 是沙箱自建的 `PUT /v2/media-images/uploads/{upload_id}?sign=…&expires=…`**，不是 COS 域名；该 PUT 不校验 Bot token（真实平台靠 COS 签名） | 按官方流程把 `presigned_url` 原样 PUT 即可；不要把它当成固定路径写死 | Content-Type 必须与 prepare 一致（模拟 COS 的 403）；sign/expires 不匹配、预签名过期或 upload_id 不存在/过期都返回 `40035003`（真实 COS 用 403），响应 `ETag` 引号内即文件 MD5 |
| 24 | `preview_url` 是 `<sim_base>/media/<sha256>` 的**长期公开地址**，不是短时效 COS 预签名链接 | 可以断言/预览，但不要把它当成官方「短期有效、请勿持久化」的语义测试 | 图片字节仍在内容哈希媒体库，删除素材图片不会删除已发送消息引用的副本 |
| 25 | 素材图片上限可用设置项 `media_image_limit` 调整（默认 100）；`upload_id` 4 小时、预签名 URL 1 小时、90 天滑动过期都受**虚拟时钟**控制 | 可以用 `clock advance` 测过期/顺延，不必等真实时间 | 官方固定 100 张；沙箱多出的设置项只影响配额校验。complete 的「计数 + 插入」在同一 SQLite 事务里，不会并发超限 |
| 26 | `prepare` 按官方说明用 `sha1` 做幂等：同一 Bot 对同一 `sha1`（且 md5/文件名/类型/大小一致）且未过期的上传任务复用同一个 `upload_id`，只刷新预签名 URL | 重复 prepare 不会产生多个上传任务 | 官方只写了「sha1 用于登记与幂等识别」，未规定复用范围；沙箱按最保守的同文件同任务处理 |
| 27 | 发送 Markdown 时，沙箱只解析 `![…](…)` 图片目标里的 `qqbot-media://image/v1/<token>`（跳过 code span / fenced block）；正文裸写或普通链接里的该串不校验也不顺延有效期 | 引用校验与滑动过期按“图片语法”生效 | 校验结果：未知/他人 token → `40035010`（HTTP 403）；已删除/过期 → `40035009`（HTTP 400）；成功后在同一事务里顺延 `expires_at` |
| 28 | 发送时按文件头（前 16 字节）嗅探图片格式并与 `prepare` 声明的 `content_type` 比对，不匹配 → `40035001`；`complete` 本身成功 | 内容不合法的图片会在发送这一步被拒，与官方「complete 可能成功、发送时格式检查失败」一致 | 只做文件头 sniff，不做完整解码；JPEG/PNG/GIF/WebP 的有效性以解码器为准，沙箱不模拟 |
| 29 | 控制 API 的消息对象额外带 `media_images: [{token,url,image_id,file_name,preview_url,expires_at}]`，GUI 用它把 `qqbot-media://` 渲染成可加载的预览图 | 这是控制面扩展字段，不是 QQ 推送字段 | 与 `at_users` 同类处理：网关 payload/`raw` 保持原样 |
| 30 | **群消息私聊回复已支持**（2026-11-17）：群消息的 `msg_id` 可传给 `POST /v2/users/{openid}/messages`，私聊回复该群消息的发送者。窗口/次数按群消息规则算，且与群内回复共用同一 `msg_id` 的计数（`reply_windows` 按 `msg_id+bot` 一行） | `msg_id` 不是该用户发的、或 Bot 已不在该群 → `40034`；超时/超次 → `40057`；重复 `msg_seq` → `40054`。官方对应码是 `40034024`/`40034005`/`40034128`/`40054005`，沙箱沿用近似码 | `msg_seq` 去重同样跨群内/私聊共用；私聊回复落在 DM 会话 |
| 31 | **好友关系仍未建模**：官方要求群消息发送者与 Bot 是好友才可私聊回复（`40054004`/`40054006`） | 沙箱把「该 openid 曾由本 Bot 生成过（`openid_map` 有记录）」当作好友，不能复现「有 openid 但非好友」的拒绝分支；这与 `GET /v2/users/{openid}/info` 的口径一致 | 不引入好友表/`FRIEND_*` 家族，保持「只覆盖群聊 + 单聊 + 群管理 + 媒体」的产品边界 |
| 32 | **`group_bots.member_role` 可配**（`member`/`admin`/`owner`），`GET .../bot_state` 返回真实值；群被动回复上限据此从 5 提到 50（设置项 `passive_reply_limit_group` / `passive_reply_limit_group_admin`），群内回复与群消息私聊回复都适用 | 沙箱没有「机器人开发者个人账号」，用**机器人在群内的角色**代表「开发者是群主/管理员」；这是该字段在当前产品边界内能做到的最接近建模 | 控制 API `PATCH /_mqs/groups/{group_id}/bots/{bot_id}` 的 `member_role`，GUI 群管理也有下拉框 |
| 33 | 群被动回复窗口默认 **60 分钟**（`passive_window_group_ms`），仍受虚拟时钟控制；单聊保持 60 分钟 / 4 次 | 用 `POST /_mqs/clock/advance` 测窗口即可，不必等真实时间 | 2026-11-17 官方把群窗口从 5 分钟延长到 60 分钟 |

## 端口与域名假设

- botpy 默认 `is_sandbox=False`，垫片同时改 `DOMAIN` 和 `SANDBOX_DOMAIN`，两种都指向沙箱。
- access token 端点挂在**模拟端口**上（`POST /app/getAppAccessToken`），不是单独域名；垫片负责改地址。
- 网关 WebSocket 路径固定 `/websocket`（同时接受 `/websocket/`）。
- 媒体下载在 `/media/<sha256>`，`file_info` 形如 `mqs-file-<sha256>`。

## 回归方式

```bash
cargo test -p mqs                                   # 单元 + 集成（140 项）
cargo run -q -p mqs -- serve                        # 或 ./target/debug/mqs serve
MQS_BASE_URL=http://127.0.0.1:5700 python examples/botpy_echo/bot.py
mqs script run smoke                                # 需要上面这个 echo bot 在线
```
