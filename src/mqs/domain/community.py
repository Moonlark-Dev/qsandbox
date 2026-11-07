"""Community surface: group management, guild writes/roles, channel DMs and friends.

These methods sit on top of :class:`mqs.domain.world.World` via a mixin so the
core message/gateway logic stays in one file.
"""

from __future__ import annotations

import json
from collections.abc import Sequence
from datetime import UTC, datetime
from typing import Any

from ..errors import InvalidParam, MessageNotFound, NoPermission, QuotaExhausted
from ..ids import new_event_id, new_msg_id, openid_for, short_token, union_openid_for


def _parse_time(value: str | None) -> int:
    if not value:
        return 0
    try:
        parsed = datetime.fromisoformat(str(value).replace("Z", "+00:00"))
    except ValueError as exc:
        raise InvalidParam(f"invalid RFC3339 timestamp: {value}") from exc
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=UTC)
    return int(parsed.timestamp() * 1000)


class CommunityMixin:
    # ------------------------------------------------------------------ group management
    async def _group_bot_row(self, group_id: str, bot_id: str) -> Any:
        row = await self.db.fetchone(
            "SELECT gb.*, b.app_id AS bot_app_id, b.name AS bot_name FROM group_bots gb "
            "JOIN bots b ON b.id=gb.bot_id WHERE gb.group_id=? AND gb.bot_id=?",
            (group_id, bot_id),
        )
        if row is None:
            raise NoPermission("bot is not a member of this group")
        return row

    async def group_info(self, group_id: str, bot_id: str) -> dict[str, Any]:
        group = await self.get_group(group_id)
        await self._group_bot_row(group_id, bot_id)
        count = await self.db.fetchone("SELECT COUNT(*) AS c FROM group_members WHERE group_id=?", (group_id,))
        try:
            tags = json.loads(group.get("tags") or "[]")
        except ValueError:
            tags = []
        return {
            "group_openid": await self.openid(bot_id, "group", group_id),
            "group_name": group["name"],
            "group_finger_memo": group.get("finger_memo") or "",
            "group_class_text": group.get("class_text") or "",
            "group_tags": tags,
            "group_member_num": int(count["c"]) if count else 0,
        }

    async def group_bot_state(self, group_id: str, bot_id: str) -> dict[str, Any]:
        row = await self._group_bot_row(group_id, bot_id)
        return {
            "member_openid": openid_for(row["bot_app_id"], "bot", bot_id),
            "joined_at": self.iso(int(row["joined_at"])),
            "allow_proactive_msg": bool(row["allow_proactive_msg"]),
            "recv_msg_setting": row["recv_msg_setting"] or "all",
            "member_role": "member",
        }

    async def update_group_bot_settings(self, group_id: str, bot_id: str, patch: dict[str, Any]) -> dict[str, Any]:
        await self._group_bot_row(group_id, bot_id)
        fields: dict[str, Any] = {}
        if patch.get("allow_proactive_msg") is not None:
            fields["allow_proactive_msg"] = 1 if patch["allow_proactive_msg"] else 0
        if patch.get("recv_msg_setting"):
            value = str(patch["recv_msg_setting"])
            if value not in ("all", "only_mention", "mention_and_context"):
                raise InvalidParam("recv_msg_setting must be all/only_mention/mention_and_context")
            fields["recv_msg_setting"] = value
        if fields:
            cols = ", ".join(f"{k}=?" for k in fields)
            await self.db.execute(
                f"UPDATE group_bots SET {cols} WHERE group_id=? AND bot_id=?",
                (*fields.values(), group_id, bot_id),
            )
        return await self.group_bot_state(group_id, bot_id)

    async def group_mute_settings(self, group_id: str, bot_id: str) -> dict[str, Any]:
        await self._group_bot_row(group_id, bot_id)
        rows = await self.db.fetchall(
            "SELECT gm.user_id, gm.mute_until, u.nickname FROM group_members gm "
            "JOIN users u ON u.id=gm.user_id WHERE gm.group_id=? AND gm.mute_until>? "
            "ORDER BY gm.mute_until",
            (group_id, self.clock.now_ms()),
        )
        members = []
        for row in rows:
            members.append(
                {
                    "member_openid": await self.openid(bot_id, "user", row["user_id"]),
                    "mute_expire_at": self.iso(int(row["mute_until"])),
                    "username": row["nickname"],
                    "union_openid": union_openid_for(row["user_id"]),
                }
            )
        return {
            "global_rule": {"mode": "none", "schedule_rules": [], "recurring_rules": []},
            "members": members,
        }

    async def set_group_mute(self, group_id: str, bot_id: str, members: Sequence[dict[str, Any]]) -> dict[str, Any]:
        await self._group_bot_row(group_id, bot_id)
        if not members:
            return {}
        if len(members) > 20:
            raise InvalidParam("at most 20 members per request")
        now = self.clock.now_ms()
        max_expire = now + 30 * 24 * 60 * 60 * 1000
        for item in members:
            op = str(item.get("op") or "")
            if op not in ("add", "update", "del"):
                raise InvalidParam("op must be add/update/del")
            member_openid = str(item.get("member_openid") or "")
            mapping = await self.resolve_openid(bot_id, member_openid)
            if mapping["kind"] != "user":
                raise InvalidParam("member_openid does not refer to a user")
            user_id = mapping["entity_id"]
            member = await self.db.fetchone(
                "SELECT role FROM group_members WHERE group_id=? AND user_id=?", (group_id, user_id)
            )
            if member is None:
                raise InvalidParam("member is not in this group")
            if member["role"] in ("owner", "admin"):
                raise NoPermission("cannot mute the group owner or an admin")
            if op == "del":
                expire = 0
            else:
                expire = _parse_time(item.get("mute_expire_at"))
                if not expire:
                    raise InvalidParam("mute_expire_at is required for add/update")
                expire = min(expire, max_expire)
            await self.db.execute(
                "UPDATE group_members SET mute_until=? WHERE group_id=? AND user_id=?",
                (expire, group_id, user_id),
            )
        return {}

    async def group_blacklist_page(
        self, group_id: str, bot_id: str, cursor: str = "", limit: int = 20
    ) -> dict[str, Any]:
        await self._group_bot_row(group_id, bot_id)
        try:
            offset = max(0, int(cursor or 0))
        except ValueError:
            offset = 0
        limit = max(1, min(int(limit or 20), 100))
        rows = await self.db.fetchall(
            "SELECT gb.user_id, gb.banned_at, u.nickname FROM group_blacklist gb "
            "JOIN users u ON u.id=gb.user_id WHERE gb.group_id=? ORDER BY gb.banned_at, gb.user_id "
            "LIMIT ? OFFSET ?",
            (group_id, limit + 1, offset),
        )
        page = rows[:limit]
        users = [
            {
                "union_openid": union_openid_for(row["user_id"]),
                "member_openid": await self.openid(bot_id, "user", row["user_id"]),
                "username": row["nickname"],
                "banned_at": self.iso(int(row["banned_at"])),
                "bot": False,
            }
            for row in page
        ]
        return {"users": users, "next_cursor": str(offset + limit) if len(rows) > limit else ""}

    async def update_group_blacklist(
        self, group_id: str, bot_id: str, op: str, member_openids: Sequence[str]
    ) -> dict[str, Any]:
        await self._group_bot_row(group_id, bot_id)
        if op not in ("add", "del"):
            raise InvalidParam("op must be add/del")
        if len(member_openids) > 20:
            raise InvalidParam("at most 20 members per request")
        fail: list[str] = []
        now = self.clock.now_ms()
        for member_openid in member_openids:
            try:
                mapping = await self.resolve_openid(bot_id, member_openid)
            except InvalidParam:
                fail.append(member_openid)
                continue
            if mapping["kind"] != "user":
                fail.append(member_openid)
                continue
            user_id = mapping["entity_id"]
            if op == "add":
                member = await self.db.fetchone(
                    "SELECT 1 FROM group_members WHERE group_id=? AND user_id=?", (group_id, user_id)
                )
                if member is not None:
                    fail.append(member_openid)
                    continue
                await self.db.execute(
                    "INSERT INTO group_blacklist(group_id,user_id,banned_at) VALUES(?,?,?) "
                    "ON CONFLICT(group_id,user_id) DO NOTHING",
                    (group_id, user_id, now),
                )
            else:
                await self.db.execute(
                    "DELETE FROM group_blacklist WHERE group_id=? AND user_id=?", (group_id, user_id)
                )
        return {"fail_openids": fail}

    async def batch_remove_group_members(
        self, group_id: str, bot_id: str, member_openids: Sequence[str], add_blacklist: bool = False
    ) -> dict[str, Any]:
        await self._group_bot_row(group_id, bot_id)
        if len(member_openids) > 20:
            raise InvalidParam("at most 20 members per request")
        failed_blacklist: list[str] = []
        now = self.clock.now_ms()
        for member_openid in member_openids:
            try:
                mapping = await self.resolve_openid(bot_id, member_openid)
            except InvalidParam:
                continue
            if mapping["kind"] != "user":
                continue
            user_id = mapping["entity_id"]
            member = await self.db.fetchone(
                "SELECT role FROM group_members WHERE group_id=? AND user_id=?", (group_id, user_id)
            )
            if member is None or member["role"] in ("owner", "admin"):
                continue
            await self.db.execute(
                "DELETE FROM group_members WHERE group_id=? AND user_id=?", (group_id, user_id)
            )
            if add_blacklist:
                await self.db.execute(
                    "INSERT INTO group_blacklist(group_id,user_id,banned_at) VALUES(?,?,?) "
                    "ON CONFLICT(group_id,user_id) DO NOTHING",
                    (group_id, user_id, now),
                )
            else:
                failed_blacklist.append(member_openid)
        return {
            "remove_members_result": "success",
            "add_to_member_blacklist_fail_openids": failed_blacklist if add_blacklist else [],
        }

    async def _join_request_out(self, row: Any, bot_id: str) -> dict[str, Any]:
        return {
            "join_request_id": row["id"],
            "risk_tips": row["risk_tips"],
            "union_openid": union_openid_for(row["user_id"]),
            "member_openid": await self.openid(bot_id, "user", row["user_id"]),
            "username": row["nickname"],
            "apply_at": self.iso(int(row["apply_at"])),
            "apply_source": row["source"],
            "invited_by": row["invited_by"],
            "bot": False,
            "verify_info": {
                "method": "verify_message",
                "verify_message": row["verify_message"],
                "review_qa_list": [],
            },
        }

    async def join_requests_page(
        self, group_id: str, bot_id: str, cursor: str = "", limit: int = 20
    ) -> dict[str, Any]:
        await self._group_bot_row(group_id, bot_id)
        try:
            offset = max(0, int(cursor or 0))
        except ValueError:
            offset = 0
        limit = max(1, min(int(limit or 20), 50))
        rows = await self.db.fetchall(
            "SELECT jr.*, u.nickname FROM group_join_requests jr JOIN users u ON u.id=jr.user_id "
            "WHERE jr.group_id=? AND jr.status='pending' ORDER BY jr.apply_at, jr.id LIMIT ? OFFSET ?",
            (group_id, limit + 1, offset),
        )
        page = rows[:limit]
        return {
            "list": [await self._join_request_out(row, bot_id) for row in page],
            "next_cursor": str(offset + limit) if len(rows) > limit else "",
        }

    async def create_join_request(
        self, group_id: str, user_id: str, *, verify_message: str = "", source: str = "self_apply"
    ) -> dict[str, Any]:
        await self.get_group(group_id)
        await self.get_user(user_id)
        member = await self.db.fetchone(
            "SELECT 1 FROM group_members WHERE group_id=? AND user_id=?", (group_id, user_id)
        )
        if member is not None:
            raise InvalidParam("user is already a member of this group")
        black = await self.db.fetchone(
            "SELECT 1 FROM group_blacklist WHERE group_id=? AND user_id=?", (group_id, user_id)
        )
        if black is not None:
            raise NoPermission("user is blacklisted in this group")
        existing = await self.db.fetchone(
            "SELECT * FROM group_join_requests WHERE group_id=? AND user_id=? AND status='pending'",
            (group_id, user_id),
        )
        if existing is not None:
            return dict(existing)
        request_id = new_event_id()
        await self.db.execute(
            "INSERT INTO group_join_requests(id,group_id,user_id,apply_at,source,verify_message)"
            " VALUES(?,?,?,?,?,?)",
            (request_id, group_id, user_id, self.clock.now_ms(), source, verify_message),
        )
        return dict(await self.db.fetchone("SELECT * FROM group_join_requests WHERE id=?", (request_id,)))

    async def approve_join_request(
        self,
        group_id: str,
        bot_id: str,
        member_openid: str,
        *,
        op: str,
        join_request_id: str = "",
        reject_reason: str = "",
        add_to_blacklist: bool = False,
    ) -> dict[str, Any]:
        await self._group_bot_row(group_id, bot_id)
        if op not in ("approve", "decline"):
            raise InvalidParam("op must be approve/decline")
        mapping = await self.resolve_openid(bot_id, member_openid)
        if mapping["kind"] != "user":
            raise InvalidParam("member_openid does not refer to a user")
        user_id = mapping["entity_id"]
        if join_request_id:
            row = await self.db.fetchone(
                "SELECT * FROM group_join_requests WHERE id=? AND group_id=? AND status='pending'",
                (join_request_id, group_id),
            )
        else:
            row = await self.db.fetchone(
                "SELECT * FROM group_join_requests WHERE group_id=? AND user_id=? AND status='pending'",
                (group_id, user_id),
            )
        if row is None:
            raise InvalidParam("pending join request not found")
        now = self.clock.now_ms()
        if op == "approve":
            await self.db.execute(
                "INSERT INTO group_members(group_id,user_id,role,joined_at) VALUES(?,?,?,?) "
                "ON CONFLICT(group_id,user_id) DO NOTHING",
                (group_id, user_id, "member", now),
            )
            await self.db.execute(
                "UPDATE group_join_requests SET status='approved', decided_at=? WHERE id=?",
                (now, row["id"]),
            )
        else:
            await self.db.execute(
                "UPDATE group_join_requests SET status='declined', decided_at=?, reject_reason=? WHERE id=?",
                (now, reject_reason, row["id"]),
            )
            if add_to_blacklist:
                member = await self.db.fetchone(
                    "SELECT 1 FROM group_members WHERE group_id=? AND user_id=?", (group_id, user_id)
                )
                if member is None:
                    await self.db.execute(
                        "INSERT INTO group_blacklist(group_id,user_id,banned_at) VALUES(?,?,?) "
                        "ON CONFLICT(group_id,user_id) DO NOTHING",
                        (group_id, user_id, now),
                    )
        return {}

    # ------------------------------------------------------------------ guild writes / roles
    async def _guild_bot_row(self, guild_id: str, bot_id: str) -> Any:
        row = await self.db.fetchone(
            "SELECT * FROM guild_members WHERE guild_id=? AND user_id=? AND member_type='bot'",
            (guild_id, bot_id),
        )
        if row is None:
            raise NoPermission("bot is not a member of this guild")
        return row

    async def _emit_guild_event(self, guild_id: str, event_type: str, payload: dict[str, Any]) -> None:
        bots = await self.db.fetchall(
            "SELECT user_id FROM guild_members WHERE guild_id=? AND member_type='bot'", (guild_id,)
        )
        event_id = new_event_id()
        for row in bots:
            bot_id = row["user_id"]
            cur = await self.db.execute(
                "INSERT INTO events(event_id,type,payload_json,conversation_id,target_bot_id,created_at)"
                " VALUES(?,?,?,?,?,?)",
                (
                    event_id,
                    event_type,
                    json.dumps(payload, ensure_ascii=False),
                    f"guild:{guild_id}",
                    bot_id,
                    self.clock.now_ms(),
                ),
            )
            bot = dict(await self.bot_by_id(bot_id))
            await self._deliver(bot_id, int(cur), event_type, event_id, payload, bot, f"guild:{guild_id}")

    def _channel_out(self, channel: dict[str, Any]) -> dict[str, Any]:
        channel = dict(channel)
        channel.setdefault("sub_type", 0)
        channel.setdefault("parent_id", "0")
        channel.setdefault("owner_id", "")
        channel.setdefault("private_type", 0)
        channel.setdefault("speak_permission", 1)
        return channel

    async def create_channel(
        self, guild_id: str, bot_id: str, body: dict[str, Any]
    ) -> dict[str, Any]:
        await self._guild_bot_row(guild_id, bot_id)
        rows = await self.db.fetchall("SELECT COUNT(*) AS c FROM channels WHERE guild_id=?", (guild_id,))
        next_index = int(rows[0]["c"]) + 1 if rows else 1
        channel_id = f"channel_{guild_id[-6:]}{next_index:02d}"
        while await self.db.fetchone("SELECT 1 FROM channels WHERE id=?", (channel_id,)):
            next_index += 1
            channel_id = f"channel_{guild_id[-6:]}{next_index:02d}"
        name = str(body.get("name") or f"子频道{next_index}")
        channel_type = int(body.get("type") if body.get("type") is not None else 0)
        sub_type = int(body.get("sub_type") or 0)
        position = int(body.get("position") if body.get("position") is not None else next_index)
        await self.db.execute(
            "INSERT INTO channels(id,guild_id,name,type,position,created_at,sub_type,parent_id,owner_id,"
            "private_type,speak_permission) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            (
                channel_id,
                guild_id,
                name,
                channel_type,
                position,
                self.clock.now_ms(),
                sub_type,
                str(body.get("parent_id") or "0"),
                bot_id,
                int(body.get("private_type") or 0),
                int(body.get("speak_permission") if body.get("speak_permission") is not None else 1),
            ),
        )
        channel = self._channel_out(await self.get_channel(channel_id))
        await self._emit_guild_event(guild_id, "CHANNEL_CREATE", channel)
        return channel

    async def update_channel(self, channel_id: str, bot_id: str, body: dict[str, Any]) -> dict[str, Any]:
        channel = await self.get_channel(channel_id)
        await self._guild_bot_row(channel["guild_id"], bot_id)
        allowed = ("name", "position", "parent_id", "private_type", "speak_permission")
        fields = {k: body[k] for k in allowed if body.get(k) is not None}
        if not fields:
            raise InvalidParam("at least one field is required")
        cols = ", ".join(f"{k}=?" for k in fields)
        await self.db.execute(
            f"UPDATE channels SET {cols} WHERE id=?", (*fields.values(), channel_id)
        )
        updated = self._channel_out(await self.get_channel(channel_id))
        await self._emit_guild_event(channel["guild_id"], "CHANNEL_UPDATE", updated)
        return updated

    async def delete_channel(self, channel_id: str, bot_id: str) -> None:
        channel = await self.get_channel(channel_id)
        await self._guild_bot_row(channel["guild_id"], bot_id)
        await self.db.execute("DELETE FROM channels WHERE id=?", (channel_id,))
        await self._emit_guild_event(
            channel["guild_id"],
            "CHANNEL_DELETE",
            {"id": channel_id, "guild_id": channel["guild_id"]},
        )

    async def create_guild_role(self, guild_id: str, bot_id: str, body: dict[str, Any]) -> dict[str, Any]:
        await self._guild_bot_row(guild_id, bot_id)
        role_id = f"role_{short_token(10)}"
        name = str(body.get("name") or "新身份组")
        await self.db.execute(
            "INSERT INTO guild_roles(id,guild_id,name,color,hoist,permissions) VALUES(?,?,?,?,?,?)",
            (
                role_id,
                guild_id,
                name,
                int(body.get("color") or 0),
                int(body.get("hoist") or 0),
                str(body.get("permissions") or "0"),
            ),
        )
        return await self._role_payload(guild_id, role_id)

    async def _role_payload(self, guild_id: str, role_id: str) -> dict[str, Any]:
        row = await self.db.fetchone(
            "SELECT * FROM guild_roles WHERE guild_id=? AND id=?", (guild_id, role_id)
        )
        if row is None:
            raise InvalidParam("role not found")
        members = await self.db.fetchall("SELECT user_id, roles FROM guild_members WHERE guild_id=?", (guild_id,))
        member_ids = [m["user_id"] for m in members if role_id in json.loads(m["roles"] or "[]")]
        data = dict(row)
        data["number"] = len(member_ids)
        data["member_limit"] = 2000
        data["member_ids"] = member_ids
        return data

    async def update_guild_role(
        self, guild_id: str, bot_id: str, role_id: str, body: dict[str, Any]
    ) -> dict[str, Any]:
        await self._guild_bot_row(guild_id, bot_id)
        await self._role_payload(guild_id, role_id)
        fields: dict[str, Any] = {}
        for key in ("name", "color", "hoist", "permissions"):
            if body.get(key) is not None:
                fields[key] = body[key]
        if not fields:
            raise InvalidParam("at least one field is required")
        cols = ", ".join(f"{k}=?" for k in fields)
        await self.db.execute(
            f"UPDATE guild_roles SET {cols} WHERE guild_id=? AND id=?",
            (*fields.values(), guild_id, role_id),
        )
        return await self._role_payload(guild_id, role_id)

    async def delete_guild_role(self, guild_id: str, bot_id: str, role_id: str) -> None:
        await self._guild_bot_row(guild_id, bot_id)
        await self._role_payload(guild_id, role_id)
        await self.db.execute("DELETE FROM guild_roles WHERE guild_id=? AND id=?", (guild_id, role_id))
        members = await self.db.fetchall("SELECT user_id, roles FROM guild_members WHERE guild_id=?", (guild_id,))
        for member in members:
            roles = json.loads(member["roles"] or "[]")
            if role_id in roles:
                roles = [r for r in roles if r != role_id]
                await self.db.execute(
                    "UPDATE guild_members SET roles=? WHERE guild_id=? AND user_id=?",
                    (json.dumps(roles), guild_id, member["user_id"]),
                )

    async def assign_guild_role(self, guild_id: str, bot_id: str, user_id: str, role_id: str) -> None:
        await self._guild_bot_row(guild_id, bot_id)
        await self._role_payload(guild_id, role_id)
        member = await self.get_guild_member(guild_id, user_id)
        roles = list(member["roles"])
        if role_id not in roles:
            roles.append(role_id)
        await self.db.execute(
            "UPDATE guild_members SET roles=? WHERE guild_id=? AND user_id=?",
            (json.dumps(roles), guild_id, user_id),
        )
        await self._emit_guild_event(
            guild_id,
            "GUILD_MEMBER_UPDATE",
            {"guild_id": guild_id, "user": {"id": user_id}, "roles": roles},
        )

    async def unassign_guild_role(self, guild_id: str, bot_id: str, user_id: str, role_id: str) -> None:
        await self._guild_bot_row(guild_id, bot_id)
        member = await self.get_guild_member(guild_id, user_id)
        roles = [r for r in member["roles"] if r != role_id]
        await self.db.execute(
            "UPDATE guild_members SET roles=? WHERE guild_id=? AND user_id=?",
            (json.dumps(roles), guild_id, user_id),
        )
        await self._emit_guild_event(
            guild_id,
            "GUILD_MEMBER_UPDATE",
            {"guild_id": guild_id, "user": {"id": user_id}, "roles": roles},
        )

    async def set_guild_member_mute(
        self, guild_id: str, bot_id: str, user_id: str, body: dict[str, Any]
    ) -> None:
        await self._guild_bot_row(guild_id, bot_id)
        await self.get_guild_member(guild_id, user_id)
        now = self.clock.now_ms()
        end = str(body.get("mute_end_timestamp") or "").strip()
        seconds = str(body.get("mute_seconds") or "").strip()
        if end:
            try:
                expire = int(float(end) * 1000)
            except ValueError as exc:
                raise InvalidParam("mute_end_timestamp must be a number") from exc
        elif seconds:
            try:
                value = int(float(seconds) * 1000)
                expire = now + value if value else 0
            except ValueError as exc:
                raise InvalidParam("mute_seconds must be a number") from exc
        else:
            raise InvalidParam("mute_end_timestamp or mute_seconds is required")
        await self.db.execute(
            "UPDATE guild_members SET mute_until=? WHERE guild_id=? AND user_id=?",
            (expire, guild_id, user_id),
        )

    async def set_guild_mute(self, guild_id: str, bot_id: str, body: dict[str, Any]) -> None:
        guild = await self.get_guild(guild_id)
        await self._guild_bot_row(guild_id, bot_id)
        now = self.clock.now_ms()
        end = str(body.get("mute_end_timestamp") or "").strip()
        seconds = str(body.get("mute_seconds") or "").strip()
        if end:
            expire = int(float(end) * 1000)
        elif seconds:
            value = int(float(seconds) * 1000)
            expire = now + value if value else 0
        else:
            raise InvalidParam("mute_end_timestamp or mute_seconds is required")
        await self.db.execute("UPDATE guilds SET mute_until=? WHERE id=?", (expire, guild["id"]))

    async def remove_guild_member(self, guild_id: str, bot_id: str, user_id: str, body: dict[str, Any]) -> None:
        await self._guild_bot_row(guild_id, bot_id)
        member = await self.get_guild_member(guild_id, user_id)
        if member["member_type"] == "bot":
            raise NoPermission("cannot remove a bot with the member endpoint")
        await self.db.execute("DELETE FROM guild_members WHERE guild_id=? AND user_id=?", (guild_id, user_id))
        await self._emit_guild_event(
            guild_id,
            "GUILD_MEMBER_REMOVE",
            {"guild_id": guild_id, "user": {"id": user_id}},
        )

    # ------------------------------------------------------------------ channel DMs
    async def _dms_session(self, dms_guild_id: str) -> Any:
        row = await self.db.fetchone("SELECT * FROM dm_sessions WHERE dms_guild_id=?", (dms_guild_id,))
        if row is None:
            raise InvalidParam("DM session not found")
        return row

    async def create_dms(self, bot_id: str, recipient_id: str, source_guild_id: str) -> dict[str, Any]:
        await self.bot_by_id(bot_id)
        target = str(recipient_id)
        mapping = await self.db.fetchone(
            "SELECT * FROM openid_map WHERE bot_id=? AND openid=?", (bot_id, target)
        )
        if mapping is not None and mapping["kind"] == "user":
            user_id = mapping["entity_id"]
        else:
            user_id = target
        await self.get_user(user_id)
        await self._guild_bot_row(source_guild_id, bot_id)
        user_member = await self.db.fetchone(
            "SELECT 1 FROM guild_members WHERE guild_id=? AND user_id=?", (source_guild_id, user_id)
        )
        if user_member is None:
            raise NoPermission("bot and user must share a guild")
        existing = await self.db.fetchone(
            "SELECT * FROM dm_sessions WHERE bot_id=? AND user_id=?", (bot_id, user_id)
        )
        if existing is not None:
            return {
                "guild_id": existing["dms_guild_id"],
                "channel_id": existing["channel_id"],
                "create_time": str(int(existing["created_at"] / 1000)),
            }
        dms_guild_id = f"dmsg_{short_token(18)}"
        channel_id = f"dmch_{short_token(18)}"
        now = self.clock.now_ms()
        await self.db.execute(
            "INSERT INTO dm_sessions(dms_guild_id,channel_id,bot_id,user_id,source_guild_id,created_at)"
            " VALUES(?,?,?,?,?,?)",
            (dms_guild_id, channel_id, bot_id, user_id, source_guild_id, now),
        )
        conversation_id = f"dms:{dms_guild_id}"
        await self.db.execute(
            "INSERT INTO conversations(id,type,peer_id,bot_id,last_seq) VALUES(?,?,?,?,0) "
            "ON CONFLICT(id) DO NOTHING",
            (conversation_id, "dms", user_id, bot_id),
        )
        return {"guild_id": dms_guild_id, "channel_id": channel_id, "create_time": str(int(now / 1000))}

    async def list_dms_sessions(self) -> list[dict[str, Any]]:
        rows = await self.db.fetchall("SELECT * FROM dm_sessions ORDER BY created_at DESC")
        out = []
        for row in rows:
            user = await self.db.fetchone("SELECT nickname FROM users WHERE id=?", (row["user_id"],))
            bot = await self.db.fetchone("SELECT name FROM bots WHERE id=?", (row["bot_id"],))
            out.append(
                {
                    **dict(row),
                    "user_name": user["nickname"] if user else row["user_id"],
                    "bot_name": bot["name"] if bot else row["bot_id"],
                    "conversation_id": f"dms:{row['dms_guild_id']}",
                }
            )
        return out

    async def _dms_payload(
        self, session: Any, msg_id: str, content: str, user_id: str, sent_at: int,
        attachments: Sequence[dict[str, Any]] = (),
    ) -> dict[str, Any]:
        user = await self.db.fetchone("SELECT nickname FROM users WHERE id=?", (user_id,))
        return {
            "id": msg_id,
            "content": content,
            "channel_id": session["channel_id"],
            "guild_id": session["dms_guild_id"],
            "author": {
                "id": user_id,
                "username": user["nickname"] if user else user_id,
                "bot": False,
                "user_openid": await self.openid(session["bot_id"], "user", user_id),
                "union_openid": union_openid_for(user_id),
            },
            "member": {"nick": user["nickname"] if user else "", "roles": [], "joined_at": self.iso(sent_at)},
            "timestamp": self.iso(sent_at),
            "seq": 1,
            "attachments": list(attachments),
        }

    async def send_as_user_to_dms(
        self,
        dms_guild_id: str,
        user_id: str,
        content: str,
        *,
        attachments: Sequence[dict[str, Any]] | None = None,
        msg_type: int = 0,
    ) -> dict[str, Any]:
        session = await self._dms_session(dms_guild_id)
        if session["user_id"] != user_id:
            raise NoPermission("this DM session belongs to another user")
        conversation_id = f"dms:{dms_guild_id}"
        await self.ensure_conversation(conversation_id)
        now = self.clock.now_ms()
        msg_id = new_msg_id()
        attachment_list = list(attachments or [])
        raw = {
            "id": msg_id,
            "content": content,
            "channel_id": session["channel_id"],
            "guild_id": dms_guild_id,
            "timestamp": self.iso(now),
            "attachments": attachment_list,
        }
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
                (conversation_id, seq, msg_id, None, "in", user_id, "user", None,
                 msg_type or (7 if attachment_list else 0), content,
                 json.dumps(raw, ensure_ascii=False), now),
            )
        message = await self.db.fetchone("SELECT * FROM messages WHERE msg_id=?", (msg_id,))
        out = await self.message_out(message)  # type: ignore[arg-type]
        payload = await self._dms_payload(session, msg_id, content, user_id, now, attachment_list)
        event_id = new_event_id()
        cur = await self.db.execute(
            "INSERT INTO events(event_id,type,payload_json,conversation_id,target_bot_id,created_at)"
            " VALUES(?,?,?,?,?,?)",
            (event_id, "DIRECT_MESSAGE_CREATE", json.dumps(payload, ensure_ascii=False),
             conversation_id, session["bot_id"], now),
        )
        bot = dict(await self.bot_by_id(session["bot_id"]))
        await self._deliver(
            session["bot_id"], int(cur), "DIRECT_MESSAGE_CREATE", event_id, payload, bot, conversation_id
        )
        await self.hub.publish({"kind": "message", "conversation_id": conversation_id, "message": out})
        return {"message": out, "event_ids": [event_id], "delivered_to": [session["bot_id"]]}

    async def send_dms(self, bot_id: str, dms_guild_id: str, body: dict[str, Any]) -> dict[str, Any]:
        session = await self._dms_session(dms_guild_id)
        if session["bot_id"] != bot_id:
            raise NoPermission("this DM session belongs to another bot")
        bot = await self.bot_by_id(bot_id)
        conversation_id = f"dms:{dms_guild_id}"
        await self.ensure_conversation(conversation_id)
        msg_id_in = body.get("msg_id")
        now = self.clock.now_ms()
        if msg_id_in:
            ref = await self.db.fetchone(
                "SELECT * FROM messages WHERE msg_id=? AND conversation_id=?", (msg_id_in, conversation_id)
            )
            if ref is None or ref["direction"] != "in":
                raise MessageNotFound("msg_id does not refer to an inbound DM message")
        else:
            if not bot["allow_proactive_msg"]:
                raise NoPermission("proactive messages are disabled for this bot")
            day = self.clock.now().strftime("%Y-%m-%d")
            user_limit = int(self.setting("dms_active_user_limit", 2))
            bot_limit = int(self.setting("dms_active_bot_limit", 200))
            row = await self.db.fetchone(
                "SELECT used FROM dms_quota WHERE bot_id=? AND user_id=? AND day=?",
                (bot_id, session["user_id"], day),
            )
            used = int(row["used"]) if row else 0
            if used >= user_limit:
                raise QuotaExhausted(f"DM active quota ({user_limit}/day/user) exhausted")
            total = await self.db.fetchone(
                "SELECT COALESCE(SUM(used),0) AS c FROM dms_quota WHERE bot_id=? AND day=?", (bot_id, day)
            )
            if total and int(total["c"]) >= bot_limit:
                raise QuotaExhausted(f"DM active quota ({bot_limit}/day/bot) exhausted")
            await self.db.execute(
                "INSERT INTO dms_quota(bot_id,user_id,day,used) VALUES(?,?,?,1) "
                "ON CONFLICT(bot_id,user_id,day) DO UPDATE SET used=used+1",
                (bot_id, session["user_id"], day),
            )
        content = body.get("content") or ""
        if not content and not body.get("markdown") and not body.get("keyboard"):
            raise InvalidParam("DM message body is empty")
        msg_id = new_msg_id("MQS_DMS")
        raw = dict(body)
        raw["id"] = msg_id
        raw["timestamp"] = self.iso(now)
        raw["channel_id"] = session["channel_id"]
        raw["guild_id"] = dms_guild_id
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
                (conversation_id, seq, msg_id, None, "out", bot_id, "bot", bot_id,
                 int(body.get("msg_type") or 0), content, json.dumps(raw, ensure_ascii=False), now),
            )
        message = await self.db.fetchone("SELECT * FROM messages WHERE msg_id=?", (msg_id,))
        out = await self.message_out(message)  # type: ignore[arg-type]
        await self.hub.publish({"kind": "message", "conversation_id": conversation_id, "message": out})
        return {"id": msg_id, "timestamp": raw["timestamp"], "seq": seq, "conversation_id": conversation_id}

    async def recall_dms(self, bot_id: str, dms_guild_id: str, message_id: str) -> None:
        session = await self._dms_session(dms_guild_id)
        if session["bot_id"] != bot_id:
            raise NoPermission("this DM session belongs to another bot")
        row = await self.db.fetchone(
            "SELECT * FROM messages WHERE msg_id=? AND conversation_id=?",
            (message_id, f"dms:{dms_guild_id}"),
        )
        if row is None or row["sender_id"] != bot_id:
            raise MessageNotFound("DM message not found or not owned by this bot")
        await self.db.execute("UPDATE messages SET recalled_at=? WHERE msg_id=?", (self.clock.now_ms(), message_id))

    # ------------------------------------------------------------------ friends
    async def list_friends(self, bot_id: str) -> list[dict[str, Any]]:
        await self.bot_by_id(bot_id)
        rows = await self.db.fetchall(
            "SELECT f.*, u.nickname FROM friends f JOIN users u ON u.id=f.user_id "
            "WHERE f.bot_id=? ORDER BY f.created_at",
            (bot_id,),
        )
        return [
            {
                "user_id": row["user_id"],
                "nickname": row["nickname"],
                "openid": await self.openid(bot_id, "user", row["user_id"]),
                "union_openid": union_openid_for(row["user_id"]),
                "scene": row["scene"],
                "scene_param": row["scene_param"],
                "created_at": self.iso(int(row["created_at"])),
            }
            for row in rows
        ]

    async def _emit_friend_event(self, bot_id: str, user_id: str, event_type: str, scene: int, scene_param: str) -> None:
        payload = {
            "openid": await self.openid(bot_id, "user", user_id),
            "timestamp": int(self.clock.now_ms() / 1000),
            "author": {"union_openid": union_openid_for(user_id)},
        }
        if event_type == "FRIEND_ADD":
            payload["scene"] = scene
            payload["scene_param"] = scene_param
        event_id = new_event_id()
        cur = await self.db.execute(
            "INSERT INTO events(event_id,type,payload_json,conversation_id,target_bot_id,created_at)"
            " VALUES(?,?,?,?,?,?)",
            (event_id, event_type, json.dumps(payload, ensure_ascii=False), None, bot_id, self.clock.now_ms()),
        )
        bot = dict(await self.bot_by_id(bot_id))
        await self._deliver(bot_id, int(cur), event_type, event_id, payload, bot, "")

    async def add_friend(self, bot_id: str, user_id: str, scene: int = 1000, scene_param: str = "") -> dict[str, Any]:
        await self.bot_by_id(bot_id)
        await self.get_user(user_id)
        existing = await self.db.fetchone(
            "SELECT 1 FROM friends WHERE bot_id=? AND user_id=?", (bot_id, user_id)
        )
        if existing is None:
            await self.db.execute(
                "INSERT INTO friends(bot_id,user_id,scene,scene_param,created_at) VALUES(?,?,?,?,?)",
                (bot_id, user_id, scene, scene_param, self.clock.now_ms()),
            )
            await self._emit_friend_event(bot_id, user_id, "FRIEND_ADD", scene, scene_param)
        return {"ok": True}

    async def remove_friend(self, bot_id: str, user_id: str) -> None:
        existing = await self.db.fetchone(
            "SELECT 1 FROM friends WHERE bot_id=? AND user_id=?", (bot_id, user_id)
        )
        if existing is None:
            return
        await self.db.execute("DELETE FROM friends WHERE bot_id=? AND user_id=?", (bot_id, user_id))
        await self._emit_friend_event(bot_id, user_id, "FRIEND_DEL", 0, "")

    async def is_friend(self, bot_id: str, user_id: str) -> bool:
        row = await self.db.fetchone(
            "SELECT 1 FROM friends WHERE bot_id=? AND user_id=?", (bot_id, user_id)
        )
        return row is not None
