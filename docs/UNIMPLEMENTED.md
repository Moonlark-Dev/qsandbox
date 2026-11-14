# QSandbox 未实现清单（对照 QQ 机器人官方文档 v2）

> 文档快照：2026-11-09。口径：官方文档站（bot.q.qq.com/wiki）全量 sitemap 174 个 API v2 页面 + 变更记录逐条比对，
> 与本仓库 `crates/mqs/src/contract.rs` 的 `REGISTRY`（29 个端点）
> 及 `gateway.rs::intent_for`（仅 `1<<25`、`1<<26`）对照。**查不到路径的写"文档未给出具体路径"，不臆造。**
> 本清单只列"文档确实有、沙箱没有"的能力；沙箱多做的东西（如 `msg_type` 直接接受 `keyboard`/`embed`）
> 不计入缺口，另见 `docs/COMPAT.md`。

## 1. 频道（guild/channel）家族 — 全部

按产品决定整族移除：沙箱路由表与 intent 表里已无任何 guild/channel 项，`/users/@me/guilds`、
`/guilds/{guild_id}`、`/channels/{channel_id}` 等一律 404。以下按官方文档分类给出现存接口，便于估算缺口面。

- **频道与子频道 CRUD（7）** — `GET /users/@me/guilds`、`GET /guilds/{guild_id}`、`GET|POST /guilds/{guild_id}/channels`、`GET|PATCH|DELETE /channels/{channel_id}`。影响：任何"频道机器人"示例（botgo/botpy 的 guild demo）在沙箱里第一步就断。
- **频道消息（4）** — `POST /channels/{channel_id}/messages`、`GET /channels/{channel_id}/messages`、`GET /channels/{channel_id}/messages/{message_id}`、`DELETE /channels/{channel_id}/messages/{message_id}`。影响：频道消息的子频道内 `seq`/`seq_in_channel` 语义、5 分钟被动窗口、每子频道 5 条/秒限频都无法模拟。
- **频道私信（3）** — `POST /users/@me/dms`、`POST /dms/{guild_id}/messages`、`DELETE /dms/{guild_id}/messages/{message_id}`；限额为每机器人每天每用户 2 条主动、累计 200 条（`server-inter/message/overview.html`）。影响：与单聊 C2C 是两套不同语义，沙箱只有后者。
- **频道成员（7）** — `GET /guilds/{guild_id}/members`、`GET|DELETE /guilds/{guild_id}/members/{user_id}`、`GET /guilds/{guild_id}/roles/{role_id}/members`、`PATCH /guilds/{guild_id}/members/{user_id}/mute`、`PATCH /guilds/{guild_id}/mute`、`GET /channels/{channel_id}/online_nums`。
- **身份组与权限（6）** — `GET|POST /guilds/{guild_id}/roles`、`PATCH|DELETE /guilds/{guild_id}/roles/{role_id}`、`PUT|DELETE /guilds/{guild_id}/members/{user_id}/roles/{role_id}`。影响：频道身份组（`permissions` 位串、`color`、`hoist`、`position`）没有沙箱对象，权限位模型整体缺失。
- **接口授权（2）** — `GET /guilds/{guild_id}/api_permission`、`POST /guilds/{guild_id}/api_permission/demand`（生成授权链接），文档见 `server-inter/channel/api_permissions/*`。
- **频道小程序（文档未给出具体路径）** — `server-inter/channel/miniapp/interface.html`、`opendata.html` 两页只描述能力与数据，未给 HTTP 路径。
- **intents（移除）** — `1<<0` GUILDS（GUILD_CREATE/UPDATE/DELETE、CHANNEL_CREATE/UPDATE/DELETE）、`1<<1` GUILD_MEMBERS（ADD/UPDATE/REMOVE）、`1<<9` GUILD_MESSAGES（MESSAGE_CREATE/DELETE）、`1<<10` GUILD_MESSAGE_REACTIONS（MESSAGE_REACTION_ADD/REMOVE）、`1<<12` DIRECT_MESSAGE（DIRECT_MESSAGE_CREATE/DELETE）、`1<<30` PUBLIC_GUILD_MESSAGES（AT_MESSAGE_CREATE/PUBLIC_MESSAGE_DELETE）。影响：`/gateway` 收到这些 intent 位时沙箱不会报错，而是静默收不到事件——调试时容易误判成"事件丢了"。
- **好友（移除）** — `FRIEND_ADD`、`FRIEND_DEL`（intent `1<<25`，`server-inter/user/manage/event.html`）。注意：它们与群/单聊共用 `1<<25`，所以同一 intent 位下沙箱"部分实现"。

## 2. 频道内容类

同上整族连带移除，但内容类接口较多、独立成节便于对账。

- **公告（4）** — `POST|DELETE /guilds/{guild_id}/announces`、`POST|DELETE /channels/{channel_id}/announces`（`server-inter/channel/content/announces/*`）。
- **精华/置顶（3）** — `PUT|DELETE /channels/{channel_id}/pins/{message_id}`、`GET /channels/{channel_id}/pins`；对象名即 `PinsMessage`（精华消息）。
- **日程（5）** — `GET|POST /channels/{channel_id}/schedules`、`GET|PATCH|DELETE /channels/{channel_id}/schedules/{schedule_id}`；限额：每频道每天 10 个、单个不超过 7 天（错误码 302012/302014）。
- **论坛帖子（4）** — `GET|PUT /channels/{channel_id}/threads`、`GET|DELETE /channels/{channel_id}/threads/{thread_id}`，另有"开放论坛"事件对象（`open_forum.html`，intent `OPEN_FORUM_EVENT`）。
- **音视频/直播（3 接口 + 2 事件类）** — `POST /channels/{channel_id}/audio`（音频控制）、`PUT|DELETE /channels/{channel_id}/mic`（上麦/下麦）、音视频/直播子频道成员进出事件（`channel_type` 2=音视频、5=直播）；`POST /channels/{channel_id}/audio` 与 `AUDIO_*` 事件（intent `1<<29`）同源。
- **子频道权限覆盖（4）** — `GET|PUT /channels/{channel_id}/roles/{role_id}/permissions`、`GET|PUT /channels/{channel_id}/members/{user_id}/permissions`。影响：这是频道里"细粒度覆盖"的唯一入口，群聊场景完全没有对应能力。
- **频道黑名单 / 成员申请** — 黑名单通过 `DELETE /guilds/{guild_id}/members/{user_id}` 的 `add_blacklist` 参数完成（同一接口）；"频道成员申请"文档未给出独立接口路径。
- **身份组细粒度 permissions（1）** — `PATCH /guilds/{guild_id}/roles/{role_id}` 的 `permissions` 位串；沙箱没有权限位模型。
- **表情表态（3 接口 + intent）** — `PUT|DELETE /channels/{channel_id}/messages/{message_id}/reactions/{type}/{id}`、`GET .../reactions/{type}/{id}?cookie&limit`（`trans/emoji.html`，仅频道可用）+ `MESSAGE_REACTION_ADD/REMOVE`。影响：与群聊无关，但 botpy 的类型定义里有，SDK 会在导入期就引用。

## 3. 群管理未做

群管理主体（群信息、`bot_state`、禁言、黑名单、批量移除、入群申请列表/审批、成员列表/详情）沙箱已覆盖，差的是 2026-08 新增的自动审批策略一类。

- **入群自动审批策略（6 个接口，全缺）** — `GET|POST /v2/groups/join_approval_strategy`、`PATCH|DELETE /v2/groups/join_approval_strategy/{strategy_id}`、`POST /v2/groups/join_approval_strategy/{strategy_id}/execute`、`POST /v2/groups/join_approval_strategy/{strategy_id}/whitelist_users`。限额：一机器人最多 20 个策略、每策略最多 100 个群、默认一年过期、`is_enable=on/off`。影响：自动化拉群/验群类机器人无法在沙箱里跑完整闭环（申请事件 + 策略执行 + 白名单）。
- **`global_rule` 只能读成 `none`** — `GET /v2/groups/{group_openid}/restrict_chat_setting` 的 `global_rule.mode` 在沙箱恒为 `none`（`community.rs:105` 硬编码），`schedule_rules`/`recurring_rules` 恒空。官方文档对 GET 定义了 `none|always|schedule` 三态与 `MuteScheduleRule`/`MuteRecurringRule`；但 POST 只接受 `members[]`，**文档未给出设置全员禁言的接口**，故这是"只读字段无法建模"，不是漏接口。
- **`bot_state.member_role` 恒为 `member`** — 官方返回 `member|owner|admin`；沙箱从不返回 `admin`，因此"机器人需为群管理员才生效"的一类行为（自动审批策略、@全员每天 1 次）无法通过该字段验证。
- **入群申请事件缺失** — `GROUP_JOIN_REQUEST`（intent `GROUP_MEMBER_EVENT (1<<24)`，见 §5），导致 `join_request_list` 只能轮询、事件驱动路径未覆盖。

## 4. 消息类未做

- **普通文件 `file_type=4`** — `POST /v2/groups|users/{openid}/files`，`file_type` 1 图片/2 视频/3 语音/4 文件（200MB），超过软限制会降级为文件类型上传（`rich-media.html`）。沙箱显式返回"file_type 4 is not supported"（`sim.rs:26`）。影响：发 PDF/zip 等真实需求在沙箱里必失败，且"软限制降级"路径无法验证。
- **图文卡片 `msg_type=8`** — 群/单聊均支持，`card.type=tuwen` + `title(1-50)/description(1-200)/url(https)/pic_url(https)`（2026-10-28 新增）。影响：新版客户端主推形态之一，沙箱直接拒。
- **输入中状态 `msg_type=6`** — 仅单聊，`input_notify` 字段。影响：流式/打字机效果的前置交互无法模拟。
- **流式消息（1 个接口）** — `POST /v2/users/{user_openid}/stream_messages`，`stream_msg_id`/`index`/`input_state(1|10)`/`input_mode(append|replace)`/`content_raw`，50 QPS；群聊明确"不支持流式参数"。影响：LLM 类机器人最常用的输出方式没有沙箱支持。
- **`event_id` 被动回复的事件白名单** — 发消息接口的 `event_id` 与 `msg_id` 二选一。沙箱已支持按 `event_id` 走被动回复（不扣主动配额），但只要求事件属于该 Bot，未校验官方列出的白名单（群 `INTERACTION_CREATE`/`GROUP_ADD_ROBOT`/`GROUP_MSG_RECEIVE`，单聊 `INTERACTION_CREATE`/`C2C_MSG_RECEIVE`/`FRIEND_ADD`）；`GROUP_MSG_RECEIVE`/`C2C_MSG_RECEIVE` 事件本身也没有投递。影响：非法 event_id 组合在沙箱里比真机宽松。
- **互动召回 `is_wakeup`** — 仅单聊，与 `msg_id`/`event_id` 互斥；用户主动对话后 30 天内 4 个周期各 1 条（当天/1-3/3-7/7-30 天）。影响：召回消息的配额与互斥校验完全没有。
- **文本交互标签的真机校验** — `<qqbot-at-user id="" />`、`<qqbot-at-everyone />`、`<qqbot-cmd-enter text="" />`、`<qqbot-cmd-input text="" show="" reference="" />` 已按 `trans/text-chain.html` 解析：GUI 渲染 @ 标签与可点击指令，点击 `/回车指令` 以用户身份推送（群里自动 @ 发消息的 Bot），`/参数指令` 回填输入框并支持 `reference` 引用；控制 API 是 `POST /_mqs/messages/{id}/cmd`。仍缺：@全员 的「机器人需群管理员、每群每天 1 次」限制、`text`/`show` 的 100 字符上限、频道专属的 `<#channel_id>`/`<emoji:id>`，以及旧格式 `<@userid>`/`@everyone` 的解析（群 @ 消息的 `content` 仍用 `<@!openid>` 前缀拼接）。
- **按钮能力仍缺一部分** — 官方 `Button` 还有 `group_id`（同组按钮点击后互斥变灰）、`render_data.style` 3/4、`action.modal`（二次确认，`content`≤40 字、确认/取消文案≤4 字）、`action.anchor`（选图器）、`permission.type=0|1|2`（指定用户/管理员/所有人）、键盘上限 5 行×5 按钮（错误码 40034029）、`keyboard.id` 模版短形式。沙箱已按官方语义处理 `action.type`（0 链接 / 1 回调 / 2 指令）以及指令按钮的 `enter`（`true` 直接发送，默认 `false` 只把 `@bot + data` 填进输入框）和 `reply`（发送时引用按钮所在消息）；`anchor` 只回传字段，没有手机端选图器。影响：按钮权限、互斥与二次确认的回归测试仍缺。
- **入站消息的扩展字段** — `message_type`（0 文本/3 结构化卡片/103 引用消息）、`message_scene.ext`（`msg_idx`/`ref_msg_idx`/`auth_token`）、`ark_data`（`ark_type` 8 种取值）、`msg_elements`、`attachments.width/height/voice_wav_url/asr_refer_text`（`autogen/event/group_message_create.html`）。沙箱 payload 无这些字段。影响：引用回复、卡片解析、语音 ASR 三类逻辑无法被测试覆盖。
- **`message_reference.message_id` 的取值口径** — 官方为 `REFIDX_*`：非机器人消息取 `message_scene.ext` 的 `msg_idx`，机器人自己发的取响应 `ext_info.ref_idx`。沙箱用内部 `msg_id`，与真机不通用。
- **撤回的 2 分钟限制已实现，但错误码是近似的** — 超过 2 分钟返回 40034；官方页面未给出这一分支的专用码。
- **错误码未细分** — 官方单发消息页给出 22006（类型与内容不匹配）、304080（文件信息无效）、304061（内容无效）、304064（订阅消息未授权）、40034005（msg_id 过期）、40034024/40034025/40034026/40034027（msg_id/event_id 越权·过期·不支持回复）、40034029（键盘超限）、40054002（机器人被禁言）、40054005（消息被去重）、40054007（长度超限）、40054010（不允许发 URL）等；沙箱只用 10001/40034/40040/40054/40055/40056/40057。影响：SDK 侧按码分支的逻辑无法在沙箱验证。
- **素材管理（4 个端点全缺 + 预签名直传）** — `POST /v2/media-images/prepare`、`PUT <presigned_url>`（直传二进制）、`POST /v2/media-images/complete`、`GET /v2/media-images`、`DELETE /v2/media-images/{image_id}/delete`；返回 `qqbot-media://image/v1/<token>` 供 Markdown 引用，单机器人上限 100 张、引用后 90 天滑动过期、upload_id 4 小时有效（2026-10-21 新增，`material-management.html`）。
- **消息审核（intent `1<<27`）** — `MESSAGE_AUDIT_PASS`/`MESSAGE_AUDIT_REJECT` 与主动消息审核结果（`message/overview.html` 提到 `MESSAGE_AUDIT` 返回 `MessageAudited`）。
- **模板消息** — 官方的 `MessageMarkdown.template_id`/`custom_template_id` 均已标注**【已废弃】**，没有独立的"模板消息"接口；现存"模板"只有按钮模版 `keyboard.id`（见上）。因此本项不是缺口，仅 `keyboard.id` 短形式未实现。

## 5. 事件未做

沙箱已投递：`GROUP_AT_MESSAGE_CREATE`、`GROUP_MESSAGE_CREATE`、`C2C_MESSAGE_CREATE`、`INTERACTION_CREATE`（仅 type=11）、`GROUP_ADD_ROBOT`、`GROUP_DEL_ROBOT`、`GROUP_MSG_RECEIVE`、`GROUP_MSG_REJECT`。其余全部未投递：

- **`1<<24` GROUP_MEMBER_EVENT** — `GROUP_JOIN_REQUEST`、`GROUP_MEMBER_ADD`、`GROUP_MEMBER_REMOVE`。注意：官方 intents 总表（`interface-framework/event-emit.html`）里没有 `1<<24`，只在这三个事件页的"Intent"字段里出现，沙箱 `intent_for` 因而不认识它。
- **`1<<25` 内未实现** — `C2C_MSG_RECEIVE`、`C2C_MSG_REJECT`（沙箱只有群的同名两个）、`SUBSCRIBE_MESSAGE_STATUS`（订阅消息授权结果，含 `subscribe_id`/`op=1|2`）。订阅消息的**发送接口文档未给出具体路径**，只有错误码 304064 与 304062 佐证其存在。
- **`1<<27` MESSAGE_AUDIT** — `MESSAGE_AUDIT_PASS`/`MESSAGE_AUDIT_REJECT`。
- **`1<<28` FORUMS_EVENT** — `FORUM_THREAD_CREATE/UPDATE/DELETE`、`FORUM_POST_CREATE/DELETE`、`FORUM_REPLY_CREATE/DELETE`、`FORUM_PUBLISH_AUDIT_RESULT`。
- **`1<<29` AUDIO_ACTION** — `AUDIO_START`、`AUDIO_FINISH`、`AUDIO_ON_MIC`、`AUDIO_OFF_MIC`。
- **`1<<10`/`1<<30`/`1<<9`/`1<<12`/`1<<0`/`1<<1`** — `MESSAGE_REACTION_ADD/REMOVE`、`AT_MESSAGE_CREATE`、`PUBLIC_MESSAGE_DELETE`、`MESSAGE_CREATE/DELETE`、`DIRECT_MESSAGE_CREATE/DELETE`、`GUILD_*`、`GUILD_MEMBER_*`、`CHANNEL_*`（随频道族移除）。
- **`INTERACTION_CREATE` 只投 type=11** — 官方还有 12 单聊快捷菜单、13 消息反馈、14 清空会话、15 进出故事集、16 切换模型、18 用户授权、19 群授权、20 群授权状态变更；其中仅 11/12 需要 `PUT /v2/interactions/{interaction_id}` 回应，其余不应回应。沙箱一律可 ack。`INTERACTION_CREATE.chat_type` 已按官方修正为群=1、单聊=2（0=频道不适用）。
- **事件体字段缺口** — 官方 `INTERACTION_CREATE` 还带 `version`、`scene`、`group_member_openid`、`data.resolved.message_scene`、`authorize_data` 等。

## 6. 鉴权/网关未做

- **token 复用与提前刷新（已实现）** — 有效期内重复获取返回同一个 token，最后 60 秒内获取会发新 token，旧 token 到过期前仍可用；`expires_in` 返回剩余秒数。与官方语义一致。
- **intent 无权限的拒绝** — 官方：传了无权限的 intents，gateway 直接报错关连接（关闭码 4013/4014）。沙箱只在投递时按位过滤（`intent_for` 只认 `1<<25`/`1<<26`），未订阅或未映射的位静默丢事件、连接照常。影响：订阅范围写错的 bug 在沙箱里不可见，SDK 会带着这个错误上真机。
- **关闭码缺口** — 官方 4001 无效 opcode、4007 seq 错误、4008 发送过快、4011 guild 过多、4012 无效 version、4013 无效 intent、4014 intent 无权限、4900~4913 内部错误、4914 已下架、4915 已封禁。沙箱只有 4002/4004/4006/4009/4010；注意 **4004 不在官方码表里**，文档对"token 无效"只给了 4014，沙箱的 4004 属于自行约定（见 `docs/COMPAT.md`）。
- **分片与连接数** — `GET /gateway/bot` 返回 `shards`、`session_start_limit{total,remaining,reset_after,max_concurrency}`；官方规定连接数不得超过 `remaining`，分片规则 `shard_id = (guild_id >> 22) % num_shards`。沙箱恒定 `shards:1`、`remaining:999` 且不递减、不校验连接数；**群聊/单聊的分片归属规则文档未给出**，沙箱暂用会话 id 的 crc32。
- **webhook 部署形态** — 官方回调地址由开发者在管理端配置，端口只允许 80/443/8080/8443，请求头带 `User-Agent: QQBot-Callback`、`X-Bot-Appid`；沙箱是固定路径 `POST /webhook/{bot_id}`。影响：端口白名单、多 Bot 共用一个回调地址两种真实部署形态都没有覆盖。
- **webhook 签名细节** — 官方 `op=13` 校验：`seed = secret` 反复重复至 ≥32 字节后截断为 Ed25519 seed，签名内容为 `event_ts + plain_token`，返回 `{plain_token, signature}`。沙箱实现了握手，但 seed 派生与时间戳拼接需与真机对齐（见 §7）。**失败重试策略文档未给出**，故不列为缺口。
- **secret 轮换 API** — 文档只提供 `POST /app/getAppAccessToken`，**未给出轮换/重置 AppSecret 的接口路径**，本项不作为缺口。

## 7. 工程/运维未做

- **secret 明文落盘** — 沙箱把 AppSecret 存在 SQLite（`bots.secret`），未接系统钥匙串（macOS Keychain / Windows Credential Manager / libsecret）。影响：演示机或共享机器上等于明文配置。
- **`--record` / 可重放 fixture** — 目前只有 `profile export/import` 快照，没有"录制真实流量→回放"的能力；官方文档不涉及，属工具链缺口。影响：与真机比对只能靠人肉抓包。
- **多 profile 并发** — CLI 有 `--profile` 与 `profiles.json` 索引，但多 profile 同时 serve、端口/ `runtime.json` 冲突的路径未进 CI。影响：一台机器同时跑"干净沙箱 + 演示沙箱"要手工改端口。
- **SDK 实测矩阵** — CI 只跑 botpy + `mqs-compat` 垫片；Koishi、NoneBot、`qq-bot-sdk`（Node）未纳入。影响：这三个生态最常用的库一旦按新路径（`stream_messages`、`media-images`、`join_approval_strategy`）改造，沙箱无法提前发现不兼容。
- **错误码真机校正** — 见 §4 末条：沙箱的 7 个码与官方单接口页的码表差距很大，需要一次以官方码表为准的逐接口核对（`err_code` 字段名已同时输出：错误体里 `code` 与 `err_code` 同值，两类 SDK 都能读到）。
- **webhook 签名真机对齐** — 用官方文档给出的示例向量（`appid:11111111`、`secret:DG5g3B4j9X2KOErG`、`plain_token:Arq0D5A61EgUu4OxUvOp`、`event_ts:1725442341` → `87befc99…`）做回归用例，目前没有。

## 影响最大的五个缺口

沙箱当前是一个"群聊 + 单聊"的完整沙箱，但把它当"QQ Bot API 沙箱"介绍会立刻在五处露馅：**一是频道族整体缺失**（`/guilds/*`、`/channels/*`、`/users/@me/dms`、频道身份组与 `1<<0/1/9/10/12/30` 事件全无），任何频道机器人示例第一步即 404，这是最大的一块面；**二是消息形态落后于 2026 年的文档**——`msg_type=8` 图文卡片、`msg_type=6` 输入中状态、`stream_messages` 流式消息、`file_type=4` 普通文件、`is_wakeup` 召回、`event_id` 事件白名单五项叠加，恰好覆盖了 LLM 类机器人最常用的输出路径，缺了它们"能跑通真实场景"这个卖点就只剩文本 echo；**三是入站消息 schema 太薄**（无 `message_type=103` 引用、无 `msg_elements`/`ark_data`/`message_scene.ext`、attachments 缺 `voice_wav_url`/`asr_refer_text`），导致引用、卡片、语音三类解析逻辑无法被测试；**四是网关与鉴权的"负面行为"没有模拟**（token 幂等与 60 秒提前刷新、intent 无权限的 4013/4014、4007/4008/4011/4914/4915 关闭码、`session_start_limit` 不递减），这些恰恰是 SDK 作者最容易写错、也最需要沙箱来复现的部分；**五是群管理的 2026-08 新增能力**（入群自动审批策略 6 个接口 + `GROUP_JOIN_REQUEST` 事件 + 素材管理 4 个接口）完全空白，使"验群/拉群"这类真实运营机器人无法在沙箱里闭环。相比之下按钮权限、错误码细分属于"精度问题"，可以在补完上述五项后再逐条校正。

## 参考 URL

- 文档首页与导航：https://bot.q.qq.com/wiki/develop/api-v2/
- API 调用指南（公共错误码、err_code、唯一身份）：https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/api-call-guide.html
- 获取访问凭证（token 有效期与刷新语义）：https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/access-token.html
- 事件订阅 Intents 与分片：https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/interface-framework/event-emit.html
- WebSocket 方式（关闭码、session_start_limit）：https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/event-emit/websocket.html
- Webhook 方式（Ed25519 校验、端口白名单）：https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/event-emit/webhook.html
- 消息收发概述（被动窗口、主动频控、去重、撤回）：https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/overview.html
- 消息类型总表：https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/type/overview.html
- 富媒体与 file_type 1/2/3/4：https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/rich-media.html
- 素材管理 `qqbot-media://`：https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/material-management.html
- 消息按钮（permission/modal/group_id）：https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/trans/msg-btn.html
- 文本交互标签（@用户/@全员/指令）：https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/trans/text-chain.html
- 结构化卡片（ARK）：https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/type/ark.html
- 发送群聊消息（msg_type 0/2/7/8、event_id、card）：https://bot.q.qq.com/wiki/develop/api-v2/autogen/api/v2_groups_group_openid_messages.post.html
- 发送单聊消息（msg_type 6、is_wakeup）：https://bot.q.qq.com/wiki/develop/api-v2/autogen/api/v2_users_user_openid_messages.post.html
- 流式发送单聊消息：https://bot.q.qq.com/wiki/develop/api-v2/autogen/api/v2_users_user_openid_stream_messages.post.html
- 撤回群消息（2 分钟限制）：https://bot.q.qq.com/wiki/develop/api-v2/autogen/api/v2_groups_group_openid_messages_message_id.delete.html
- 群消息事件体（message_scene/ark_data/attachments）：https://bot.q.qq.com/wiki/develop/api-v2/autogen/event/group_message_create.html
- 互动事件（type 11~20、chat_type）：https://bot.q.qq.com/wiki/develop/api-v2/autogen/event/interaction_create.html
- 入群申请事件：https://bot.q.qq.com/wiki/develop/api-v2/autogen/event/group_join_request.html
- 订阅消息状态事件：https://bot.q.qq.com/wiki/develop/api-v2/autogen/event/subscribe_message_status.html
- 入群自动审批策略（6 接口）：https://bot.q.qq.com/wiki/develop/api-v2/autogen/api/v2_groups_join_approval_strategy.get.html
- 查询/修改群禁言：https://bot.q.qq.com/wiki/develop/api-v2/autogen/api/v2_groups_group_openid_restrict_chat_setting.get.html
- 机器人群内状态（member_role/recv_msg_setting）：https://bot.q.qq.com/wiki/develop/api-v2/autogen/api/v2_groups_group_openid_bot_state.get.html
- 自定义菜单：https://bot.q.qq.com/wiki/develop/api-v2/autogen/api/v2_menu.put.html
- 指令面板：https://bot.q.qq.com/wiki/develop/api-v2/autogen/api/v2_panels.post.html
- 生成分享链接：https://bot.q.qq.com/wiki/develop/api-v2/autogen/api/v2_generate_url_link.post.html
- 频道内容（公告/精华/日程）：https://bot.q.qq.com/wiki/develop/api-v2/server-inter/channel/content/announces/delete_guild_announces.html
- 频道身份组与权限：https://bot.q.qq.com/wiki/develop/api-v2/server-inter/channel/role-group/get_guild_roles.html
- 表情表态对象：https://bot.q.qq.com/wiki/develop/api-v2/openapi/reaction/model.html
- 变更记录（2026-08/09/10 新增项）：https://bot.q.qq.com/wiki/develop/api-v2/changelog.html
