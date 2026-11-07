-- MQS schema v2: guild family
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
