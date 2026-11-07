"""User rich messages: control upload + inbound attachments on all scenes."""

import asyncio
import json

import httpx

PNG = (
    b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01"
    b"\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\nIDATx\x9cc\x00\x01"
    b"\x00\x00\x05\x00\x01\r\n-\xb4\x00\x00\x00\x00IEND\xaeB`\x82"
)


async def _upload(mqs) -> dict:
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        response = await client.post(
            "/_mqs/media",
            files={"file": ("pixel.png", PNG, "image/png")},
        )
        assert response.status_code == 200, response.text
        return response.json()


async def test_upload_and_group_rich_message(mqs) -> None:
    media = await _upload(mqs)
    assert media["content_type"] == "image/png"
    assert media["url"].startswith("/media/")
    ws, ready, token = await mqs.connect_gateway()
    try:
        async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
            sent = await client.post(
                "/_mqs/chat/send",
                json={
                    "as_user": await mqs.user_id(),
                    "to": f"group:{await mqs.group_id()}",
                    "content": "",
                    "at_bots": ["10000"],
                    "attachments": [media],
                },
            )
            assert sent.status_code == 200, sent.text
            message = sent.json()["message"]
            assert message["msg_type"] == 7
            assert message["attachments"][0]["url"] == media["url"]
        frame = json.loads(await asyncio.wait_for(ws.recv(), 3))
        assert frame["t"] == "GROUP_AT_MESSAGE_CREATE"
        assert frame["d"]["attachments"][0]["content_type"] == "image/png"
        assert frame["d"]["attachments"][0]["url"] == media["url"]
    finally:
        await ws.close()


async def test_upload_and_channel_rich_message(mqs) -> None:
    media = await _upload(mqs)
    guild = (await mqs.world.list_guilds())[0]
    channel = next(c for c in await mqs.world.list_channels(guild["id"]) if c["type"] == 0)
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        sent = await client.post(
            f"/_mqs/channels/{channel['id']}/messages",
            json={"as_user": await mqs.user_id(), "content": "", "attachments": [media]},
        )
        assert sent.status_code == 200, sent.text
        message = sent.json()["message"]
        assert message["msg_type"] == 7
        assert message["attachments"][0]["url"] == media["url"]


async def test_upload_rejects_non_media(mqs) -> None:
    async with httpx.AsyncClient(base_url=mqs.control_base, headers=mqs.headers) as client:
        response = await client.post(
            "/_mqs/media",
            files={"file": ("note.txt", b"hello", "text/plain")},
        )
        assert response.status_code == 400, response.text
        assert response.json()["code"] == 10001
