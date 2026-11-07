"""Auth fidelity: token issuance, expiry, error codes, trace headers."""

import httpx


async def test_token_issue_and_use(mqs) -> None:
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.post(
            "/app/getAppAccessToken", json={"appId": "10000", "clientSecret": "mqs-secret-10000"}
        )
        assert response.status_code == 200
        body = response.json()
        assert body["access_token"].startswith("MQS_")
        assert body["expires_in"] == "7200"
        token = body["access_token"]

        me = await client.get("/users/@me", headers={"Authorization": f"QQBot {token}"})
        assert me.status_code == 200
        assert me.json()["id"] == "10000"
        assert me.json()["bot"] is True
        assert me.headers["X-Tps-Trace-ID"]


async def test_wrong_secret_and_unknown_app(mqs) -> None:
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.post(
            "/app/getAppAccessToken", json={"appId": "10000", "clientSecret": "wrong"}
        )
        assert response.status_code == 401
        assert response.json()["code"] == 11244

        response = await client.post(
            "/app/getAppAccessToken", json={"appId": "99999", "clientSecret": "x"}
        )
        assert response.status_code == 401
        assert response.json()["code"] == 11243


async def test_missing_or_bad_token(mqs) -> None:
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.get("/users/@me")
        assert response.status_code == 401
        assert response.json()["code"] == 11241

        response = await client.get("/users/@me", headers={"Authorization": "QQBot not-a-token"})
        assert response.status_code == 401
        assert response.json()["code"] == 11241


async def test_token_expiry_uses_virtual_clock(mqs) -> None:
    token = await mqs.token()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        headers = {"Authorization": f"QQBot {token}"}
        assert (await client.get("/users/@me", headers=headers)).status_code == 200
        await mqs.world.db.set_setting("clock_mode", "virtual")
        mqs.world.clock.set_mode("virtual")
        mqs.world.clock.advance(7200 * 1000 + 1)
        await mqs.world.persist_clock()
        response = await client.get("/users/@me", headers=headers)
        assert response.status_code == 401
        assert response.json()["code"] == 11242


async def test_validation_errors_are_qq_shaped(mqs) -> None:
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        response = await client.post("/_mqs/chat/send", json={"content": "no user"})
        assert response.status_code == 400
        body = response.json()
        assert body["code"] == 10001
        assert "detail" not in body
        assert body["message"].startswith("invalid parameter")

        response = await client.post("/_mqs/chat/send", json={"as_user": "u_x", "to": "group:nope", "content": "x"})
        assert response.status_code == 400 or response.status_code == 403
        assert "detail" not in response.json()
