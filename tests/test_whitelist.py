"""Invite-only (内邀) interface whitelist: 11253, exposure switch, member APIs."""

import httpx


async def _group_openid(mqs) -> tuple[str, str]:
    group_id = await mqs.group_id()
    return group_id, await mqs.world.openid("10000", "group", group_id)


async def test_public_bot_gets_11253(mqs) -> None:
    _, openid = await _group_openid(mqs)
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.get(f"/v2/groups/{openid}/members", headers=headers)
    assert response.status_code == 403
    body = response.json()
    assert body["code"] == 11253
    assert "白名单" in body["hint"]


async def test_whitelisted_bot_can_list_members(mqs) -> None:
    group_id, openid = await _group_openid(mqs)
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        patched = await client.patch("/_mqs/bots/10000", json={"whitelisted": True})
        assert patched.status_code == 200
        assert patched.json()["whitelisted"] is True

    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.get(f"/v2/groups/{openid}/members", headers=headers)
        assert response.status_code == 200, response.text
        assert response.headers["X-RateLimit-Limit"] == "60"
        body = response.json()
        assert body["next_cursor"] == ""
        assert len(body["members"]) >= 2
        bots = [m for m in body["members"] if m["bot"]]
        assert bots and bots[0]["member_openid"]
        member = body["members"][0]
        for key in ("member_openid", "username", "member_role", "bot", "joined_at", "union_openid"):
            assert key in member

        detail = await client.get(f"/v2/groups/{openid}/members/{member['member_openid']}", headers=headers)
        assert detail.status_code == 200, detail.text
        assert detail.headers["X-RateLimit-Limit"] == "30"
        assert detail.json()["member_openid"] == member["member_openid"]
        assert detail.json()["member_role"] in ("owner", "admin", "member")


async def test_sandbox_can_disable_tencent_whitelist(mqs) -> None:
    _, openid = await _group_openid(mqs)
    await mqs.world.update_settings({"feature_whitelist": False})
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.get(f"/v2/groups/{openid}/members", headers=headers)
        assert response.status_code == 200, response.text


async def test_expose_public_hides_invite_endpoints(mqs) -> None:
    group_id, openid = await _group_openid(mqs)
    await mqs.world.db.execute("UPDATE bots SET whitelisted=1 WHERE id='10000'")
    await mqs.world.update_settings({"feature_expose": "public"})
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.get(f"/v2/groups/{openid}/members", headers=headers)
        assert response.status_code == 404
        assert response.json()["code"] == 404
        # public endpoints stay available
        me = await client.get("/users/@me", headers=headers)
        assert me.status_code == 200


async def test_member_list_pagination_cursor(mqs) -> None:
    group_id = await mqs.group_id()
    for index in range(35):
        user = await mqs.world.create_user(nickname=f"分页用户{index}")
        await mqs.world.add_member(group_id, user["id"], "member")
    await mqs.world.db.execute("UPDATE bots SET whitelisted=1 WHERE id='10000'")
    headers = await mqs.bot_headers()
    openid = await mqs.world.openid("10000", "group", group_id)
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        first = await client.get(f"/v2/groups/{openid}/members", headers=headers)
        assert first.status_code == 200
        body = first.json()
        assert len(body["members"]) == 30
        assert body["next_cursor"]
        second = await client.get(
            f"/v2/groups/{openid}/members?cursor={body['next_cursor']}", headers=headers
        )
        assert second.status_code == 200
        assert second.json()["members"]
        first_ids = {m["member_openid"] for m in body["members"]}
        assert not first_ids & {m["member_openid"] for m in second.json()["members"]}
