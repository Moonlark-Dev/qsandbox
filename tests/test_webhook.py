"""Webhook mode: URL verification handshake and signed delivery."""

import hashlib

import httpx
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from mqs.domain.webhooks import sign_handshake


def _verify(secret: str, event_ts: str, plain_token: str, signature: str) -> None:
    key = Ed25519PrivateKey.from_private_bytes(hashlib.sha256(secret.encode()).digest())
    public = key.public_key()
    public.verify(bytes.fromhex(signature), f"{event_ts}{plain_token}".encode())


async def test_handshake_signature(mqs) -> None:
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        response = await client.post(
            "/webhook/10000", json={"op": 0, "d": {"plain_token": "abc", "event_ts": "1700000000"}}
        )
    assert response.status_code == 200
    body = response.json()
    assert body["plain_token"] == "abc"
    _verify("mqs-secret-10000", "1700000000", "abc", body["signature"])
    assert body["signature"] == sign_handshake("mqs-secret-10000", "1700000000", "abc")


async def test_register_webhook_endpoint(mqs) -> None:
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        created = await client.post("/_mqs/bots/10000/webhook", json={"url": "http://127.0.0.1:9/hook"})
        assert created.status_code == 200, created.text
        listed = await client.get("/_mqs/bots/10000/webhook")
        assert listed.status_code == 200
        assert listed.json()[0]["url"] == "http://127.0.0.1:9/hook"


async def test_webhook_delivery_is_attempted_and_logged(mqs) -> None:
    # port 9 (discard) is closed; delivery must be attempted and logged, never crash the send
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        await client.post("/_mqs/bots/10000/webhook", json={"url": "http://127.0.0.1:9/hook"})
        response = await client.post(
            "/_mqs/chat/send",
            json={"as_user": await mqs.user_id(), "to": f"group:{await mqs.group_id()}", "content": "webhook"},
        )
        assert response.status_code == 200
    import asyncio

    await asyncio.sleep(0.5)
    frames = await mqs.world.list_frames(limit=50, kind="http")
    assert any("webhook" in frame["summary"] for frame in frames)
