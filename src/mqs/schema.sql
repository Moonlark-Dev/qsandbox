-- MQS schema v1
-- Single-writer SQLite. Every change goes through the domain layer, never the API.

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bots (
    id         TEXT PRIMARY KEY,              -- app_id is the bot id
    app_id     TEXT NOT NULL UNIQUE,
    secret     TEXT NOT NULL,
    name       TEXT NOT NULL,
    avatar     TEXT NOT NULL DEFAULT '',
    scopes     TEXT NOT NULL DEFAULT '[]',
    delivery   TEXT NOT NULL DEFAULT 'ws',    -- ws | webhook
    created_at INTEGER NOT NULL
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
    group_id  TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    bot_id    TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    joined_at INTEGER NOT NULL,
    PRIMARY KEY (group_id, bot_id)
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

CREATE TABLE IF NOT EXISTS rate_buckets (
    key        TEXT PRIMARY KEY,
    tokens     REAL NOT NULL,
    updated_at INTEGER NOT NULL
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

CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation_id, seq);
CREATE INDEX IF NOT EXISTS idx_messages_sent ON messages(sent_at);
CREATE INDEX IF NOT EXISTS idx_events_bot ON events(target_bot_id, seq);
CREATE INDEX IF NOT EXISTS idx_events_conv ON events(conversation_id, seq);
CREATE INDEX IF NOT EXISTS idx_frames_id ON frames(id);
CREATE INDEX IF NOT EXISTS idx_openid_lookup ON openid_map(bot_id, openid);
CREATE INDEX IF NOT EXISTS idx_members_user ON group_members(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_bot ON gateway_sessions(bot_id, closed_at);

-- ---------------------------------------------------------------- guild family (L2)
CREATE TABLE IF NOT EXISTS guilds (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    owner_id   TEXT,
    icon       TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS channels (
    id         TEXT PRIMARY KEY,
    guild_id   TEXT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    type       INTEGER NOT NULL DEFAULT 0,   -- 0 text, 2 voice
    position   INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS guild_members (
    guild_id   TEXT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    user_id    TEXT NOT NULL,
    nickname   TEXT NOT NULL DEFAULT '',
    member_type TEXT NOT NULL DEFAULT 'user', -- user | bot
    roles      TEXT NOT NULL DEFAULT '[]',
    joined_at  INTEGER NOT NULL,
    PRIMARY KEY (guild_id, user_id)
);

CREATE TABLE IF NOT EXISTS guild_roles (
    id          TEXT PRIMARY KEY,
    guild_id    TEXT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    color       INTEGER NOT NULL DEFAULT 0,
    hoist       INTEGER NOT NULL DEFAULT 0,
    permissions TEXT NOT NULL DEFAULT '0'
);

CREATE INDEX IF NOT EXISTS idx_channels_guild ON channels(guild_id, position);
CREATE INDEX IF NOT EXISTS idx_guild_members ON guild_members(guild_id);
