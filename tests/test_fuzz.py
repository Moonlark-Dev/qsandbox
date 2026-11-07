"""Fuzz-ish robustness: malformed input must produce QQ-shaped errors, never 500."""

import asyncio

import httpx
import pytest
import websockets
from websockets.exceptions import ConnectionClosed


async def _targets(mqs) -> tuple[str, dict]:
    group_id = await mqs.group_id()
    openid = await mqs.world.openid("10000", "group", group_id)
    return openid, await mqs.bot_headers()


@pytest.mark.parametrize(
    "body",
    [
        b"not json at all",
        b"[]",
        b'{"content": 123}',
        b'{"msg_seq": "abc"}',
        b'{"msg_type": "text"}',
        b'{"content": null}',
        b'{"media": "not-an-object"}',
        b'{"message_reference": {"message_id": 42}}',
    ],
)
async def test_malformed_message_bodies(mqs, body: bytes) -> None:
    openid, headers = await _targets(mqs)
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.post(
            f"/v2/groups/{openid}/messages",
            headers={**headers, "Content-Type": "application/json"},
            content=body,
        )
    assert response.status_code < 500, response.text
    payload = response.json()
    assert payload["code"] != 10002
    assert "trace_id" in payload


async def test_malformed_token_request(mqs) -> None:
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        for body in (b"", b"{}", b'{"appId": {}, "clientSecret": []}', b"not-json"):
            response = await client.post(
                "/app/getAppAccessToken",
                content=body,
                headers={"Content-Type": "application/json"},
            )
            assert response.status_code < 500, (body, response.text)
            assert response.json()["code"] == 10001


async def test_unknown_guild_and_channel_are_404(mqs) -> None:
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        for path in ("/guilds/does-not-exist", "/channels/does-not-exist"):
            response = await client.get(path, headers=headers)
            assert response.status_code in (400, 403, 404)
            assert response.json()["code"] != 10002


async def test_gateway_malformed_json_closes_4002(mqs) -> None:
    ws = await websockets.connect(mqs.ws_url())
    try:
        await ws.recv()
        await ws.send("this is not json")
        with pytest.raises(ConnectionClosed) as exc:
            await asyncio.wait_for(ws.recv(), 2)
        assert exc.value.rcvd is not None
        assert exc.value.rcvd.code == 4002
    finally:
        await ws.close()


async def test_unknown_intents_and_shard_are_rejected(mqs) -> None:
    token = await mqs.token()
    ws = await websockets.connect(mqs.ws_url())
    try:
        await ws.recv()
        await ws.send(
            '{"op": 2, "d": {"token": "QQBot ' + token + '", "intents": 0, "shard": [5, 2]}}'
        )
        with pytest.raises(ConnectionClosed) as exc:
            await asyncio.wait_for(ws.recv(), 2)
        assert exc.value.rcvd is not None
        assert exc.value.rcvd.code == 4010
    finally:
        await ws.close()
