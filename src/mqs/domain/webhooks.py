"""Webhook delivery mode (the second half of QQ's event delivery story)."""

from __future__ import annotations

import asyncio
import hashlib
import json
import time
from typing import TYPE_CHECKING, Any

import httpx
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

if TYPE_CHECKING:  # pragma: no cover
    from .world import World


def _signing_key(secret: str) -> Ed25519PrivateKey:
    seed = hashlib.sha256(secret.encode("utf-8")).digest()
    return Ed25519PrivateKey.from_private_bytes(seed)


def sign_handshake(secret: str, event_ts: str, plain_token: str) -> str:
    key = _signing_key(secret)
    return key.sign(f"{event_ts}{plain_token}".encode()).hex()


class WebhookDispatcher:
    def __init__(self, world: World) -> None:
        self.world = world
        self._client: httpx.AsyncClient | None = None

    @property
    def client(self) -> httpx.AsyncClient:
        if self._client is None:
            self._client = httpx.AsyncClient(timeout=httpx.Timeout(3.0))
        return self._client

    async def close(self) -> None:
        if self._client is not None:
            await self._client.aclose()
            self._client = None

    async def endpoints(self, bot_id: str) -> list[Any]:
        return await self.world.db.fetchall(
            "SELECT * FROM webhook_endpoints WHERE bot_id=? AND enabled=1", (bot_id,)
        )

    async def deliver(self, bot_id: str, frame: dict[str, Any]) -> None:
        rows = await self.endpoints(bot_id)
        for row in rows:
            asyncio.create_task(self._post(row, frame))

    async def _post(self, endpoint: Any, frame: dict[str, Any]) -> None:
        body = json.dumps(frame, ensure_ascii=False).encode("utf-8")
        ts = str(int(time.time()))
        secret = endpoint["secret"]
        signature = _signing_key(secret).sign(ts.encode() + body).hex()
        headers = {
            "Content-Type": "application/json",
            "X-Signature-Ed25519": signature,
            "X-Signature-Timestamp": ts,
            "X-MQS-Event": frame.get("t", ""),
            "X-TPS-trace-ID": self.world.new_trace_id(),
        }
        for attempt in range(2):
            try:
                resp = await self.client.post(endpoint["url"], content=body, headers=headers)
                await self.world.log_frame(
                    "out",
                    "http",
                    f"webhook {frame.get('t')} -> {endpoint['url']} [{resp.status_code}]",
                    {"url": endpoint["url"], "status": resp.status_code, "body": body.decode("utf-8", "replace")[:4000]},
                    bot_id=endpoint["bot_id"],
                )
                if resp.status_code < 500:
                    return
            except httpx.HTTPError as exc:
                await self.world.log_frame(
                    "out",
                    "http",
                    f"webhook {frame.get('t')} -> {endpoint['url']} error: {exc}",
                    {"url": endpoint["url"], "error": str(exc)},
                    bot_id=endpoint["bot_id"],
                )
            await asyncio.sleep(0.2 * (attempt + 1))
