"""L2 guild family: REST shapes, channel messaging, gateway events, channel quotas."""

import asyncio
import json

import httpx


async def _guild(mqs) -> dict:
    guilds = await mqs.world.list_guilds()
    assert guilds, "bootstrap should create a default guild"
    return guilds[0]


async def _text_channel(mqs, guild: dict) -> dict:
    channels = await mqs.world.list_channels(guild["id"])
    text = [c for c in channels if c["type"] == 0]
    assert text
    return text[0]


async def test_guild_rest_shapes(mqs) -> None:
    guild = await _guild(mqs)
    channel = await _text_channel(mqs, guild)
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        guilds = await client.get("/users/@me/guilds", headers=headers)
        assert guilds.status_code == 200
        assert guilds.json()[0]["id"] == guild["id"]

        detail = await client.get(f"/guilds/{guild['id']}", headers=headers)
        assert detail.status_code == 200
        assert detail.json()["name"] == guild["name"]

        channels = await client.get(f"/guilds/{guild['id']}/channels", headers=headers)
        assert channels.status_code == 200
        assert {c["id"] for c in channels.json()} == {c["id"] for c in await mqs.world.list_channels(guild["id"])}

        members = await client.get(f"/guilds/{guild['id']}/members", headers=headers)
        assert members.status_code == 200
        assert any(m["user"]["bot"] for m in members.json())

        member = await client.get(f"/guilds/{guild['id']}/members/10000", headers=headers)
        assert member.status_code == 200
        assert member.json()["user"]["bot"] is True

        roles = await client.get(f"/guilds/{guild['id']}/roles", headers=headers)
        assert roles.status_code == 200
        assert {r["name"] for r in roles.json()["roles"]} == {"全体成员", "管理员"}

        single = await client.get(f"/channels/{channel['id']}", headers=headers)
        assert single.status_code == 200
        assert single.json()["type"] == 0


async def test_unknown_guild_is_rejected_for_non_member(mqs) -> None:
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.get("/guilds/guild_999999", headers=headers)
        assert response.status_code == 403
        assert response.json()["code"] == 40040


async def test_channel_message_events(mqs) -> None:
    guild = await _guild(mqs)
    channel = await _text_channel(mqs, guild)
    user = (await mqs.world.list_users())[0]
    ws, ready, token = await mqs.connect_gateway(intents=(1 << 9) | (1 << 30))
    try:
        async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
            response = await client.post(
                f"/_mqs/channels/{channel['id']}/messages",
                json={"as_user": user["id"], "content": "hello guild"},
            )
            assert response.status_code == 200, response.text
        frame = json.loads(await asyncio.wait_for(ws.recv(), 5))
        assert frame["t"] == "MESSAGE_CREATE"
        payload = frame["d"]
        assert payload["channel_id"] == channel["id"]
        assert payload["guild_id"] == guild["id"]
        assert payload["content"] == "hello guild"
        assert payload["author"]["bot"] is False

        async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
            response = await client.post(
                f"/_mqs/channels/{channel['id']}/messages",
                json={"as_user": user["id"], "content": "hey bot", "mentions": ["10000"]},
            )
            assert response.status_code == 200
        at_frame = json.loads(await asyncio.wait_for(ws.recv(), 5))
        assert at_frame["t"] == "AT_MESSAGE_CREATE"
        assert at_frame["d"]["mentions"] == [{"id": "10000"}]
    finally:
        await ws.close()


async def test_channel_active_send_and_rate_limit(mqs) -> None:
    guild = await _guild(mqs)
    channel = await _text_channel(mqs, guild)
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        for index in range(5):
            response = await client.post(
                f"/channels/{channel['id']}/messages",
                headers=headers,
                json={"content": f"channel {index}"},
            )
            assert response.status_code == 200, response.text
            assert response.headers["X-RateLimit-Limit"] == "5"
        rejected = await client.post(
            f"/channels/{channel['id']}/messages", headers=headers, json={"content": "too fast"}
        )
        assert rejected.status_code == 429
        assert rejected.headers["X-RateLimit-Limit"] == "5"


async def test_channel_passive_reply_window(mqs) -> None:
    guild = await _guild(mqs)
    channel = await _text_channel(mqs, guild)
    user = (await mqs.world.list_users())[0]
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        response = await client.post(
            f"/_mqs/channels/{channel['id']}/messages",
            json={"as_user": user["id"], "content": "channel ping"},
        )
        msg_id = response.json()["message"]["msg_id"]
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        reply = await client.post(
            f"/channels/{channel['id']}/messages",
            headers=headers,
            json={"content": "channel pong", "msg_id": msg_id},
        )
        assert reply.status_code == 200, reply.text
        mqs.world.clock.advance(6 * 60 * 1000)
        await mqs.world.persist_clock()
        late = await client.post(
            f"/channels/{channel['id']}/messages",
            headers=headers,
            json={"content": "too late", "msg_id": msg_id},
        )
        assert late.status_code == 400
        assert late.json()["code"] == 40057


async def test_channel_daily_and_channel_count_limits(mqs) -> None:
    guild = await _guild(mqs)
    channels = [c for c in await mqs.world.list_channels(guild["id"]) if c["type"] == 0]
    assert len(channels) >= 2
    await mqs.world.update_settings({"channel_active_channel_limit": 1, "rate_mode": "off"})
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        first = await client.post(
            f"/channels/{channels[0]['id']}/messages", headers=headers, json={"content": "first channel"}
        )
        assert first.status_code == 200, first.text
        second = await client.post(
            f"/channels/{channels[1]['id']}/messages", headers=headers, json={"content": "second channel"}
        )
        assert second.status_code == 429
        assert second.json()["code"] == 40055
