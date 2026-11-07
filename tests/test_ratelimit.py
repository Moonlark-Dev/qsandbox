"""QQ active-message rate limits (official policy, 2026-11-07).

unverified c2c : 5/s & 30/min per bot, 20/min per relation, 1000/day
certified  c2c : 10/s per bot,        20/min per relation, 1000/day
unverified group: 30/min per bot,      20/min per relation, 1000/day
certified  group: 60/min per bot,      20/min per relation, 1000/day
"""

import httpx

from mqs.domain.ratelimit import DAILY_LIMIT


async def _ensure_bot(mqs, app_id: str = "10001") -> dict:
    bot = await mqs.world.bot_by_app_id(app_id)
    if bot is None:
        await mqs.world.create_bot(
            app_id=app_id,
            name=f"RL Bot {app_id}",
            scopes=["group_message", "c2c_message", "media"],
        )
    return await mqs.bot_headers(app_id, f"mqs-secret-{app_id}")


async def _user(mqs) -> dict:
    return (await mqs.world.list_users())[0]


async def _active_c2c(client, headers, openid, content="hi"):
    return await client.post(
        f"/v2/users/{openid}/messages", headers=headers, json={"content": content}
    )


async def test_unverified_c2c_5_qps(mqs) -> None:
    await mqs.world.update_settings({"rate_mode": "unverified", "require_online": False})
    headers = await _ensure_bot(mqs)
    user = await _user(mqs)
    openid = await mqs.world.openid("10001", "user", user["id"])
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        for index in range(5):
            response = await _active_c2c(client, headers, openid, f"c2c {index}")
            assert response.status_code == 200, response.text
            assert response.headers["X-RateLimit-Limit"] == "5"
        rejected = await _active_c2c(client, headers, openid, "sixth")
        assert rejected.status_code == 429
        assert rejected.json()["code"] == 42900
        assert float(rejected.headers["Retry-After"]) > 0
        assert rejected.headers["X-RateLimit-Limit"] == "5"


async def test_certified_c2c_10_qps(mqs) -> None:
    await mqs.world.update_settings({"rate_mode": "certified", "require_online": False})
    headers = await _ensure_bot(mqs)
    user = await _user(mqs)
    openid = await mqs.world.openid("10001", "user", user["id"])
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        for index in range(10):
            response = await _active_c2c(client, headers, openid, f"cert {index}")
            assert response.status_code == 200, response.text
        rejected = await _active_c2c(client, headers, openid, "eleventh")
        assert rejected.status_code == 429
        assert rejected.headers["X-RateLimit-Limit"] == "10"


async def test_unverified_group_bot_30_per_minute(mqs) -> None:
    await mqs.world.update_settings({"rate_mode": "unverified", "require_online": False})
    headers = await _ensure_bot(mqs)
    user = await _user(mqs)
    groups = []
    for _ in range(2):
        groups.append(await mqs.world.create_group(owner_id=user["id"], member_ids=[], bot_ids=["10001"]))
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        sent = 0
        for group in groups:
            openid = await mqs.world.openid("10001", "group", group["id"])
            for _ in range(15):
                response = await client.post(
                    f"/v2/groups/{openid}/messages", headers=headers, json={"content": f"g{sent}"}
                )
                assert response.status_code == 200, response.text
                sent += 1
        assert sent == 30
        openid = await mqs.world.openid("10001", "group", groups[0]["id"])
        rejected = await client.post(
            f"/v2/groups/{openid}/messages", headers=headers, json={"content": "over bot limit"}
        )
        assert rejected.status_code == 429
        assert rejected.headers["X-RateLimit-Limit"] == "30"


async def test_relation_20_per_minute(mqs) -> None:
    await mqs.world.update_settings({"rate_mode": "certified", "require_online": False})
    headers = await _ensure_bot(mqs)
    user = await _user(mqs)
    group = await mqs.world.create_group(owner_id=user["id"], member_ids=[], bot_ids=["10001"])
    openid = await mqs.world.openid("10001", "group", group["id"])
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        for index in range(20):
            response = await client.post(
                f"/v2/groups/{openid}/messages", headers=headers, json={"content": f"rel {index}"}
            )
            assert response.status_code == 200, response.text
        rejected = await client.post(
            f"/v2/groups/{openid}/messages", headers=headers, json={"content": "21st"}
        )
        assert rejected.status_code == 429
        assert rejected.headers["X-RateLimit-Limit"] == "20"


async def test_daily_quota_1000(mqs) -> None:
    await mqs.world.update_settings({"rate_mode": "off", "require_online": False})
    headers = await _ensure_bot(mqs)
    user = await _user(mqs)
    group = await mqs.world.create_group(owner_id=user["id"], member_ids=[], bot_ids=["10001"])
    day = mqs.world.clock.now().strftime("%Y-%m-%d")
    await mqs.world.db.execute(
        "INSERT INTO active_quota(bot_id,conversation_id,day,used) VALUES(?,?,?,?) "
        "ON CONFLICT(bot_id,conversation_id,day) DO UPDATE SET used=excluded.used",
        ("10001", f"group:{group['id']}", day, DAILY_LIMIT),
    )
    openid = await mqs.world.openid("10001", "group", group["id"])
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.post(
            f"/v2/groups/{openid}/messages", headers=headers, json={"content": "over daily"}
        )
        assert response.status_code == 429
        assert response.json()["code"] == 40055


async def test_off_mode_is_unlimited(mqs) -> None:
    await mqs.world.update_settings({"rate_mode": "off", "require_online": False})
    headers = await _ensure_bot(mqs)
    user = await _user(mqs)
    openid = await mqs.world.openid("10001", "user", user["id"])
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        for index in range(12):
            response = await _active_c2c(client, headers, openid, f"off {index}")
            assert response.status_code == 200, response.text


async def test_passive_replies_are_not_rate_limited(mqs) -> None:
    """Passive replies have their own window/count, not the active bucket."""
    await mqs.world.update_settings({"rate_mode": "unverified", "require_online": False})
    headers = await _ensure_bot(mqs)
    user = await _user(mqs)
    group = await mqs.world.create_group(owner_id=user["id"], member_ids=[user["id"]], bot_ids=["10001"])
    conversation = f"group:{group['id']}"
    await mqs.world.send_as_user(conversation, user["id"], "ping")
    message = await mqs.last_inbound(conversation)
    openid = await mqs.world.openid("10001", "group", group["id"])
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        # the group passive allowance is 5, all 5 must pass without hitting the 5/s active bucket
        for seq in range(1, 6):
            response = await client.post(
                f"/v2/groups/{openid}/messages",
                headers=headers,
                json={"content": f"reply {seq}", "msg_id": message["msg_id"], "msg_seq": seq},
            )
            assert response.status_code == 200, response.text
