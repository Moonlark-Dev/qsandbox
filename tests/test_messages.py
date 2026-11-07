"""Message semantics: delivery, openid scoping, passive window, dedupe, recall."""

import json

import httpx


async def _send_user(mqs, content: str = "hello", at_bots=None) -> dict:
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        response = await client.post(
            "/_mqs/chat/send",
            json={
                "as_user": await mqs.user_id(),
                "to": f"group:{await mqs.group_id()}",
                "content": content,
                "at_bots": at_bots,
            },
        )
        assert response.status_code == 200, response.text
        return response.json()


async def test_user_message_reaches_bot_with_mention(mqs) -> None:
    ws, ready, token = await mqs.connect_gateway()
    try:
        await _send_user(mqs, "ping")
        frame = json.loads(await ws.recv())
        assert frame["t"] == "GROUP_AT_MESSAGE_CREATE"
        payload = frame["d"]
        assert payload["content"].startswith("<@!")
        assert payload["group_id"] == await mqs.group_id()
        assert payload["author"]["member_openid"] == payload["author"]["id"]
        assert len(payload["group_openid"]) == 32
    finally:
        await ws.close()


async def test_openids_are_scoped_per_bot(mqs) -> None:
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        create = await client.post("/_mqs/bots", json={"app_id": "10001", "name": "Second Bot"})
        assert create.status_code == 200
        gid = await mqs.group_id()
        assert (await client.post(f"/_mqs/groups/{gid}/bots/10001")).status_code == 200
    await _send_user(mqs, "both of you", at_bots=["10000", "10001"])
    events = await mqs.world.list_events()
    group_events = [e for e in events if e["type"] == "GROUP_AT_MESSAGE_CREATE"]
    by_bot = {e["target_bot_id"]: e["payload"] for e in group_events}
    assert set(by_bot) == {"10000", "10001"}
    assert by_bot["10000"]["group_openid"] != by_bot["10001"]["group_openid"]
    assert by_bot["10000"]["author"]["member_openid"] != by_bot["10001"]["author"]["member_openid"]


async def test_passive_reply_and_duplicate_seq(mqs) -> None:
    ws, ready, token = await mqs.connect_gateway()
    headers = {"Authorization": f"QQBot {token}"}
    try:
        await _send_user(mqs, "reply please")
        message = await mqs.last_inbound(f"group:{await mqs.group_id()}")
        async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
            event = await mqs.world.list_events()
            group_openid = event[-1]["payload"]["group_openid"]
            first = await client.post(
                f"/v2/groups/{group_openid}/messages",
                headers=headers,
                json={"content": "first", "msg_id": message["msg_id"], "msg_seq": 1},
            )
            assert first.status_code == 200, first.text
            duplicate = await client.post(
                f"/v2/groups/{group_openid}/messages",
                headers=headers,
                json={"content": "dup", "msg_id": message["msg_id"], "msg_seq": 1},
            )
            assert duplicate.status_code == 400
            assert duplicate.json()["code"] == 40054
            assert duplicate.headers["X-Tps-Trace-ID"]
    finally:
        await ws.close()


async def test_passive_reply_limit_and_expiry(mqs) -> None:
    ws, ready, token = await mqs.connect_gateway()
    headers = {"Authorization": f"QQBot {token}"}
    try:
        await _send_user(mqs, "five replies max")
        message = await mqs.last_inbound(f"group:{await mqs.group_id()}")
        event = (await mqs.world.list_events())[-1]
        group_openid = event["payload"]["group_openid"]
        async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
            for seq in range(1, 6):
                response = await client.post(
                    f"/v2/groups/{group_openid}/messages",
                    headers=headers,
                    json={"content": f"reply {seq}", "msg_id": message["msg_id"], "msg_seq": seq},
                )
                assert response.status_code == 200, response.text
            sixth = await client.post(
                f"/v2/groups/{group_openid}/messages",
                headers=headers,
                json={"content": "sixth", "msg_id": message["msg_id"]},
            )
            assert sixth.status_code == 400
            assert sixth.json()["code"] == 40057

            mqs.world.clock.advance(6 * 60 * 1000)
            await mqs.world.persist_clock()
            late = await client.post(
                f"/v2/groups/{group_openid}/messages",
                headers=headers,
                json={"content": "late", "msg_id": message["msg_id"], "msg_seq": 9},
            )
            assert late.status_code == 400
            assert late.json()["code"] == 40057
    finally:
        await ws.close()


async def test_unknown_fields_are_preserved(mqs) -> None:
    ws, ready, token = await mqs.connect_gateway()
    headers = {"Authorization": f"QQBot {token}"}
    try:
        await _send_user(mqs, "passthrough")
        message = await mqs.last_inbound(f"group:{await mqs.group_id()}")
        event = (await mqs.world.list_events())[-1]
        group_openid = event["payload"]["group_openid"]
        async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
            response = await client.post(
                f"/v2/groups/{group_openid}/messages",
                headers=headers,
                json={
                    "content": "with extras",
                    "msg_id": message["msg_id"],
                    "msg_seq": 2,
                    "future_field": {"segments": [{"type": "new_thing", "data": {"x": 1}}]},
                },
            )
            assert response.status_code == 200
        stored = [m for m in await mqs.world.list_messages(f"group:{await mqs.group_id()}") if m["direction"] == "out"]
        assert stored[-1]["raw"]["future_field"]["segments"][0]["type"] == "new_thing"
    finally:
        await ws.close()


async def test_recall_only_own_messages(mqs) -> None:
    ws, ready, token = await mqs.connect_gateway()
    headers = {"Authorization": f"QQBot {token}"}
    try:
        await _send_user(mqs, "recall me")
        message = await mqs.last_inbound(f"group:{await mqs.group_id()}")
        event = (await mqs.world.list_events())[-1]
        group_openid = event["payload"]["group_openid"]
        async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
            reply = await client.post(
                f"/v2/groups/{group_openid}/messages",
                headers=headers,
                json={"content": "to recall", "msg_id": message["msg_id"]},
            )
            reply_id = reply.json()["id"]
            deleted = await client.delete(f"/v2/groups/{group_openid}/messages/{reply_id}", headers=headers)
            assert deleted.status_code == 204
            not_mine = await client.delete(
                f"/v2/groups/{group_openid}/messages/{message['msg_id']}", headers=headers
            )
            assert not_mine.status_code == 404
            assert not_mine.json()["code"] == 40034
    finally:
        await ws.close()


async def test_bot_offline_and_active_quota(mqs) -> None:
    headers = await mqs.bot_headers()
    await _send_user(mqs, "offline?")
    await mqs.last_inbound(f"group:{await mqs.group_id()}")
    event = (await mqs.world.list_events())[-1]
    group_openid = event["payload"]["group_openid"]
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.post(
            f"/v2/groups/{group_openid}/messages", headers=headers, json={"content": "am I online?"}
        )
        assert response.status_code == 403
        assert response.json()["code"] == 40056

    await mqs.world.update_settings({"require_online": False})
    await mqs.world.update_settings({"active_quota_per_day": 1})
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        first = await client.post(
            f"/v2/groups/{group_openid}/messages", headers=headers, json={"content": "active 1"}
        )
        assert first.status_code == 200, first.text
        second = await client.post(
            f"/v2/groups/{group_openid}/messages", headers=headers, json={"content": "active 2"}
        )
        assert second.status_code == 429
        assert second.json()["code"] == 40055


async def test_mention_only_delivered_to_mentioned_bot(mqs) -> None:
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        await client.post("/_mqs/bots", json={"app_id": "10001", "name": "Second Bot"})
        gid = await mqs.group_id()
        await client.post(f"/_mqs/groups/{gid}/bots/10001")
    await _send_user(mqs, "only 10001", at_bots=["10001"])
    events = await mqs.world.list_events()
    group_events = [e for e in events if e["type"] == "GROUP_AT_MESSAGE_CREATE"]
    assert {e["target_bot_id"] for e in group_events} == {"10001"}
