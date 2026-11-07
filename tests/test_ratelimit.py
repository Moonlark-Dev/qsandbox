"""Token-bucket rate limiting and its headers."""

import httpx


async def _setup(mqs, mode: str = "strict") -> tuple[dict, str, str]:
    await mqs.world.update_settings({"rate_mode": mode, "require_online": False})
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        await client.post(
            "/_mqs/chat/send",
            json={"as_user": await mqs.user_id(), "to": f"group:{await mqs.group_id()}", "content": "rl"},
        )
    message = await mqs.last_inbound(f"group:{await mqs.group_id()}")
    event = (await mqs.world.list_events())[-1]
    return await mqs.bot_headers(), message["msg_id"], event["payload"]["group_openid"]


async def test_strict_mode_rejects_third_message(mqs) -> None:
    headers, msg_id, group_openid = await _setup(mqs, "strict")
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        for seq in (1, 2):
            response = await client.post(
                f"/v2/groups/{group_openid}/messages",
                headers=headers,
                json={"content": f"ok {seq}", "msg_id": msg_id, "msg_seq": seq},
            )
            assert response.status_code == 200, response.text
            assert response.headers["X-RateLimit-Limit"] == "2"
        rejected = await client.post(
            f"/v2/groups/{group_openid}/messages",
            headers=headers,
            json={"content": "too fast", "msg_id": msg_id, "msg_seq": 3},
        )
        assert rejected.status_code == 429
        assert rejected.json()["code"] == 42900
        assert float(rejected.headers["Retry-After"]) > 0
        assert float(rejected.headers["X-RateLimit-Reset"]) > 0

    mqs.world.clock.advance(3000)
    await mqs.world.persist_clock()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        recovered = await client.post(
            f"/v2/groups/{group_openid}/messages",
            headers=headers,
            json={"content": "after refill", "msg_id": msg_id, "msg_seq": 4},
        )
        assert recovered.status_code == 200


async def test_off_mode_is_unlimited(mqs) -> None:
    headers, msg_id, group_openid = await _setup(mqs, "off")
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        for seq in range(1, 8):
            response = await client.post(
                f"/v2/groups/{group_openid}/messages",
                headers=headers,
                json={"content": f"unlimited {seq}"},
            )
            assert response.status_code == 200, (seq, response.text)


async def test_rate_limit_is_per_bot(mqs) -> None:
    headers, msg_id, group_openid = await _setup(mqs, "strict")
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        await client.post("/_mqs/bots", json={"app_id": "10001", "name": "Other Bot"})
        await client.post(f"/_mqs/groups/{await mqs.group_id()}/bots/10001")
    other = await mqs.bot_headers("10001", "mqs-secret-10001")
    # map the same group openid for the other bot
    other_openid = await mqs.world.openid("10001", "group", await mqs.group_id())
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.post(
            f"/v2/groups/{other_openid}/messages",
            headers=other,
            json={"content": "other bot", "msg_id": msg_id, "msg_seq": 1},
        )
        # the msg_id belongs to 10000's scoped conversation; this must not be a rate-limit error
        assert response.status_code in (200, 400)
        assert response.status_code != 429
