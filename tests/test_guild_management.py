"""Channel writes, roles/members, channel DMs, friends and proactive switches."""

import asyncio
import json

import httpx


async def _guild(mqs) -> dict:
    guilds = await mqs.world.list_guilds()
    assert guilds
    return guilds[0]


async def _channel(mqs, guild: dict) -> dict:
    channels = await mqs.world.list_channels(guild["id"])
    return next(c for c in channels if c["type"] == 0)


async def _user(mqs) -> dict:
    return (await mqs.world.list_users())[0]


async def test_channel_create_update_delete(mqs) -> None:
    guild = await _guild(mqs)
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        created = await client.post(
            f"/guilds/{guild['id']}/channels",
            headers=headers,
            json={"name": "公告区", "type": 0, "sub_type": 1, "position": 9},
        )
        assert created.status_code == 200, created.text
        channel = created.json()
        assert channel["name"] == "公告区"
        assert channel["sub_type"] == 1

        patched = await client.patch(
            f"/channels/{channel['id']}", headers=headers, json={"name": "公告区（已改名）"}
        )
        assert patched.status_code == 200, patched.text
        assert patched.json()["name"] == "公告区（已改名）"

        deleted = await client.delete(f"/channels/{channel['id']}", headers=headers)
        assert deleted.status_code == 204
        assert all(c["id"] != channel["id"] for c in await mqs.world.list_channels(guild["id"]))


async def test_role_crud_and_member_assignment(mqs) -> None:
    guild = await _guild(mqs)
    user = await _user(mqs)
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        created = await client.post(
            f"/guilds/{guild['id']}/roles", headers=headers, json={"name": "测试身份组", "color": 99999}
        )
        assert created.status_code == 200, created.text
        role = created.json()
        assert role["name"] == "测试身份组"

        updated = await client.patch(
            f"/guilds/{guild['id']}/roles/{role['id']}", headers=headers, json={"hoist": 1}
        )
        assert updated.status_code == 200
        assert updated.json()["hoist"] == 1

        assigned = await client.put(
            f"/guilds/{guild['id']}/members/{user['id']}/roles/{role['id']}", headers=headers
        )
        assert assigned.status_code == 204
        roles = (await client.get(f"/guilds/{guild['id']}/roles", headers=headers)).json()["roles"]
        target = next(r for r in roles if r["id"] == role["id"])
        assert user["id"] in target["member_ids"]
        assert target["number"] >= 1

        removed = await client.delete(
            f"/guilds/{guild['id']}/members/{user['id']}/roles/{role['id']}", headers=headers
        )
        assert removed.status_code == 204

        deleted = await client.delete(f"/guilds/{guild['id']}/roles/{role['id']}", headers=headers)
        assert deleted.status_code == 204
        assert all(r["id"] != role["id"] for r in await mqs.world.list_guild_roles(guild["id"]))


async def test_guild_member_mute_and_remove(mqs) -> None:
    guild = await _guild(mqs)
    members = await mqs.world.list_guild_members(guild["id"])
    target = next(m for m in members if m["member_type"] == "user")
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        muted = await client.patch(
            f"/guilds/{guild['id']}/members/{target['user_id']}/mute",
            headers=headers,
            json={"mute_seconds": "120"},
        )
        assert muted.status_code == 204
        after = await mqs.world.get_guild_member(guild["id"], target["user_id"])
        assert after["mute_until"] > 0

        unmuted = await client.patch(
            f"/guilds/{guild['id']}/members/{target['user_id']}/mute",
            headers=headers,
            json={"mute_seconds": "0"},
        )
        assert unmuted.status_code == 204
        assert (await mqs.world.get_guild_member(guild["id"], target["user_id"]))["mute_until"] == 0

        removed = await client.delete(
            f"/guilds/{guild['id']}/members/{target['user_id']}", headers=headers
        )
        assert removed.status_code == 204
        assert all(m["user_id"] != target["user_id"] for m in await mqs.world.list_guild_members(guild["id"]))


async def test_channel_dms_create_send_and_active_quota(mqs) -> None:
    guild = await _guild(mqs)
    user = await _user(mqs)
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        created = await client.post(
            "/users/@me/dms",
            headers=headers,
            json={"recipient_id": user["id"], "source_guild_id": guild["id"]},
        )
        assert created.status_code == 200, created.text
        dms = created.json()
        assert dms["guild_id"].startswith("dmsg_")
        assert dms["channel_id"].startswith("dmch_")

        first = await client.post(
            f"/dms/{dms['guild_id']}/messages", headers=headers, json={"content": "私信一"}
        )
        assert first.status_code == 200, first.text
        second = await client.post(
            f"/dms/{dms['guild_id']}/messages", headers=headers, json={"content": "私信二"}
        )
        assert second.status_code == 200, second.text
        third = await client.post(
            f"/dms/{dms['guild_id']}/messages", headers=headers, json={"content": "私信三"}
        )
        assert third.status_code == 429, third.text
        assert third.json()["code"] == 40055

    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        inbound = await client.post(
            f"/_mqs/dms/{dms['guild_id']}/messages",
            json={"as_user": user["id"], "content": "用户回一句"},
        )
        assert inbound.status_code == 200, inbound.text
    message = (await mqs.world.list_messages(f"dms:{dms['guild_id']}"))[-1]
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        passive = await client.post(
            f"/dms/{dms['guild_id']}/messages",
            headers=headers,
            json={"content": "被动回复", "msg_id": message["msg_id"]},
        )
        assert passive.status_code == 200, passive.text


async def test_dms_event_and_friend_events(mqs) -> None:
    guild = await _guild(mqs)
    user = await _user(mqs)
    headers = await mqs.bot_headers()
    ws, ready, token = await mqs.connect_gateway(intents=(1 << 25) | (1 << 12))
    try:
        async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
            dms = (
                await client.post(
                    "/users/@me/dms",
                    headers=headers,
                    json={"recipient_id": user["id"], "source_guild_id": guild["id"]},
                )
            ).json()
        async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
            await client.post(
                f"/_mqs/dms/{dms['guild_id']}/messages",
                json={"as_user": user["id"], "content": "私信事件"},
            )
        frame = json.loads(await asyncio.wait_for(ws.recv(), 3))
        assert frame["t"] == "DIRECT_MESSAGE_CREATE"
        assert frame["d"]["guild_id"] == dms["guild_id"]
        assert frame["d"]["content"] == "私信事件"

        async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
            added = await client.post(f"/_mqs/bots/10000/friends/{user['id']}")
            assert added.status_code == 200
        frame = json.loads(await asyncio.wait_for(ws.recv(), 3))
        assert frame["t"] == "FRIEND_ADD"
        friends = await mqs.world.list_friends("10000")
        assert any(f["user_id"] == user["id"] for f in friends)
        async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
            removed = await client.delete(f"/_mqs/bots/10000/friends/{user['id']}")
            assert removed.status_code == 204
        frame = json.loads(await asyncio.wait_for(ws.recv(), 3))
        assert frame["t"] == "FRIEND_DEL"
    finally:
        await ws.close()


async def test_channel_active_blocked_by_proactive_switch(mqs) -> None:
    guild = await _guild(mqs)
    channel = await _channel(mqs, guild)
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        await client.patch("/_mqs/bots/10000", json={"allow_proactive_msg": False})
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        blocked = await client.post(
            f"/channels/{channel['id']}/messages", headers=headers, json={"content": "主动频道消息"}
        )
        assert blocked.status_code == 403, blocked.text
        assert blocked.json()["code"] == 40040
