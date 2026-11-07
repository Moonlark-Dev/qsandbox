"""Group management: info, mute, blacklist, join requests, proactive switch."""

import httpx


async def _bot_headers(mqs, app_id: str = "10000") -> dict:
    return await mqs.bot_headers(app_id)


async def _whitelist(mqs, app_id: str = "10000") -> None:
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        response = await client.patch(f"/_mqs/bots/{app_id}", json={"whitelisted": True})
        assert response.status_code == 200


async def _group_openid(mqs, group_id: str, bot_id: str = "10000") -> str:
    return await mqs.world.openid(bot_id, "group", group_id)


async def test_group_info_and_bot_state(mqs) -> None:
    await _whitelist(mqs)
    gid = await mqs.group_id()
    openid = await _group_openid(mqs, gid)
    headers = await _bot_headers(mqs)
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        info = await client.get(f"/v2/groups/{openid}/info", headers=headers)
        assert info.status_code == 200, info.text
        body = info.json()
        assert body["group_openid"] == openid
        assert body["group_name"] == (await mqs.world.get_group(gid))["name"]
        assert body["group_member_num"] >= 3

        state = await client.get(f"/v2/groups/{openid}/bot_state", headers=headers)
        assert state.status_code == 200, state.text
        assert state.json()["allow_proactive_msg"] is True
        assert state.json()["recv_msg_setting"] == "all"


async def test_group_mute_add_and_del(mqs) -> None:
    await _whitelist(mqs)
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        await client.patch("/_mqs/settings", json={"require_online": False})
        await client.patch("/_mqs/bots/10000", json={"whitelisted": True})
    gid = await mqs.group_id()
    group = await mqs.world.get_group(gid)
    members = await mqs.world.list_members(gid)
    target = next(m for m in members if m["role"] == "member" and m["user_id"] != group["owner_id"])
    openid = await _group_openid(mqs, gid)
    member_openid = await mqs.world.openid("10000", "user", target["user_id"])
    headers = await _bot_headers(mqs)
    expire = mqs.world.iso(mqs.world.clock.now_ms() + 10 * 60 * 1000)
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        set_resp = await client.post(
            f"/v2/groups/{openid}/restrict_chat_setting",
            headers=headers,
            json={
                "members": [
                    {"op": "add", "member_openid": member_openid, "mute_expire_at": expire}
                ]
            },
        )
        assert set_resp.status_code == 200, set_resp.text
        settings = await client.get(f"/v2/groups/{openid}/restrict_chat_setting", headers=headers)
        assert settings.status_code == 200
        assert any(m["member_openid"] == member_openid for m in settings.json()["members"])

    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        blocked = await client.post(
            "/_mqs/chat/send",
            json={"as_user": target["user_id"], "to": f"group:{gid}", "content": "muted", "at_bots": []},
        )
        assert blocked.status_code == 403, blocked.text

    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        del_resp = await client.post(
            f"/v2/groups/{openid}/restrict_chat_setting",
            headers=headers,
            json={"members": [{"op": "del", "member_openid": member_openid}]},
        )
        assert del_resp.status_code == 200
    assert (await mqs.world.list_members(gid))[0] is not None


async def test_group_blacklist_add_remove(mqs) -> None:
    await _whitelist(mqs)
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        outsider = (await client.post("/_mqs/users", json={"nickname": "黑名单访客"})).json()
    gid = await mqs.group_id()
    openid = await _group_openid(mqs, gid)
    member_openid = await mqs.world.openid("10000", "user", outsider["id"])
    headers = await _bot_headers(mqs)
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        add = await client.post(
            f"/v2/groups/{openid}/member_blacklist",
            headers=headers,
            json={"op": "add", "member_openids": [member_openid]},
        )
        assert add.status_code == 200, add.text
        assert add.json()["fail_openids"] == []
        listed = await client.get(f"/v2/groups/{openid}/member_blacklist", headers=headers)
        assert any(u["member_openid"] == member_openid for u in listed.json()["users"])
        removed = await client.post(
            f"/v2/groups/{openid}/member_blacklist",
            headers=headers,
            json={"op": "del", "member_openids": [member_openid]},
        )
        assert removed.status_code == 200
    listed = await mqs.world.group_blacklist_page(gid, "10000")
    assert not listed["users"]


async def test_join_request_approve_and_decline(mqs) -> None:
    await _whitelist(mqs)
    gid = await mqs.group_id()
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        outsider = (await client.post("/_mqs/users", json={"nickname": "申请入群者"})).json()
        request = await client.post(
            f"/_mqs/groups/{gid}/join-requests",
            json={"user_id": outsider["id"], "verify_message": "我想进群"},
        )
        assert request.status_code == 200, request.text
        request_id = request.json()["id"]
    openid = await _group_openid(mqs, gid)
    member_openid = await mqs.world.openid("10000", "user", outsider["id"])
    headers = await _bot_headers(mqs)
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        listed = await client.get(f"/v2/groups/{openid}/join_request_list", headers=headers)
        assert listed.status_code == 200, listed.text
        item = next(x for x in listed.json()["list"] if x["join_request_id"] == request_id)
        assert item["verify_info"]["verify_message"] == "我想进群"
        approved = await client.post(
            f"/v2/groups/{openid}/approval_join_request/{member_openid}",
            headers=headers,
            json={"op": "approve", "join_request_id": request_id},
        )
        assert approved.status_code == 200, approved.text
    members = await mqs.world.list_members(gid)
    assert any(m["user_id"] == outsider["id"] for m in members)

    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        outsider2 = (await client.post("/_mqs/users", json={"nickname": "被拒者"})).json()
        request2 = await client.post(
            f"/_mqs/groups/{gid}/join-requests", json={"user_id": outsider2["id"]}
        )
        assert request2.status_code == 200
        request2_id = request2.json()["id"]
    member_openid2 = await mqs.world.openid("10000", "user", outsider2["id"])
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        declined = await client.post(
            f"/v2/groups/{openid}/approval_join_request/{member_openid2}",
            headers=headers,
            json={
                "op": "decline",
                "join_request_id": request2_id,
                "reject_reason": "测试拒绝",
                "add_to_member_blacklist": True,
            },
        )
        assert declined.status_code == 200, declined.text
    black = await mqs.world.group_blacklist_page(gid, "10000")
    assert any(u["username"] == "被拒者" for u in black["users"])


async def test_proactive_switch_blocks_active_messages(mqs) -> None:
    await _whitelist(mqs)
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        await client.patch("/_mqs/settings", json={"require_online": False})
    gid = await mqs.group_id()
    openid = await _group_openid(mqs, gid)
    headers = await _bot_headers(mqs)
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        active = await client.post(
            f"/v2/groups/{openid}/messages", headers=headers, json={"content": "主动消息"}
        )
        assert active.status_code == 200, active.text
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        toggled = await client.patch(
            f"/_mqs/groups/{gid}/bots/10000", json={"allow_proactive_msg": False}
        )
        assert toggled.status_code == 200, toggled.text
        assert toggled.json()["allow_proactive_msg"] is False
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        blocked = await client.post(
            f"/v2/groups/{openid}/messages", headers=headers, json={"content": "再发一条"}
        )
        assert blocked.status_code == 403, blocked.text
        assert blocked.json()["code"] == 40040
