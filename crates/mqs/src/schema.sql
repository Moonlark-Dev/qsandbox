-- QSandbox schema v4 (Rust rewrite, single baseline: guild/channel family removed)
-- Single-writer SQLite. Every change goes through the domain layer, never the API.

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bots (
    id                   TEXT PRIMARY KEY,              -- app_id is the bot id
    app_id               TEXT NOT NULL UNIQUE,
    secret               TEXT NOT NULL,
    name                 TEXT NOT NULL,
    avatar               TEXT NOT NULL DEFAULT '',
    scopes               TEXT NOT NULL DEFAULT '[]',
    delivery             TEXT NOT NULL DEFAULT 'ws',    -- ws | webhook
    whitelisted          INTEGER NOT NULL DEFAULT 0,    -- 内邀接口白名单
    allow_proactive_msg  INTEGER NOT NULL DEFAULT 1,
    created_at           INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS users (
    id         TEXT PRIMARY KEY,
    nickname   TEXT NOT NULL,
    avatar     TEXT NOT NULL DEFAULT '',
    gender     INTEGER NOT NULL DEFAULT 0,
    age        INTEGER NOT NULL DEFAULT 0,
    city       TEXT NOT NULL DEFAULT '',
    actor_type TEXT NOT NULL DEFAULT 'human', -- human | npc | bot
    seed       INTEGER NOT NULL DEFAULT 0,
    behavior   TEXT NOT NULL DEFAULT 'normal',
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS groups (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    owner_id    TEXT,
    join_policy TEXT NOT NULL DEFAULT 'open',
    avatar      TEXT NOT NULL DEFAULT '',
    finger_memo TEXT NOT NULL DEFAULT '',
    class_text  TEXT NOT NULL DEFAULT '',
    tags        TEXT NOT NULL DEFAULT '[]',
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS group_members (
    group_id   TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role       TEXT NOT NULL DEFAULT 'member',  -- owner | admin | member
    joined_at  INTEGER NOT NULL,
    mute_until INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (group_id, user_id)
);

CREATE TABLE IF NOT EXISTS group_bots (
    group_id            TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    bot_id              TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    joined_at           INTEGER NOT NULL,
    allow_proactive_msg INTEGER NOT NULL DEFAULT 1,
    recv_msg_setting    TEXT NOT NULL DEFAULT 'all',  -- all | only_mention | mention_and_context
    member_role         TEXT NOT NULL DEFAULT 'member', -- member | admin | owner（机器人在群内的角色）
    PRIMARY KEY (group_id, bot_id)
);

CREATE TABLE IF NOT EXISTS group_blacklist (
    group_id  TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id   TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    banned_at INTEGER NOT NULL,
    PRIMARY KEY (group_id, user_id)
);

CREATE TABLE IF NOT EXISTS group_join_requests (
    id             TEXT PRIMARY KEY,
    group_id       TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id        TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    apply_at       INTEGER NOT NULL,
    source         TEXT NOT NULL DEFAULT 'self_apply', -- self_apply | invited
    invited_by     TEXT NOT NULL DEFAULT '',
    verify_message TEXT NOT NULL DEFAULT '',
    risk_tips      TEXT NOT NULL DEFAULT '',
    status         TEXT NOT NULL DEFAULT 'pending',    -- pending | approved | declined
    decided_at     INTEGER NOT NULL DEFAULT 0,
    reject_reason  TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS conversations (
    id       TEXT PRIMARY KEY,                 -- group:<gid> | dm:<bot>:<uid>
    type     TEXT NOT NULL,                    -- group | dm
    peer_id  TEXT NOT NULL,
    bot_id   TEXT,
    last_seq INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS openid_map (
    bot_id       TEXT NOT NULL,
    kind         TEXT NOT NULL,                 -- user | group
    entity_id    TEXT NOT NULL,
    openid       TEXT NOT NULL,
    union_openid TEXT NOT NULL,
    PRIMARY KEY (bot_id, kind, entity_id),
    UNIQUE (bot_id, openid)
);

CREATE TABLE IF NOT EXISTS messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    msg_id          TEXT NOT NULL UNIQUE,
    event_id        TEXT,
    direction       TEXT NOT NULL,             -- in | out
    sender_id       TEXT NOT NULL,
    sender_type     TEXT NOT NULL,             -- user | bot
    bot_id          TEXT,
    msg_type        INTEGER NOT NULL DEFAULT 0,
    content         TEXT NOT NULL DEFAULT '',
    raw_json        TEXT NOT NULL DEFAULT '{}',
    sent_at         INTEGER NOT NULL,
    recalled_at     INTEGER NOT NULL DEFAULT 0,
    UNIQUE (conversation_id, seq)
);

CREATE TABLE IF NOT EXISTS events (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id        TEXT NOT NULL,
    type            TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    conversation_id TEXT,
    target_bot_id   TEXT NOT NULL,
    created_at      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS gateway_sessions (
    session_id  TEXT PRIMARY KEY,
    bot_id      TEXT NOT NULL,
    intents     INTEGER NOT NULL DEFAULT 0,
    last_seq    INTEGER NOT NULL DEFAULT 0,
    shard_id    INTEGER NOT NULL DEFAULT 0,
    shard_count INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL,
    closed_at   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS webhook_endpoints (
    id         TEXT PRIMARY KEY,
    bot_id     TEXT NOT NULL,
    url        TEXT NOT NULL,
    secret     TEXT NOT NULL,
    enabled    INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS app_tokens (
    token      TEXT PRIMARY KEY,
    bot_id     TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS media (
    hash       TEXT PRIMARY KEY,
    mime       TEXT NOT NULL,
    size       INTEGER NOT NULL,
    path       TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS reply_windows (
    msg_id     TEXT NOT NULL,
    bot_id     TEXT NOT NULL,
    count      INTEGER NOT NULL DEFAULT 0,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (msg_id, bot_id)
);

CREATE TABLE IF NOT EXISTS reply_keys (
    bot_id     TEXT NOT NULL,
    msg_id     TEXT NOT NULL,
    msg_seq    INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (bot_id, msg_id, msg_seq)
);

-- 流式单聊消息：一条 stream 对应会话里的一条消息，`sent_content` 是已下发前缀。
-- 沙箱即时下发（没有网络/打字机延迟），所以没有独立的 pending 列：
-- 每次请求处理完，sent_content 就是当前全文，`input_mode=replace` 的前缀校验以它为准。
CREATE TABLE IF NOT EXISTS stream_messages (
    stream_msg_id      TEXT PRIMARY KEY,             -- 也是这条消息的 msg_id / 响应 id
    bot_id             TEXT NOT NULL,
    user_id            TEXT NOT NULL,                -- 目标用户（内部 id）
    conversation_id    TEXT NOT NULL,
    content_type       TEXT NOT NULL DEFAULT 'text', -- text | markdown
    sent_content       TEXT NOT NULL DEFAULT '',     -- 已下发内容（replace 必须以此为前缀）
    input_state        INTEGER NOT NULL DEFAULT 1,   -- 1 生成中 | 10 生成结束
    last_index         INTEGER NOT NULL DEFAULT 0,
    last_content_raw   TEXT NOT NULL DEFAULT '',     -- 上一分片的原始内容（用于重放判定）
    last_input_mode    TEXT NOT NULL DEFAULT 'append',
    last_input_state   INTEGER NOT NULL DEFAULT 1,
    ref_idx            TEXT NOT NULL DEFAULT '',     -- 被动回复的 REFIDX_* 假值
    finished_at        INTEGER NOT NULL DEFAULT 0,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS active_quota (
    bot_id          TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    day             TEXT NOT NULL,
    used            INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (bot_id, conversation_id, day)
);

CREATE TABLE IF NOT EXISTS frames (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    direction  TEXT NOT NULL,                 -- in | out | internal
    kind       TEXT NOT NULL,                 -- http | ws | event
    bot_id     TEXT,
    session_id TEXT,
    summary    TEXT NOT NULL DEFAULT '',
    data_json  TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS rules (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    event_type     TEXT NOT NULL,
    condition_json TEXT NOT NULL DEFAULT '{}',
    action_json    TEXT NOT NULL DEFAULT '{}',
    delay_ms       INTEGER NOT NULL DEFAULT 0,
    enabled        INTEGER NOT NULL DEFAULT 1,
    created_at     INTEGER NOT NULL
);


-- 素材管理（material-management）：Markdown 图片先上传、再以 qqbot-media:// 引用。
-- 图片与 Bot 绑定；token 是 qqbot-media URL 里的引用标记，上传任务单独一张表。
CREATE TABLE IF NOT EXISTS media_images (
    id           TEXT PRIMARY KEY,                 -- image_id，形如 <app_id>_<rand>
    bot_id       TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    token        TEXT NOT NULL UNIQUE,             -- qqbot-media://image/v1/<token>
    media_hash   TEXT NOT NULL,                    -- media 表的内容哈希（预览/转存用）
    file_name    TEXT NOT NULL,
    content_type TEXT NOT NULL,
    size         INTEGER NOT NULL,
    md5          TEXT NOT NULL,
    sha1         TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,                 -- 滑动过期：最后一次被引用 + 90 天
    deleted_at   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS media_image_uploads (
    id                 TEXT PRIMARY KEY,           -- upload_id（4 小时有效）
    bot_id             TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    file_name          TEXT NOT NULL,
    content_type       TEXT NOT NULL,
    size               INTEGER NOT NULL,
    md5                TEXT NOT NULL,
    sha1               TEXT NOT NULL,
    presign_token      TEXT NOT NULL,              -- 模拟 COS 预签名 URL 的 sign 参数
    presign_expires_at INTEGER NOT NULL,
    created_at         INTEGER NOT NULL,
    expires_at         INTEGER NOT NULL,
    uploaded_at        INTEGER NOT NULL DEFAULT 0,
    etag               TEXT NOT NULL DEFAULT '',   -- PUT 时算出的文件 MD5
    media_hash         TEXT NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS idx_media_images_bot ON media_images(bot_id, deleted_at, expires_at, created_at);

CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation_id, seq);
CREATE INDEX IF NOT EXISTS idx_messages_sent ON messages(sent_at);
CREATE INDEX IF NOT EXISTS idx_events_bot ON events(target_bot_id, seq);
CREATE INDEX IF NOT EXISTS idx_events_conv ON events(conversation_id, seq);
CREATE INDEX IF NOT EXISTS idx_frames_id ON frames(id);
CREATE INDEX IF NOT EXISTS idx_openid_lookup ON openid_map(bot_id, openid);
CREATE INDEX IF NOT EXISTS idx_members_user ON group_members(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_bot ON gateway_sessions(bot_id, closed_at);
CREATE INDEX IF NOT EXISTS idx_group_join_pending ON group_join_requests(group_id, status, apply_at);
