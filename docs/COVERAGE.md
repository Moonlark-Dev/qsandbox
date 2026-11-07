# QSandbox 接口覆盖率

> 自动生成 · 文档快照 2026-11-07 · 请勿手改

**31 endpoints — 30 implemented, 1 stubs, 0 n/a**

| 层级 | 总数 | full | partial | stub | n/a |
|---|---|---|---|---|---|
| L0 | 6 | 6 | 0 | 0 | 0 |
| L1 | 11 | 9 | 2 | 0 | 0 |
| L2 | 11 | 10 | 0 | 1 | 0 |
| L3 | 3 | 2 | 1 | 0 | 0 |

## 明细

| 方法 | 路径 | 覆盖 | 鉴权 | 分层 | 说明 |
|---|---|---|---|---|---|
| `POST` | `/app/getAppAccessToken` | full | appid+secret | L0 | 获取 App Access Token（7200s） |
| `GET` | `/gateway` | full | Bot Token | L0 | 获取网关 WebSocket 地址 |
| `GET` | `/gateway/bot` | full | Bot Token | L0 | 获取网关地址与分片建议 |
| `GET` | `/users/@me` | full | Bot Token | L0 | 获取机器人自身信息 |
| `POST` | `/v2/groups/{group_openid}/messages` | full | Bot Token | L0 | 发送群聊消息（被动 / 主动） |
| `POST` | `/v2/users/{openid}/messages` | full | Bot Token | L0 | 发送单聊消息（被动 / 主动） |
| `GET` | `/media/{hash}` | full | 无 | L1 | 沙箱媒体下载（内容哈希寻址） |
| `POST` | `/v2/groups/{group_openid}/files` | full | Bot Token | L1 | 上传群聊富媒体文件 |
| `DELETE` | `/v2/groups/{group_openid}/messages/{message_id}` | partial | Bot Token | L1 | 撤回群聊消息 |
| `POST` | `/v2/groups/{group_openid}/upload_part_finish` | full | Bot Token | L1 | 群聊富媒体分片上传：完成分片 |
| `POST` | `/v2/groups/{group_openid}/upload_prepare` | full | Bot Token | L1 | 群聊富媒体分片上传：预上传 |
| `PUT` | `/v2/interactions/{interaction_id}` | full | Bot Token | L1 | 确认按钮回调结果（code 0 成功） |
| `PUT` | `/v2/uploads/{upload_id}/parts/{index}` | full | 无 | L1 | 富媒体分片上传：上传分片（预签名 URL） |
| `POST` | `/v2/users/{openid}/files` | full | Bot Token | L1 | 上传单聊富媒体文件 |
| `DELETE` | `/v2/users/{openid}/messages/{message_id}` | partial | Bot Token | L1 | 撤回单聊消息 |
| `POST` | `/v2/users/{openid}/upload_part_finish` | full | Bot Token | L1 | 单聊富媒体分片上传：完成分片 |
| `POST` | `/v2/users/{openid}/upload_prepare` | full | Bot Token | L1 | 单聊富媒体分片上传：预上传 |
| `GET` | `/channels/{channel_id}` | full | Bot Token | L2 | 获取子频道信息 |
| `GET` | `/channels/{channel_id}/messages` | full | Bot Token | L2 | 获取频道消息列表 |
| `POST` | `/channels/{channel_id}/messages` | full | Bot Token | L2 | 发送频道消息 |
| `GET` | `/channels/{channel_id}/messages/{message_id}` | full | Bot Token | L2 | 获取频道消息 |
| `GET` | `/guilds/{guild_id}` | full | Bot Token | L2 | 获取频道信息 |
| `GET` | `/guilds/{guild_id}/channels` | full | Bot Token | L2 | 获取子频道列表 |
| `GET` | `/guilds/{guild_id}/members` | full | Bot Token | L2 | 获取频道成员列表 |
| `GET` | `/guilds/{guild_id}/members/{user_id}` | full | Bot Token | L2 | 获取单个频道成员 |
| `GET` | `/guilds/{guild_id}/roles` | full | Bot Token | L2 | 获取频道身份组列表 |
| `POST` | `/users/@me/dms` | stub | Bot Token | L2 | 创建私信会话（频道私信） |
| `GET` | `/users/@me/guilds` | full | Bot Token | L2 | 获取机器人所在频道列表 |
| `GET` | `/v2/groups/{group_openid}/members` | full | Bot Token | L3 | 获取群成员列表（内邀 / 白名单接口） |
| `GET` | `/v2/groups/{group_openid}/members/{member_openid}` | full | Bot Token | L3 | 获取群成员信息（内邀 / 白名单接口） |
| `POST` | `/webhook/{bot_id}` | partial | 无 | L3 | Webhook 回调入口（含 URL 校验握手） |

覆盖等级：`full` 行为保真 · `partial` 字段或分支不全 · `stub` 仅返回结构 · `n/a` 明确不模拟。
