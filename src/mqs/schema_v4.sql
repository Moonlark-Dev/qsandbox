-- MQS schema v4: group management, channel writes/roles, channel DMs, friends,
-- proactive-message switches and user rich-media messages.

-- ---------------------------------------------------------------- groups
ALTER TABLE groups ADD COLUMN finger_memo TEXT NOT NULL DEFAULT '';
ALTER TABLE groups ADD COLUMN class_text TEXT NOT NULL DEFAULT '';
ALTER TABLE groups ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';

ALTER TABLE group_bots ADD COLUMN allow_proactive_msg INTEGER NOT NULL DEFAULT 1;
ALTER TABLE group_bots ADD COLUMN recv_msg_setting TEXT NOT NULL DEFAULT 'all';

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

CREATE INDEX IF NOT EXISTS idx_group_join_pending
    ON group_join_requests(group_id, status, apply_at);

-- ---------------------------------------------------------------- bots / friends
ALTER TABLE bots ADD COLUMN allow_proactive_msg INTEGER NOT NULL DEFAULT 1;

CREATE TABLE IF NOT EXISTS friends (
    bot_id     TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scene      INTEGER NOT NULL DEFAULT 1000,
    scene_param TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    PRIMARY KEY (bot_id, user_id)
);

-- ---------------------------------------------------------------- guilds
ALTER TABLE guilds ADD COLUMN mute_until INTEGER NOT NULL DEFAULT 0;

ALTER TABLE channels ADD COLUMN sub_type INTEGER NOT NULL DEFAULT 0;
ALTER TABLE channels ADD COLUMN parent_id TEXT NOT NULL DEFAULT '0';
ALTER TABLE channels ADD COLUMN owner_id TEXT NOT NULL DEFAULT '';
ALTER TABLE channels ADD COLUMN private_type INTEGER NOT NULL DEFAULT 0;
ALTER TABLE channels ADD COLUMN speak_permission INTEGER NOT NULL DEFAULT 1;

ALTER TABLE guild_members ADD COLUMN mute_until INTEGER NOT NULL DEFAULT 0;

-- ---------------------------------------------------------------- channel DMs
CREATE TABLE IF NOT EXISTS dm_sessions (
    dms_guild_id    TEXT PRIMARY KEY,
    channel_id      TEXT NOT NULL,
    bot_id          TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source_guild_id TEXT NOT NULL DEFAULT '',
    created_at      INTEGER NOT NULL,
    UNIQUE (bot_id, user_id)
);

CREATE TABLE IF NOT EXISTS dms_quota (
    bot_id  TEXT NOT NULL,
    user_id TEXT NOT NULL,
    day     TEXT NOT NULL,
    used    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (bot_id, user_id, day)
);
