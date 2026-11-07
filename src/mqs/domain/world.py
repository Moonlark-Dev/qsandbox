"""The world model: users, bots, groups, conversations, messages, events."""

from __future__ import annotations

import asyncio
import json
import time
from collections.abc import Iterable, Sequence
from datetime import datetime
from typing import Any

import aiosqlite

from ..clock import Clock
from ..config import ProfilePaths
from ..db import Database
from ..errors import (
    BotOffline,
    DuplicateReply,
    InvalidAppId,
    InvalidParam,
    InvalidSecret,
    MessageNotFound,
    NoPermission,
    PassiveWindowExpired,
    QuotaExhausted,
)
from ..ids import (
    FakeData,
    new_event_id,
    new_msg_id,
    new_session_id,
    openid_for,
    union_openid_for,
)
from .hub import Hub
from .ratelimit import RateLimiter
from .webhooks import WebhookDispatcher

BOT_SCOPE_GROUP = "group_message"
BOT_SCOPE_C2C = "c2c_message"
BOT_SCOPE_MEDIA = "media"
BOT_SCOPE_MEMBER = "member"


def _rows(rows: Iterable[aiosqlite.Row]) -> list[dict[str, Any]]:
    return [dict(r) for r in rows]


class World:
    def __init__(self, db: Database, clock: Clock, profile: str, paths: ProfilePaths) -> None:
        self.db = db
        self.clock = clock
        self.profile = profile
        self.paths = paths
        self.settings: dict[str, Any] = {}
        self.fake = FakeData(42)
        self.hub = Hub()
        self.rate = RateLimiter(clock, "loose")
        self.webhooks = WebhookDispatcher(self)
        self.gateway: Any = None  # set by server after construction
        self.started_at = int(time.time() * 1000)
        self.boot_id = new_session_id()
        self._rule_tasks: set[asyncio.Task] = set()
        self._frame_writes = 0
        self._lock = asyncio.Lock()

    # ------------------------------------------------------------------ lifecycle
    async def start(self) -> None:
        self.settings = await self.db.all_settings()
        self.clock.restore(self.settings.get("clock_mode", "real"), self.settings.get("clock_now_ms") or self.clock.now_ms())
        self.started_at = int(time.time() * 1000)
        self.rate = RateLimiter(self.clock, self.settings.get("rate_mode", "loose"))
        self.fake.reseed(int(self.settings.get("seed", 42)))
        await self._bootstrap()
        await self.db.prune_frames()

    async def stop(self) -> None:
        for task in list(self._rule_tasks):
            task.cancel()
        self._rule_tasks.clear()
        await self.webhooks.close()

    async def _bootstrap(self) -> None:
        """Fresh profile: create a usable default world (one bot, a few users, a group)."""
        bot_count = await self.db.fetchone("SELECT COUNT(*) AS c FROM bots")
        if (bot_count["c"] if bot_count else 0) == 0:
            await self.create_bot(
                app_id="10000",
                secret="mqs-secret-10000",
                name="MQS 测试 Bot",
                scopes=[BOT_SCOPE_GROUP, BOT_SCOPE_C2C, BOT_SCOPE_MEDIA, BOT_SCOPE_MEMBER],
            )
        user_count = await self.db.fetchone("SELECT COUNT(*) AS c FROM users")
        if (user_count["c"] if user_count else 0) == 0:
            await self.create_user(nickname="小满", actor_type="human", behavior="normal")
            await self.create_user(nickname="星见", actor_type="human", behavior="normal")
            for name in ("青禾", "月汐", "阿柚"):
                await self.create_user(nickname=name, actor_type="npc", behavior="silent")
        group_count = await self.db.fetchone("SELECT COUNT(*) AS c FROM groups")
        if (group_count["c"] if group_count else 0) == 0:
            owner = await self.db.fetchone("SELECT id FROM users WHERE actor_type='human' ORDER BY id LIMIT 1")
            members = await self.db.fetchall("SELECT id FROM users ORDER BY id")
            if owner:
                await self.create_group(
                    name="MQS 测试群",
                    owner_id=owner["id"],
                    member_ids=[r["id"] for r in members if r["id"] != owner["id"]],
                )

    # ------------------------------------------------------------------ settings
    def setting(self, key: str, default: Any = None) -> Any:
        return self.settings.get(key, default)

    async def update_settings(self, patch: dict[str, Any]) -> dict[str, Any]:
        mapping = {
            "feature_whitelist": "feature.whitelist",
            "feature_expose": "feature.expose",
            "chat_auto_at_bot": "chat.auto_at_bot",
        }
        for incoming, value in patch.items():
            if value is None:
                continue
            key = mapping.get(incoming, incoming)
            if key == "clock_mode":
                self.clock.set_mode(value)
                await self.db.set_setting("clock_mode", value)
                await self.db.set_setting("clock_now_ms", self.clock.now_ms())
                self.settings["clock_mode"] = value
                self.settings["clock_now_ms"] = self.clock.now_ms()
                continue
            if key == "seed":
                self.fake.reseed(int(value))
            if key == "rate_mode":
                if value is False:
                    value = "off"
                elif value is True:
                    value = "loose"
                self.rate.set_mode(value)
            await self.db.set_setting(key, value)
            self.settings[key] = value
        return self.settings

    async def persist_clock(self) -> None:
        await self.db.set_setting("clock_mode", self.clock.mode)
        await self.db.set_setting("clock_now_ms", self.clock.now_ms())
        self.settings["clock_mode"] = self.clock.mode
        self.settings["clock_now_ms"] = self.clock.now_ms()

    # ------------------------------------------------------------------ serializers
    @staticmethod
    def _avatar(entity_id: str) -> str:
        return f"/avatars/{entity_id}.svg"

    def user_out(self, row: aiosqlite.Row | dict) -> dict[str, Any]:
        data = dict(row)
        data["avatar"] = data.get("avatar") or self._avatar(data["id"])
        return data

    def bot_out(self, row: aiosqlite.Row | dict) -> dict[str, Any]:
        data = dict(row)
        data["avatar"] = data.get("avatar") or self._avatar(f"bot_{data['id']}")
        try:
            data["scopes"] = json.loads(data.get("scopes") or "[]")
        except ValueError:
            data["scopes"] = []
        data["online"] = bool(self.gateway and self.gateway.is_online(data["id"]))
        return data

    async def group_out(self, row: aiosqlite.Row | dict) -> dict[str, Any]:
        data = dict(row)
        gid = data["id"]
        count = await self.db.fetchone("SELECT COUNT(*) AS c FROM group_members WHERE group_id=?", (gid,))
        bots = await self.db.fetchall(
            "SELECT b.id, b.name FROM group_bots gb JOIN bots b ON b.id=gb.bot_id WHERE gb.group_id=?", (gid,)
        )
        owners = await self.db.fetchone("SELECT user_id FROM group_members WHERE group_id=? AND role='owner'", (gid,))
        data["avatar"] = data.get("avatar") or self._avatar(gid)
        data["member_count"] = int(count["c"]) if count else 0
        data["bot_ids"] = [b["id"] for b in bots]
        data["bots"] = [{"id": b["id"], "name": b["name"]} for b in bots]
        data["owner_id"] = owners["user_id"] if owners else None
        data["conversation_id"] = f"group:{gid}"
        return data

    # ------------------------------------------------------------------ users
    async def create_user(
        self,
        nickname: str | None = None,
        *,
        avatar: str | None = None,
        gender: int = 0,
        age: int = 0,
        city: str = "",
        actor_type: str = "human",
        behavior: str = "normal",
        seed: int | None = None,
    ) -> dict[str, Any]:
        seed = self.fake.seed if seed is None else seed
        nickname = nickname or self.fake.nickname()
        async with self.db.tx() as conn:
            cur = await conn.execute("SELECT COUNT(*) AS c FROM users")
            row = await cur.fetchone()
            await cur.close()
            number = int(row["c"]) + 1
            uid = f"u_{number:06d}"
            now = self.clock.now_ms()
            await conn.execute(
                "INSERT INTO users(id,nickname,avatar,gender,age,city,actor_type,seed,behavior,created_at)"
                " VALUES(?,?,?,?,?,?,?,?,?,?)",
                (uid, nickname, avatar or self._avatar(uid), gender, age, city or self.fake.city(),
                 actor_type, seed, behavior, now),
            )
            out = await self._fetchone_conn(conn, "SELECT * FROM users WHERE id=?", (uid,))
        return self.user_out(out)

    async def create_random_users(
        self,
        count: int,
        *,
        seed: int | None = None,
        behavior: str | None = None,
        actor_type: str = "human",
        group_id: str | None = None,
    ) -> list[dict[str, Any]]:
        if count < 1 or count > 500:
            raise InvalidParam("count must be between 1 and 500")
        created = []
        for i in range(count):
            item_seed = (self.fake.seed if seed is None else seed) + i
            user = await self.create_user(
                actor_type=actor_type,
                behavior=behavior or self.fake.behavior(),
                gender=self.fake.gender(),
                age=self.fake.age(),
                city=self.fake.city(),
                seed=item_seed,
            )
            created.append(user)
            if group_id:
                await self.add_member(group_id, user["id"], "member")
        return created

    async def list_users(self, *, actor_type: str | None = None, available: bool = False) -> list[dict[str, Any]]:
        sql = "SELECT * FROM users"
        params: list[Any] = []
        if actor_type:
            sql += " WHERE actor_type=?"
            params.append(actor_type)
        elif available:
            sql += " WHERE actor_type='human'"
        sql += " ORDER BY id"
        rows = await self.db.fetchall(sql, params)
        return [self.user_out(r) for r in rows]

    async def get_user(self, user_id: str) -> dict[str, Any]:
        row = await self.db.fetchone("SELECT * FROM users WHERE id=?", (user_id,))
        if row is None:
            raise InvalidParam(f"user not found: {user_id}")
        return self.user_out(row)

    async def update_user(self, user_id: str, patch: dict[str, Any]) -> dict[str, Any]:
        await self.get_user(user_id)
        fields = {k: v for k, v in patch.items() if v is not None}
        if not fields:
            return await self.get_user(user_id)
        cols = ", ".join(f"{k}=?" for k in fields)
        await self.db.execute(f"UPDATE users SET {cols} WHERE id=?", (*fields.values(), user_id))
        return await self.get_user(user_id)

    async def delete_user(self, user_id: str) -> None:
        await self.get_user(user_id)
        await self.db.execute("DELETE FROM openid_map WHERE entity_id=?", (user_id,))
        await self.db.execute("DELETE FROM users WHERE id=?", (user_id,))

    # ------------------------------------------------------------------ openids
    async def _openid_tx(
        self, conn: aiosqlite.Connection, bot_row: aiosqlite.Row, kind: str, entity_id: str
    ) -> str:
        cur = await conn.execute(
            "SELECT openid FROM openid_map WHERE bot_id=? AND kind=? AND entity_id=?",
            (bot_row["id"], kind, entity_id),
        )
        row = await cur.fetchone()
        await cur.close()
        if row:
            return row["openid"]
        openid = openid_for(bot_row["app_id"], kind, entity_id)
        await conn.execute(
            "INSERT INTO openid_map(bot_id,kind,entity_id,openid,union_openid) VALUES(?,?,?,?,?)",
            (bot_row["id"], kind, entity_id, openid, union_openid_for(entity_id)),
        )
        return openid

    async def openid(self, bot_id: str, kind: str, entity_id: str) -> str:
        bot = await self.bot_by_id(bot_id)
        row = await self.db.fetchone(
            "SELECT openid FROM openid_map WHERE bot_id=? AND kind=? AND entity_id=?", (bot_id, kind, entity_id)
        )
        if row:
            return row["openid"]
        async with self.db.tx() as conn:
            return await self._openid_tx(conn, bot, kind, entity_id)  # type: ignore[arg-type]

    async def resolve_openid(self, bot_id: str, openid: str) -> aiosqlite.Row:
        row = await self.db.fetchone("SELECT * FROM openid_map WHERE bot_id=? AND openid=?", (bot_id, openid))
        if row is None:
            raise InvalidParam(f"unknown openid for this bot: {openid}")
        return row

    # ------------------------------------------------------------------ bots
    async def create_bot(
        self,
        *,
        app_id: str | None = None,
        secret: str | None = None,
        name: str | None = None,
        avatar: str | None = None,
        scopes: Sequence[str] | None = None,
        delivery: str = "ws",
    ) -> dict[str, Any]:
        async with self.db.tx() as conn:
            if not app_id:
                cur = await conn.execute("SELECT MAX(CAST(app_id AS INTEGER)) AS m FROM bots")
                row = await cur.fetchone()
                await cur.close()
                app_id = str(int(row["m"] or 10000) + 1)
            app_id = str(app_id)
            cur = await conn.execute("SELECT id FROM bots WHERE app_id=?", (app_id,))
            exists = await cur.fetchone()
            await cur.close()
            if exists:
                raise InvalidParam(f"bot with app_id {app_id} already exists")
            bot_id = app_id
            secret = secret or f"mqs-secret-{app_id}"
            name = name or self.fake.bot_name()
            scopes = list(scopes) if scopes is not None else [BOT_SCOPE_GROUP, BOT_SCOPE_C2C, BOT_SCOPE_MEDIA]
            await conn.execute(
                "INSERT INTO bots(id,app_id,secret,name,avatar,scopes,delivery,created_at) VALUES(?,?,?,?,?,?,?,?)",
                (bot_id, app_id, secret, name, avatar or self._avatar(f"bot_{bot_id}"),
                 json.dumps(scopes), delivery, self.clock.now_ms()),
            )
            out = await self._fetchone_conn(conn, "SELECT * FROM bots WHERE id=?", (bot_id,))
        return self.bot_out(out)

    async def list_bots(self) -> list[dict[str, Any]]:
        rows = await self.db.fetchall("SELECT * FROM bots ORDER BY id")
        return [self.bot_out(r) for r in rows]

    async def get_bot(self, bot_id: str) -> dict[str, Any]:
        return self.bot_out(await self.bot_by_id(bot_id))

    async def bot_by_id(self, bot_id: str) -> aiosqlite.Row:
        row = await self.db.fetchone("SELECT * FROM bots WHERE id=?", (bot_id,))
        if row is None:
            raise InvalidParam(f"bot not found: {bot_id}")
        return row

    async def bot_by_app_id(self, app_id: str) -> aiosqlite.Row | None:
        return await self.db.fetchone("SELECT * FROM bots WHERE app_id=?", (app_id,))

    async def update_bot(self, bot_id: str, patch: dict[str, Any]) -> dict[str, Any]:
        await self.bot_by_id(bot_id)
        fields: dict[str, Any] = {}
        for key, value in patch.items():
            if value is None:
                continue
            fields[key] = json.dumps(value) if key == "scopes" else value
        if fields:
            cols = ", ".join(f"{k}=?" for k in fields)
            await self.db.execute(f"UPDATE bots SET {cols} WHERE id=?", (*fields.values(), bot_id))
        return await self.get_bot(bot_id)

    async def delete_bot(self, bot_id: str) -> None:
        await self.bot_by_id(bot_id)
        await self.db.execute("DELETE FROM openid_map WHERE bot_id=?", (bot_id,))
        await self.db.execute("DELETE FROM app_tokens WHERE bot_id=?", (bot_id,))
        await self.db.execute("DELETE FROM bots WHERE id=?", (bot_id,))

    async def rotate_secret(self, bot_id: str) -> dict[str, Any]:
        await self.bot_by_id(bot_id)
        secret = f"mqs-secret-{bot_id}-{new_session_id()[:10]}"
        await self.db.execute("UPDATE bots SET secret=? WHERE id=?", (secret, bot_id))
        await self.db.execute("DELETE FROM app_tokens WHERE bot_id=?", (bot_id,))
        return {"bot_id": bot_id, "secret": secret}

    async def issue_token(self, app_id: str, secret: str) -> dict[str, Any]:
        bot = await self.bot_by_app_id(str(app_id))
        if bot is None:
            raise InvalidAppId("app_id not found in this sandbox")
        if secret != bot["secret"]:
            raise InvalidSecret("client_secret does not match")
        token = f"MQS_{new_session_id()}"
        expires_at = self.clock.now_ms() + 7200 * 1000
        await self.db.execute(
            "INSERT INTO app_tokens(token,bot_id,expires_at,created_at) VALUES(?,?,?,?)",
            (token, bot["id"], expires_at, self.clock.now_ms()),
        )
        return {"access_token": token, "expires_in": "7200"}

    async def authenticate_bot(self, token: str) -> dict[str, Any]:
        if token.startswith("QQBot "):
            token = token[6:]
        row = await self.db.fetchone(
            "SELECT b.*, t.expires_at AS token_expires FROM app_tokens t JOIN bots b ON b.id=t.bot_id WHERE t.token=?",
            (token,),
        )
        if row is None:
            from ..errors import InvalidToken

            raise InvalidToken("access token is not valid for this sandbox")
        if int(row["token_expires"]) < self.clock.now_ms():
            from ..errors import TokenExpired

            raise TokenExpired("access token expired")
        return self.bot_out(row)

    # ------------------------------------------------------------------ groups
    async def create_group(
        self,
        *,
        name: str | None = None,
        owner_id: str | None = None,
        member_ids: Sequence[str] | None = None,
        bot_ids: Sequence[str] | None = None,
        join_policy: str = "open",
        random_members: int = 0,
    ) -> dict[str, Any]:
        if random_members:
            extra = await self.create_random_users(random_members)
            member_ids = list(member_ids or []) + [u["id"] for u in extra]
        if not owner_id:
            owner_row = await self.db.fetchone(
                "SELECT id FROM users WHERE actor_type='human' ORDER BY id LIMIT 1"
            ) or await self.db.fetchone("SELECT id FROM users ORDER BY id LIMIT 1")
            if owner_row:
                owner_id = owner_row["id"]
        async with self.db.tx() as conn:
            cur = await conn.execute("SELECT COUNT(*) AS c FROM groups")
            row = await cur.fetchone()
            await cur.close()
            gid = f"g_{int(row['c']) + 1:06d}"
            now = self.clock.now_ms()
            await conn.execute(
                "INSERT INTO groups(id,name,owner_id,join_policy,avatar,created_at) VALUES(?,?,?,?,?,?)",
                (gid, name or self.fake.group_name(), owner_id, join_policy, self._avatar(gid), now),
            )
            await conn.execute(
                "INSERT INTO conversations(id,type,peer_id,bot_id,last_seq) VALUES(?,?,?,?,0)",
                (f"group:{gid}", "group", gid, None),
            )
            members = list(dict.fromkeys(member_ids or []))
            if owner_id and owner_id not in members:
                members.insert(0, owner_id)
            for uid in members:
                await conn.execute(
                    "INSERT OR IGNORE INTO group_members(group_id,user_id,role,joined_at) VALUES(?,?,?,?)",
                    (gid, uid, "owner" if uid == owner_id else "member", now),
                )
            if bot_ids is None:
                cur = await conn.execute("SELECT id FROM bots")
                bot_ids = [r["id"] for r in await cur.fetchall()]
                await cur.close()
            for bid in dict.fromkeys(bot_ids):
                await conn.execute(
                    "INSERT OR IGNORE INTO group_bots(group_id,bot_id,joined_at) VALUES(?,?,?)", (gid, bid, now)
                )
            out = await self._fetchone_conn(conn, "SELECT * FROM groups WHERE id=?", (gid,))
            for bid in dict.fromkeys(bot_ids):
                botrow = await self._fetchone_conn(conn, "SELECT * FROM bots WHERE id=?", (bid,))
                if botrow:
                    await self._openid_tx(conn, botrow, "group", gid)
        return await self.group_out(out)

    async def list_groups(self) -> list[dict[str, Any]]:
        rows = await self.db.fetchall("SELECT * FROM groups ORDER BY id")
        return [await self.group_out(r) for r in rows]

    async def get_group(self, group_id: str) -> dict[str, Any]:
        row = await self.db.fetchone("SELECT * FROM groups WHERE id=?", (group_id,))
        if row is None:
            raise InvalidParam(f"group not found: {group_id}")
        return await self.group_out(row)

    async def update_group(self, group_id: str, patch: dict[str, Any]) -> dict[str, Any]:
        await self.get_group(group_id)
        fields = {k: v for k, v in patch.items() if v is not None}
        if fields:
            cols = ", ".join(f"{k}=?" for k in fields)
            await self.db.execute(f"UPDATE groups SET {cols} WHERE id=?", (*fields.values(), group_id))
        return await self.get_group(group_id)

    async def delete_group(self, group_id: str) -> None:
        await self.get_group(group_id)
        await self.db.execute("DELETE FROM groups WHERE id=?", (group_id,))

    async def list_members(self, group_id: str) -> list[dict[str, Any]]:
        rows = await self.db.fetchall(
            "SELECT gm.*, u.nickname, u.avatar, u.actor_type, u.behavior FROM group_members gm "
            "JOIN users u ON u.id=gm.user_id WHERE gm.group_id=? ORDER BY gm.joined_at, gm.user_id",
            (group_id,),
        )
        out = []
        for r in rows:
            data = dict(r)
            data["avatar"] = data.get("avatar") or self._avatar(data["user_id"])
            out.append(data)
        return out

    async def add_member(self, group_id: str, user_id: str, role: str = "member") -> dict[str, Any]:
        await self.get_group(group_id)
        await self.get_user(user_id)
        async with self.db.tx() as conn:
            if role == "owner":
                await conn.execute("UPDATE group_members SET role='admin' WHERE group_id=? AND role='owner'", (group_id,))
            await conn.execute(
                "INSERT INTO group_members(group_id,user_id,role,joined_at) VALUES(?,?,?,?) "
                "ON CONFLICT(group_id,user_id) DO UPDATE SET role=excluded.role",
                (group_id, user_id, role, self.clock.now_ms()),
            )
        return (await self.list_members(group_id))[-1]

    async def update_member(self, group_id: str, user_id: str, patch: dict[str, Any]) -> dict[str, Any]:
        fields: dict[str, Any] = {}
        if patch.get("role"):
            fields["role"] = patch["role"]
        if patch.get("mute_seconds") is not None:
            fields["mute_until"] = self.clock.now_ms() + int(patch["mute_seconds"]) * 1000
        if fields:
            cols = ", ".join(f"{k}=?" for k in fields)
            await self.db.execute(
                f"UPDATE group_members SET {cols} WHERE group_id=? AND user_id=?",
                (*fields.values(), group_id, user_id),
            )
        members = await self.list_members(group_id)
        for m in members:
            if m["user_id"] == user_id:
                return m
        raise InvalidParam(f"member not found: {user_id}")

    async def remove_member(self, group_id: str, user_id: str) -> None:
        await self.db.execute("DELETE FROM group_members WHERE group_id=? AND user_id=?", (group_id, user_id))

    async def add_bot_to_group(self, group_id: str, bot_id: str) -> dict[str, Any]:
        await self.get_group(group_id)
        await self.bot_by_id(bot_id)
        await self.db.execute(
            "INSERT OR IGNORE INTO group_bots(group_id,bot_id,joined_at) VALUES(?,?,?)",
            (group_id, bot_id, self.clock.now_ms()),
        )
        await self.openid(bot_id, "group", group_id)
        return await self.get_group(group_id)

    async def remove_bot_from_group(self, group_id: str, bot_id: str) -> None:
        await self.db.execute("DELETE FROM group_bots WHERE group_id=? AND bot_id=?", (group_id, bot_id))

    # ------------------------------------------------------------------ conversations
    async def list_conversations(self) -> list[dict[str, Any]]:
        groups = await self.db.fetchall("SELECT * FROM groups ORDER BY id")
        out: list[dict[str, Any]] = []
        for g in groups:
            members = await self.db.fetchone("SELECT COUNT(*) AS c FROM group_members WHERE group_id=?", (g["id"],))
            member_rows = await self.db.fetchall("SELECT user_id FROM group_members WHERE group_id=?", (g["id"],))
            bots = await self.db.fetchall(
                "SELECT b.id, b.name FROM group_bots gb JOIN bots b ON b.id=gb.bot_id WHERE gb.group_id=?", (g["id"],)
            )
            last = await self._last_message(f"group:{g['id']}")
            out.append(
                {
                    "id": f"group:{g['id']}",
                    "type": "group",
                    "peer_id": g["id"],
                    "name": g["name"],
                    "avatar": g["avatar"] or self._avatar(g["id"]),
                    "member_count": int(members["c"]) if members else 0,
                    "member_ids": [m["user_id"] for m in member_rows],
                    "bot_ids": [b["id"] for b in bots],
                    "last_message": last,
                }
            )
        dms = await self.db.fetchall("SELECT * FROM conversations WHERE type='dm' ORDER BY id")
        for d in dms:
            user = await self.db.fetchone("SELECT * FROM users WHERE id=?", (d["peer_id"],))
            bot = await self.db.fetchone("SELECT * FROM bots WHERE id=?", (d["bot_id"],))
            if not user or not bot:
                continue
            out.append(
                {
                    "id": d["id"],
                    "type": "dm",
                    "peer_id": d["peer_id"],
                    "name": f"{user['nickname']} ↔ {bot['name']}",
                    "avatar": user["avatar"] or self._avatar(user["id"]),
                    "bot_id": d["bot_id"],
                    "user_id": user["id"],
                    "last_message": await self._last_message(d["id"]),
                }
            )
        out.sort(key=lambda c: (c.get("last_message") or {}).get("sent_at", 0), reverse=True)
        return out

    async def _last_message(self, conversation_id: str) -> dict[str, Any] | None:
        row = await self.db.fetchone(
            "SELECT * FROM messages WHERE conversation_id=? ORDER BY seq DESC LIMIT 1", (conversation_id,)
        )
        if row is None:
            return None
        data = dict(row)
        data["raw"] = json.loads(data.pop("raw_json") or "{}")
        return data

    async def ensure_conversation(self, conversation_id: str) -> None:
        kind, _, peer = conversation_id.partition(":")
        if kind == "group":
            await self.db.execute(
                "INSERT INTO conversations(id,type,peer_id,bot_id,last_seq) VALUES(?,?,?,NULL,0) "
                "ON CONFLICT(id) DO NOTHING",
                (conversation_id, "group", peer),
            )
        elif kind == "dm":
            bot_id, _, user_id = peer.partition(":")
            await self.db.execute(
                "INSERT INTO conversations(id,type,peer_id,bot_id,last_seq) VALUES(?,?,?,?,0) "
                "ON CONFLICT(id) DO NOTHING",
                (conversation_id, "dm", user_id, bot_id),
            )

    async def resolve_conversation(self, *, to: str | None = None, conversation_id: str | None = None) -> str:
        candidate = conversation_id or to
        if not candidate:
            raise InvalidParam("conversation_id or to is required")
        if candidate.startswith("group:") or candidate.startswith("dm:"):
            row = await self.db.fetchone("SELECT id FROM conversations WHERE id=?", (candidate,))
            if row is None and candidate.startswith("group:"):
                gid = candidate.split(":", 1)[1]
                if await self.db.fetchone("SELECT id FROM groups WHERE id=?", (gid,)):
                    await self.ensure_conversation(candidate)
                    row = {"id": candidate}
            if row is None and candidate.startswith("dm:"):
                bot_id, _, user_id = candidate[3:].partition(":")
                if await self.bot_by_app_id(bot_id) and await self.db.fetchone(
                    "SELECT id FROM users WHERE id=?", (user_id,)
                ):
                    await self.ensure_conversation(candidate)
                    row = {"id": candidate}
            if row is None:
                raise InvalidParam(f"conversation not found: {candidate}")
            return candidate
        # bare id: group or "<bot>:<user>"
        if await self.db.fetchone("SELECT id FROM groups WHERE id=?", (candidate,)):
            conv = f"group:{candidate}"
            await self.ensure_conversation(conv)
            return conv
        if ":" in candidate:
            bot_id, _, user_id = candidate.partition(":")
            if await self.bot_by_app_id(bot_id) and await self.db.fetchone("SELECT id FROM users WHERE id=?", (user_id,)):
                conv = f"dm:{bot_id}:{user_id}"
                await self.ensure_conversation(conv)
                return conv
        raise InvalidParam(f"cannot resolve conversation: {candidate}")

    # ------------------------------------------------------------------ messages
    async def list_messages(self, conversation_id: str, *, limit: int = 50, before_seq: int | None = None) -> list[dict[str, Any]]:
        sql = "SELECT * FROM messages WHERE conversation_id=?"
        params: list[Any] = [conversation_id]
        if before_seq is not None:
            sql += " AND seq < ?"
            params.append(before_seq)
        sql += " ORDER BY seq DESC LIMIT ?"
        params.append(max(1, min(limit, 200)))
        rows = await self.db.fetchall(sql, params)
        return [await self.message_out(r) for r in reversed(rows)]

    async def message_out(self, row: aiosqlite.Row | dict) -> dict[str, Any]:
        data = dict(row)
        raw = json.loads(data.pop("raw_json") or "{}")
        sender = None
        try:
            if data["sender_type"] == "user":
                r = await self.db.fetchone("SELECT id,nickname,avatar FROM users WHERE id=?", (data["sender_id"],))
                if r:
                    sender = {"id": r["id"], "name": r["nickname"], "avatar": r["avatar"] or self._avatar(r["id"])}
            else:
                r = await self.db.fetchone("SELECT id,name,avatar FROM bots WHERE id=?", (data["sender_id"],))
                if r:
                    sender = {"id": r["id"], "name": r["name"], "avatar": r["avatar"] or self._avatar(f"bot_{r['id']}")}
        except (KeyError, TypeError):
            sender = None
        data["sender"] = sender
        data["raw"] = raw
        data["content"] = data.get("content") or ""
        return data

    async def send_as_user(
        self,
        conversation_id: str,
        user_id: str,
        content: str,
        *,
        msg_type: int = 0,
        mentions: Sequence[str] | None = None,
        at_bots: Sequence[str] | None = None,
        reply_to: str | None = None,
    ) -> dict[str, Any]:
        await self.ensure_conversation(conversation_id)
        conv = await self.db.fetchone("SELECT * FROM conversations WHERE id=?", (conversation_id,))
        if conv is None:
            raise InvalidParam(f"conversation not found: {conversation_id}")
        user = await self.db.fetchone("SELECT * FROM users WHERE id=?", (user_id,))
        if user is None:
            raise InvalidParam(f"user not found: {user_id}")

        if conv["type"] == "group":
            member = await self.db.fetchone(
                "SELECT * FROM group_members WHERE group_id=? AND user_id=?", (conv["peer_id"], user_id)
            )
            if member is None:
                raise NoPermission("user is not a member of this group")
            if int(member["mute_until"] or 0) > self.clock.now_ms():
                raise NoPermission("user is muted in this group")
            bot_rows = await self.db.fetchall(
                "SELECT b.* FROM group_bots gb JOIN bots b ON b.id=gb.bot_id WHERE gb.group_id=?", (conv["peer_id"],)
            )
            mention_ids = list(dict.fromkeys(list(mentions or []) + list(at_bots or [])))
            if at_bots is not None:
                target_bot_ids = [b for b in at_bots if any(r["id"] == b for r in bot_rows)]
            elif mentions:
                target_bot_ids = [b for b in mention_ids if any(r["id"] == b for r in bot_rows)]
            elif self.setting("chat.auto_at_bot", True):
                target_bot_ids = [r["id"] for r in bot_rows]
            else:
                target_bot_ids = []
            mentioned_users: list[str] = []
            for mid in mention_ids:
                if any(r["id"] == mid for r in bot_rows):
                    continue
                row = await self.db.fetchone(
                    "SELECT user_id FROM group_members WHERE group_id=? AND user_id=?", (conv["peer_id"], mid)
                )
                if row:
                    mentioned_users.append(mid)
        else:
            if conv["peer_id"] != user_id:
                raise NoPermission("this DM belongs to another user")
            bot_rows = await self.db.fetchall("SELECT * FROM bots WHERE id=?", (conv["bot_id"],))
            target_bot_ids = [r["id"] for r in bot_rows]
            mentioned_users = []

        if not bot_rows:
            raise InvalidParam("no bot is present in this conversation")
        reply_ref = None
        if reply_to:
            ref = await self.db.fetchone("SELECT * FROM messages WHERE msg_id=? AND conversation_id=?", (reply_to, conversation_id))
            if ref is None:
                raise MessageNotFound("reply_to message not found in this conversation")
            reply_ref = reply_to

        event_type = "GROUP_AT_MESSAGE_CREATE" if conv["type"] == "group" else "C2C_MESSAGE_CREATE"
        event_id = new_event_id()
        deliveries: list[tuple[str, int, dict[str, Any], dict[str, Any]]] = []
        async with self.db.tx() as conn:
            cur = await conn.execute("UPDATE conversations SET last_seq=last_seq+1 WHERE id=?", (conversation_id,))
            await cur.close()
            cur = await conn.execute("SELECT last_seq FROM conversations WHERE id=?", (conversation_id,))
            seq_row = await cur.fetchone()
            await cur.close()
            seq = int(seq_row["last_seq"])
            msg_id = new_msg_id()
            sent_at = self.clock.now_ms()
            raw_message = {
                "id": msg_id,
                "content": content,
                "group_id": conv["peer_id"] if conv["type"] == "group" else None,
                "timestamp": self.clock.iso(sent_at),
                "msg_seq": seq,
            }
            await conn.execute(
                "INSERT INTO messages(conversation_id,seq,msg_id,event_id,direction,sender_id,sender_type,bot_id,"
                "msg_type,content,raw_json,sent_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                (conversation_id, seq, msg_id, event_id, "in", user_id, "user", None, msg_type, content,
                 json.dumps(raw_message, ensure_ascii=False), sent_at),
            )
            for bot in bot_rows:
                if bot["id"] not in target_bot_ids:
                    continue
                group_openid = None
                if conv["type"] == "group":
                    group_openid = await self._openid_tx(conn, bot, "group", conv["peer_id"])
                user_openid = await self._openid_tx(conn, bot, "user", user_id)
                union = union_openid_for(user_id)
                payload: dict[str, Any] = {
                    "id": msg_id,
                    "content": content,
                    "author": {
                        "id": user_openid,
                        "member_openid": user_openid if conv["type"] == "group" else None,
                        "user_openid": user_openid if conv["type"] == "dm" else None,
                        "union_openid": union,
                    },
                    "timestamp": self.clock.iso(sent_at),
                    "msg_seq": seq,
                    "mentions": [],
                    "attachments": [],
                }
                if conv["type"] == "group":
                    payload["group_openid"] = group_openid
                    payload["group_id"] = conv["peer_id"]
                    if bot["id"] in target_bot_ids:
                        bot_openid = openid_for(bot["app_id"], "bot", bot["id"])
                        payload["content"] = f"<@!{bot_openid}> {content}" if content else f"<@!{bot_openid}>"
                else:
                    payload["author"]["user_openid"] = user_openid
                for mid in mentioned_users:
                    mrow = await self._openid_tx(conn, bot, "user", mid)
                    payload["mentions"].append({"id": mrow, "member_openid": mrow})
                if reply_ref:
                    payload["message_reference"] = {"message_id": reply_ref}
                cur = await conn.execute(
                    "INSERT INTO events(event_id,type,payload_json,conversation_id,target_bot_id,created_at)"
                    " VALUES(?,?,?,?,?,?)",
                    (event_id, event_type, json.dumps(payload, ensure_ascii=False), conversation_id, bot["id"], sent_at),
                )
                deliveries.append((bot["id"], int(cur.lastrowid or 0), payload, dict(bot)))
                await cur.close()

        message = await self.db.fetchone("SELECT * FROM messages WHERE msg_id=?", (msg_id,))
        out = await self.message_out(message)  # type: ignore[arg-type]
        for bot_id, seq_no, payload, bot in deliveries:
            await self._deliver(bot_id, seq_no, event_type, event_id, payload, bot, conversation_id)
        await self.hub.publish({"kind": "message", "conversation_id": conversation_id, "message": out})
        await self._maybe_run_rules(event_type, payload=deliveries[0][2] if deliveries else {}, conversation_id=conversation_id)
        return {"message": out, "event_ids": [event_id], "delivered_to": [d[0] for d in deliveries]}

    async def _deliver(
        self,
        bot_id: str,
        seq: int,
        event_type: str,
        event_id: str,
        payload: dict[str, Any],
        bot: dict[str, Any],
        conversation_id: str,
    ) -> None:
        frame = {"op": 0, "s": seq, "t": event_type, "id": event_id, "d": payload}
        if self.gateway is not None:
            await self.gateway.dispatch(bot_id, frame, conversation_id)
        if bot.get("delivery") == "webhook" or await self.webhooks.endpoints(bot_id):
            await self.webhooks.deliver(bot_id, frame)
        await self.hub.publish(
            {
                "kind": "event",
                "event": {
                    "seq": seq,
                    "event_id": event_id,
                    "type": event_type,
                    "bot_id": bot_id,
                    "conversation_id": conversation_id,
                    "payload": payload,
                    "created_at": self.clock.now_ms(),
                },
            }
        )

    async def send_as_bot(
        self,
        bot_id: str,
        *,
        target_kind: str,
        target_openid: str,
        body: dict[str, Any],
        source: str = "rest",
    ) -> dict[str, Any]:
        bot = await self.bot_by_id(bot_id)
        scopes = json.loads(bot["scopes"] or "[]")
        if target_kind == "group" and BOT_SCOPE_GROUP not in scopes:
            raise NoPermission("bot lacks the group_message scope")
        if target_kind == "user" and BOT_SCOPE_C2C not in scopes:
            raise NoPermission("bot lacks the c2c_message scope")
        if body.get("media") and BOT_SCOPE_MEDIA not in scopes:
            raise NoPermission("bot lacks the media scope")

        mapping = await self.resolve_openid(bot_id, target_openid)
        if mapping["kind"] != target_kind:
            raise InvalidParam("openid kind mismatch")
        entity_id = mapping["entity_id"]
        if target_kind == "group":
            if not await self.db.fetchone("SELECT 1 FROM group_bots WHERE group_id=? AND bot_id=?", (entity_id, bot_id)):
                raise NoPermission("bot is not a member of this group")
            conversation_id = f"group:{entity_id}"
        else:
            conversation_id = f"dm:{bot_id}:{entity_id}"
        await self.ensure_conversation(conversation_id)

        if self.setting("require_online", True) and bot["delivery"] == "ws":
            if self.gateway is None or not self.gateway.is_online(bot_id):
                raise BotOffline("connect the bot to the gateway before sending messages")

        now = self.clock.now_ms()
        msg_id_in = body.get("msg_id")
        if msg_id_in:
            ref = await self.db.fetchone(
                "SELECT * FROM messages WHERE msg_id=? AND conversation_id=?", (msg_id_in, conversation_id)
            )
            if ref is None or ref["direction"] != "in":
                raise MessageNotFound("msg_id does not refer to an inbound message of this conversation")
            window = int(self.setting("passive_window_ms", 5 * 60 * 1000))
            if now - int(ref["sent_at"]) > window:
                raise PassiveWindowExpired("passive reply window (5 minutes) has expired")
            limit = int(self.setting("passive_reply_limit", 5))
            wr = await self.db.fetchone(
                "SELECT * FROM reply_windows WHERE msg_id=? AND bot_id=?", (msg_id_in, bot_id)
            )
            used = int(wr["count"]) if wr else 0
            if used >= limit:
                raise PassiveWindowExpired(f"passive reply limit ({limit}) exhausted for this msg_id")
            msg_seq = body.get("msg_seq")
            if msg_seq is None:
                cur = await self.db.fetchone(
                    "SELECT MAX(msg_seq) AS m FROM reply_keys WHERE bot_id=? AND msg_id=?", (bot_id, msg_id_in)
                )
                msg_seq = int(cur["m"] or 0) + 1 if cur else 1
            else:
                msg_seq = int(msg_seq)
                dup = await self.db.fetchone(
                    "SELECT 1 FROM reply_keys WHERE bot_id=? AND msg_id=? AND msg_seq=?", (bot_id, msg_id_in, msg_seq)
                )
                if dup:
                    raise DuplicateReply(f"msg_seq {msg_seq} already used for msg_id {msg_id_in}")
            await self.db.execute(
                "INSERT INTO reply_keys(bot_id,msg_id,msg_seq,created_at) VALUES(?,?,?,?)",
                (bot_id, msg_id_in, msg_seq, now),
            )
            await self.db.execute(
                "INSERT INTO reply_windows(msg_id,bot_id,count,expires_at) VALUES(?,?,?,?) "
                "ON CONFLICT(msg_id,bot_id) DO UPDATE SET count=count+1, expires_at=excluded.expires_at",
                (msg_id_in, bot_id, 1, int(ref["sent_at"]) + window),
            )
        else:
            msg_seq = int(body.get("msg_seq") or 1)
            quota = int(self.setting("active_quota_per_day", 100))
            day = self.clock.now().strftime("%Y-%m-%d")
            row = await self.db.fetchone(
                "SELECT used FROM active_quota WHERE bot_id=? AND conversation_id=? AND day=?",
                (bot_id, conversation_id, day),
            )
            used = int(row["used"]) if row else 0
            if used >= quota:
                raise QuotaExhausted(f"active message quota ({quota}/day) exhausted")
            await self.db.execute(
                "INSERT INTO active_quota(bot_id,conversation_id,day,used) VALUES(?,?,?,1) "
                "ON CONFLICT(bot_id,conversation_id,day) DO UPDATE SET used=used+1",
                (bot_id, conversation_id, day),
            )

        msg_type = int(body.get("msg_type") or 0)
        content = body.get("content") or ""
        if not content and not any(body.get(k) for k in ("markdown", "keyboard", "ark", "embed", "media")):
            raise InvalidParam("message body is empty")

        msg_id = new_msg_id("MQS_MSG")
        sent_at = self.clock.now_ms()
        raw = dict(body)
        raw["id"] = msg_id
        raw["timestamp"] = self.clock.iso(sent_at)
        raw["msg_seq"] = msg_seq
        attachments = []
        if body.get("media"):
            file_info = (body.get("media") or {}).get("file_info")
            if file_info:
                mhash = str(file_info).removeprefix("mqs-file-")
                media_row = await self.db.fetchone("SELECT * FROM media WHERE hash=?", (mhash,))
                if media_row:
                    attachments.append(
                        {
                            "content_type": media_row["mime"],
                            "filename": mhash[:12],
                            "size": media_row["size"],
                            "url": f"/media/{mhash}",
                            "id": mhash,
                        }
                    )
        async with self.db.tx() as conn:
            cur = await conn.execute("UPDATE conversations SET last_seq=last_seq+1 WHERE id=?", (conversation_id,))
            await cur.close()
            cur = await conn.execute("SELECT last_seq FROM conversations WHERE id=?", (conversation_id,))
            seq_row = await cur.fetchone()
            await cur.close()
            seq = int(seq_row["last_seq"])
            await conn.execute(
                "INSERT INTO messages(conversation_id,seq,msg_id,event_id,direction,sender_id,sender_type,bot_id,"
                "msg_type,content,raw_json,sent_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                (conversation_id, seq, msg_id, None, "out", bot_id, "bot", bot_id, msg_type, content,
                 json.dumps(raw, ensure_ascii=False), sent_at),
            )
        message = await self.db.fetchone("SELECT * FROM messages WHERE msg_id=?", (msg_id,))
        out = await self.message_out(message)  # type: ignore[arg-type]
        out["attachments"] = attachments
        await self.hub.publish({"kind": "message", "conversation_id": conversation_id, "message": out})
        await self.log_frame(
            "out",
            "http",
            f"POST message -> {target_kind}:{target_openid[:12]} [{msg_id[:18]}]",
            {"request": raw, "response": {"id": msg_id, "timestamp": raw["timestamp"]}},
            bot_id=bot_id,
        )
        return {"id": msg_id, "timestamp": raw["timestamp"], "seq": seq, "conversation_id": conversation_id}

    async def recall_message(self, bot_id: str, target_kind: str, target_openid: str, message_id: str) -> None:
        mapping = await self.resolve_openid(bot_id, target_openid)
        if mapping["kind"] != target_kind:
            raise InvalidParam("openid kind mismatch")
        conversation_id = (
            f"group:{mapping['entity_id']}" if target_kind == "group" else f"dm:{bot_id}:{mapping['entity_id']}"
        )
        row = await self.db.fetchone(
            "SELECT * FROM messages WHERE msg_id=? AND conversation_id=?", (message_id, conversation_id)
        )
        if row is None or row["sender_id"] != bot_id:
            raise MessageNotFound("message not found or not owned by this bot")
        await self.db.execute("UPDATE messages SET recalled_at=? WHERE msg_id=?", (self.clock.now_ms(), message_id))
        await self.hub.publish(
            {"kind": "recall", "conversation_id": conversation_id, "message_id": message_id, "bot_id": bot_id}
        )

    async def list_events(self, *, since_seq: int = 0, limit: int = 100, conversation_id: str | None = None) -> list[dict]:
        sql = "SELECT * FROM events WHERE seq > ?"
        params: list[Any] = [since_seq]
        if conversation_id:
            sql += " AND conversation_id=?"
            params.append(conversation_id)
        sql += " ORDER BY seq LIMIT ?"
        params.append(max(1, min(limit, 500)))
        rows = await self.db.fetchall(sql, params)
        out = []
        for r in rows:
            data = dict(r)
            data["payload"] = json.loads(data.pop("payload_json") or "{}")
            out.append(data)
        return out

    # ------------------------------------------------------------------ rules
    async def list_rules(self) -> list[dict[str, Any]]:
        rows = await self.db.fetchall("SELECT * FROM rules ORDER BY created_at")
        out = []
        for r in rows:
            data = dict(r)
            data["condition"] = json.loads(data.pop("condition_json") or "{}")
            data["action"] = json.loads(data.pop("action_json") or "{}")
            data["enabled"] = bool(data["enabled"])
            out.append(data)
        return out

    async def create_rule(self, data: dict[str, Any]) -> dict[str, Any]:
        rid = f"rule_{new_session_id()[:12]}"
        await self.db.execute(
            "INSERT INTO rules(id,name,event_type,condition_json,action_json,delay_ms,enabled,created_at)"
            " VALUES(?,?,?,?,?,?,?,?)",
            (
                rid,
                data.get("name") or "rule",
                data.get("event_type") or "group_at_message_create",
                json.dumps(data.get("condition") or {}, ensure_ascii=False),
                json.dumps(data.get("action") or {}, ensure_ascii=False),
                int(data.get("delay_ms") or 0),
                1 if data.get("enabled", True) else 0,
                self.clock.now_ms(),
            ),
        )
        rules = await self.list_rules()
        return next(r for r in rules if r["id"] == rid)

    async def delete_rule(self, rule_id: str) -> None:
        await self.db.execute("DELETE FROM rules WHERE id=?", (rule_id,))

    async def _maybe_run_rules(self, event_type: str, *, payload: dict[str, Any], conversation_id: str) -> None:
        rules = await self.list_rules()
        for rule in rules:
            if not rule["enabled"]:
                continue
            if rule["event_type"].lower() not in (event_type.lower(), "message"):
                continue
            cond = rule["condition"] or {}
            if cond.get("conversation_id") and cond["conversation_id"] != conversation_id:
                continue
            if cond.get("content_contains") and cond["content_contains"] not in (payload.get("content") or ""):
                continue
            delay = int(rule["delay_ms"] or 0)
            task = asyncio.create_task(self._run_rule(rule, conversation_id, delay, payload))
            self._rule_tasks.add(task)
            task.add_done_callback(self._rule_tasks.discard)

    async def _run_rule(self, rule: dict[str, Any], conversation_id: str, delay: int, payload: dict[str, Any]) -> None:
        if delay:
            await asyncio.sleep(min(delay, 60_000) / 1000)
        action = rule["action"] or {}
        if action.get("type", "send_as_user") != "send_as_user":
            return
        user_id = action.get("user_id")
        text = (action.get("text") or "").replace("{content}", payload.get("content") or "")
        if not user_id or not text:
            return
        try:
            await self.send_as_user(
                conversation_id,
                user_id,
                text,
                at_bots=action.get("at_bots"),
            )
        except Exception:  # noqa: BLE001 - rules must never crash the world
            return

    # ------------------------------------------------------------------ media
    async def store_media(self, data: bytes, mime: str) -> dict[str, Any]:
        from ..ids import media_hash

        mhash = media_hash(data)
        path = self.paths.media / mhash
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.exists():
            path.write_bytes(data)
        await self.db.execute(
            "INSERT INTO media(hash,mime,size,path,created_at) VALUES(?,?,?,?,?) "
            "ON CONFLICT(hash) DO NOTHING",
            (mhash, mime, len(data), str(path), self.clock.now_ms()),
        )
        row = await self.db.fetchone("SELECT * FROM media WHERE hash=?", (mhash,))
        return dict(row)  # type: ignore[arg-type]

    async def media_by_hash(self, mhash: str) -> dict[str, Any] | None:
        row = await self.db.fetchone("SELECT * FROM media WHERE hash=?", (mhash,))
        return dict(row) if row else None

    # ------------------------------------------------------------------ debug
    async def log_frame(
        self,
        direction: str,
        kind: str,
        summary: str,
        data: dict[str, Any],
        *,
        bot_id: str | None = None,
        session_id: str | None = None,
    ) -> None:
        await self.db.execute(
            "INSERT INTO frames(ts,direction,kind,bot_id,session_id,summary,data_json) VALUES(?,?,?,?,?,?,?)",
            (self.clock.now_ms(), direction, kind, bot_id, session_id, summary[:300],
             json.dumps(data, ensure_ascii=False)[:20000]),
        )
        self._frame_writes += 1
        if self._frame_writes % 50 == 0:
            await self.db.prune_frames()
        await self.hub.publish(
            {
                "kind": "frame",
                "frame": {
                    "ts": self.clock.now_ms(),
                    "direction": direction,
                    "kind": kind,
                    "bot_id": bot_id,
                    "session_id": session_id,
                    "summary": summary[:300],
                    "data": data,
                },
            }
        )

    async def list_frames(self, *, since_id: int = 0, limit: int = 200, bot_id: str | None = None,
                          kind: str | None = None, q: str | None = None) -> list[dict[str, Any]]:
        sql = "SELECT * FROM frames WHERE id > ?"
        params: list[Any] = [since_id]
        if bot_id:
            sql += " AND bot_id=?"
            params.append(bot_id)
        if kind:
            sql += " AND kind=?"
            params.append(kind)
        sql += " ORDER BY id LIMIT ?"
        params.append(max(1, min(limit, 1000)))
        rows = await self.db.fetchall(sql, params)
        out = []
        for r in rows:
            data = dict(r)
            data["data"] = json.loads(data.pop("data_json") or "{}")
            out.append(data)
        if q:
            out = [f for f in out if q.lower() in json.dumps(f["data"], ensure_ascii=False).lower()]
        return out

    # ------------------------------------------------------------------ status
    async def counts(self) -> dict[str, int]:
        tables = ["users", "bots", "groups", "messages", "events", "gateway_sessions", "frames", "media", "rules"]
        out: dict[str, int] = {}
        for table in tables:
            row = await self.db.fetchone(f"SELECT COUNT(*) AS c FROM {table}")
            out[table] = int(row["c"]) if row else 0
        return out

    async def status(self) -> dict[str, Any]:
        counts = await self.counts()
        from ..contract import summary as contract_summary

        row = await self.db.fetchone("SELECT COALESCE(MAX(seq),0) AS m FROM events")
        max_event_seq = int(row["m"]) if row else 0
        row = await self.db.fetchone("SELECT COALESCE(MAX(id),0) AS m FROM messages")
        max_message_id = int(row["m"]) if row else 0
        row = await self.db.fetchone("SELECT COALESCE(MAX(id),0) AS m FROM frames")
        max_frame_id = int(row["m"]) if row else 0

        return {
            "profile": self.profile,
            "boot_id": self.boot_id,
            "started_at": self.started_at,
            "uptime_ms": max(0, int(time.time() * 1000) - self.started_at),
            "clock": self.clock.snapshot(),
            "counts": counts,
            "max_event_seq": max_event_seq,
            "max_message_id": max_message_id,
            "max_frame_id": max_frame_id,
            "rate": self.rate.stats(),
            "gateway": self.gateway.stats() if self.gateway else {"connections": 0, "sessions": 0},
            "db_size": self.db.size_bytes(),
            "settings": self.settings,
            "coverage": contract_summary(),
            "hub_subscribers": self.hub.subscriber_count,
        }

    # ------------------------------------------------------------------ utility
    @staticmethod
    async def _fetchone_conn(conn: aiosqlite.Connection, sql: str, params: Sequence[Any]) -> aiosqlite.Row:
        cur = await conn.execute(sql, params)
        row = await cur.fetchone()
        await cur.close()
        return row  # type: ignore[return-value]

    async def checkpoint_for_snapshot(self, target: str) -> None:
        import os

        if os.path.exists(target):
            os.unlink(target)
        await self.db.execute("VACUUM INTO ?", (target,))

    def new_trace_id(self) -> str:
        from ..errors import new_trace_id

        return new_trace_id()

    @staticmethod
    def iso(ms: int) -> str:
        return datetime.fromtimestamp(ms / 1000).astimezone().isoformat(timespec="seconds")
