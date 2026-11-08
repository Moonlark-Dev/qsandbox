"""Shared fixtures: one in-process MQS server per test."""

from __future__ import annotations

import asyncio
import json
from collections.abc import AsyncIterator
from typing import Any

import httpx
import pytest
import websockets

from mqs.config import ProcessConfig, write_runtime
from mqs.server import run_headless


class MQS:
    def __init__(self, ctx: dict[str, Any]) -> None:
        self.ctx = ctx
        self.handshake = ctx["handshake"]
        self.world = ctx["world"]
        self.config: ProcessConfig = ctx["config"]
        self.sim_base = f"http://127.0.0.1:{self.handshake['sim_port']}"
        self.control_base = f"http://127.0.0.1:{self.handshake['control_port']}"
        self.headers = {"X-MQS-Token": self.handshake["token"]}

    async def sim(self) -> httpx.AsyncClient:
        return httpx.AsyncClient(base_url=self.sim_base, timeout=20)

    async def ctl(self) -> httpx.AsyncClient:
        return httpx.AsyncClient(base_url=self.control_base, headers=self.headers, timeout=30)

    async def token(self, app_id: str = "10000", secret: str = "mqs-secret-10000") -> str:
        async with httpx.AsyncClient(timeout=10) as client:
            response = await client.post(
                self.sim_base + "/app/getAppAccessToken", json={"appId": app_id, "clientSecret": secret}
            )
            response.raise_for_status()
            return response.json()["access_token"]

    async def bot_headers(self, app_id: str = "10000", secret: str = "mqs-secret-10000") -> dict[str, str]:
        return {"Authorization": f"QQBot {await self.token(app_id, secret)}"}

    def ws_url(self) -> str:
        return f"ws://127.0.0.1:{self.handshake['sim_port']}/websocket"

    async def connect_gateway(self, app_id: str = "10000", intents: int = 1 << 25) -> tuple[Any, dict, str]:
        token = await self.token(app_id, f"mqs-secret-{app_id}")
        ws = await websockets.connect(self.ws_url())
        hello = json.loads(await ws.recv())
        assert hello["op"] == 10
        await ws.send(json.dumps({"op": 2, "d": {"token": f"QQBot {token}", "intents": intents, "shard": [0, 1]}}))
        ready = json.loads(await ws.recv())
        assert ready["t"] == "READY", ready
        return ws, ready["d"], token

    async def last_inbound(self, conversation_id: str) -> dict:
        messages = await self.world.list_messages(conversation_id, limit=50)
        for message in reversed(messages):
            if message["direction"] == "in":
                return message
        raise AssertionError("no inbound message")

    async def group_id(self) -> str:
        groups = await self.world.list_groups()
        return groups[0]["id"]

    async def user_id(self) -> str:
        users = await self.world.list_users()
        return users[0]["id"]


@pytest.fixture
async def mqs(tmp_path) -> AsyncIterator[MQS]:
    cfg = ProcessConfig(profile="test", home=tmp_path)
    ctx = await run_headless(cfg)
    server = MQS(ctx)
    write_runtime(cfg.home, cfg.profile, server.handshake)
    try:
        yield server
    finally:
        await ctx["gateway"].stop()
        await ctx["world"].stop()
        ctx["sim_server"].should_exit = True
        ctx["control_server"].should_exit = True
        await asyncio.gather(ctx["sim_task"], ctx["control_task"], return_exceptions=True)
        await ctx["db"].close()


@pytest.fixture
async def echo_bot(mqs: MQS) -> AsyncIterator[asyncio.Task]:
    """A minimal SUT bot: replies 'echo: <text>' to group @ and C2C messages."""

    stop = asyncio.Event()
    token = await mqs.token()

    async def run() -> None:
        async with websockets.connect(mqs.ws_url()) as ws:
            await ws.recv()
            await ws.send(json.dumps({"op": 2, "d": {"token": f"QQBot {token}", "intents": 1 << 25, "shard": [0, 1]}}))
            ready = json.loads(await ws.recv())
            assert ready["t"] == "READY"

            async def heartbeat() -> None:
                while not stop.is_set():
                    await asyncio.sleep(20)
                    try:
                        await ws.send(json.dumps({"op": 1, "d": 0}))
                    except Exception:  # noqa: BLE001
                        return

            hb = asyncio.create_task(heartbeat())
            try:
                async for raw in ws:
                    frame = json.loads(raw)
                    if frame.get("op") != 0 or frame.get("t") not in (
                        "GROUP_AT_MESSAGE_CREATE",
                        "GROUP_MESSAGE_CREATE",
                        "C2C_MESSAGE_CREATE",
                    ):
                        continue
                    data = frame["d"]
                    import re

                    is_at = frame["t"] == "GROUP_AT_MESSAGE_CREATE"
                    if frame["t"] == "GROUP_MESSAGE_CREATE":
                        # full-message mode: only answer when this bot was @'d
                        is_at = any(m.get("is_you") for m in data.get("mentions") or [])
                        if not is_at:
                            continue
                    text = re.sub(r"<@![^>]+>", "", data.get("content", "")).strip()
                    if frame["t"] in ("GROUP_AT_MESSAGE_CREATE", "GROUP_MESSAGE_CREATE"):
                        url = f"/v2/groups/{data['group_openid']}/messages"
                    else:
                        url = f"/v2/users/{data['author']['user_openid']}/messages"
                    async with httpx.AsyncClient(base_url=mqs.sim_base, timeout=10) as client:
                        await client.post(
                            url,
                            headers={"Authorization": f"QQBot {token}"},
                            json={"content": f"echo: {text}", "msg_id": data["id"]},
                        )
            finally:
                hb.cancel()

    task = asyncio.create_task(run())
    await asyncio.sleep(0.3)
    try:
        yield task
    finally:
        stop.set()
        task.cancel()
        try:
            await task
        except (asyncio.CancelledError, Exception):  # noqa: BLE001
            pass
