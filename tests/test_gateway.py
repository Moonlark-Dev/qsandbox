"""Gateway protocol: hello/identify/ready/heartbeat/resume/intents/errors."""

import asyncio
import json

import httpx
import pytest
import websockets
from websockets.exceptions import ConnectionClosed


async def test_hello_identify_ready_and_heartbeat(mqs) -> None:
    ws, ready, token = await mqs.connect_gateway()
    try:
        assert ready["version"] == 1
        assert ready["shard"] == [0, 1]
        assert ready["user"]["id"] == "10000"
        await ws.send(json.dumps({"op": 1, "d": 0}))
        ack = json.loads(await asyncio.wait_for(ws.recv(), 2))
        assert ack == {"op": 11, "d": None}
    finally:
        await ws.close()


async def test_intents_filtering(mqs) -> None:
    ws, ready, token = await mqs.connect_gateway(intents=1)  # guilds only, not public messages
    try:
        async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
            response = await client.post(
                "/_mqs/chat/send",
                json={"as_user": await mqs.user_id(), "to": f"group:{await mqs.group_id()}", "content": "not for you", "at_bots": ["10000"]},
            )
            assert response.status_code == 200
        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(ws.recv(), 0.5)
    finally:
        await ws.close()


async def test_resume_replays_missed_events(mqs) -> None:
    ws, ready, token = await mqs.connect_gateway()
    session_id = ready["session_id"]
    await ws.close()
    await asyncio.sleep(0.3)

    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        response = await client.post(
            "/_mqs/chat/send",
            json={"as_user": await mqs.user_id(), "to": f"group:{await mqs.group_id()}", "content": "while offline", "at_bots": ["10000"]},
        )
        assert response.status_code == 200

    ws2 = await websockets.connect(mqs.ws_url())
    try:
        assert json.loads(await ws2.recv())["op"] == 10
        await ws2.send(
            json.dumps({"op": 6, "d": {"token": f"QQBot {token}", "session_id": session_id, "seq": 0}})
        )
        resumed = json.loads(await asyncio.wait_for(ws2.recv(), 2))
        assert resumed["t"] == "RESUMED"
        event = None
        for _ in range(10):
            frame = json.loads(await asyncio.wait_for(ws2.recv(), 2))
            if frame.get("t") == "GROUP_AT_MESSAGE_CREATE":
                event = frame
                break
        assert event is not None, "missed group message was not replayed"
        assert "while offline" in event["d"]["content"]
        assert event["s"] >= 1
    finally:
        await ws2.close()


async def test_invalid_session_closes_socket(mqs) -> None:
    token = await mqs.token()
    ws = await websockets.connect(mqs.ws_url())
    try:
        await ws.recv()
        await ws.send(json.dumps({"op": 6, "d": {"token": f"QQBot {token}", "session_id": "bogus", "seq": 0}}))
        invalid = json.loads(await ws.recv())
        assert invalid["op"] == 9
        assert invalid["d"] is False
        with pytest.raises(ConnectionClosed):
            await asyncio.wait_for(ws.recv(), 2)
    finally:
        await ws.close()


async def test_bad_identify_token_closes_4004(mqs) -> None:
    ws = await websockets.connect(mqs.ws_url())
    try:
        await ws.recv()
        await ws.send(json.dumps({"op": 2, "d": {"token": "QQBot nope", "intents": 1 << 25}}))
        with pytest.raises(ConnectionClosed) as exc:
            await asyncio.wait_for(ws.recv(), 2)
        assert exc.value.rcvd is not None and exc.value.rcvd.code == 4004
    finally:
        await ws.close()


async def test_gateway_url_and_frames_are_logged(mqs) -> None:
    headers = await mqs.bot_headers()
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.get("/gateway/bot", headers=headers)
        assert response.status_code == 200
        body = response.json()
        assert body["url"].startswith("ws://")
        assert body["shards"] == 1

    ws, ready, token = await mqs.connect_gateway()
    await ws.close()
    await asyncio.sleep(0.3)
    frames = await mqs.world.list_frames(limit=100, kind="ws")
    summaries = " ".join(frame["summary"] for frame in frames)
    assert "READY" in summaries
    assert "gateway disconnected" in summaries
