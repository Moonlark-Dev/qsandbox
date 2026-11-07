"""Media upload/download and the file_type contract."""

import hashlib

import httpx

PNG = (
    b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00"
    b"\x1f\x15\xc4\x89\x00\x00\x00\nIDATx\x9cc\x00\x01\x00\x00\x05\x00\x01\r\n-\xb4\x00\x00"
    b"\x00\x00IEND\xaeB`\x82"
)


async def test_upload_and_send_media(mqs) -> None:
    ws, ready, token = await mqs.connect_gateway()
    headers = {"Authorization": f"QQBot {token}"}
    try:
        group_openid = await mqs.world.openid("10000", "group", await mqs.group_id())
        async with httpx.AsyncClient(base_url=mqs.sim_base, timeout=30) as client:
            upload = await client.post(
                f"/v2/groups/{group_openid}/files",
                headers=headers,
                files={"file": ("a.png", PNG, "image/png")},
                data={"file_type": "1"},
            )
            assert upload.status_code == 200, upload.text
            body = upload.json()
            assert body["file_info"].startswith("mqs-file-")
            assert body["ttl"] == 0
            digest = body["file_uuid"]
            assert digest == hashlib.sha256(PNG).hexdigest()

            sent = await client.post(
                f"/v2/groups/{group_openid}/messages",
                headers=headers,
                json={
                    "content": "",
                    "msg_type": 7,
                    "media": {"file_info": body["file_info"]},
                },
            )
            assert sent.status_code == 200, sent.text

            download = await client.get(f"/media/{digest}")
            assert download.status_code == 200
            assert download.content == PNG
            assert download.headers["content-type"].startswith("image/")
    finally:
        await ws.close()


async def test_upload_by_media_url(mqs) -> None:
    headers = await mqs.bot_headers()
    group_openid = await mqs.world.openid("10000", "group", await mqs.group_id())
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        stored = await mqs.world.store_media(PNG, "image/png")
        response = await client.post(
            f"/v2/groups/{group_openid}/files",
            headers=headers,
            json={"file_type": 1, "url": f"/media/{stored['hash']}"},
        )
        assert response.status_code == 200, response.text
        assert response.json()["file_uuid"] == stored["hash"]


async def test_upload_rejects_bad_type_and_oversize(mqs) -> None:
    headers = await mqs.bot_headers()
    group_openid = await mqs.world.openid("10000", "group", await mqs.group_id())
    async with httpx.AsyncClient(base_url=mqs.sim_base, timeout=60) as client:
        bad_type = await client.post(
            f"/v2/groups/{group_openid}/files",
            headers=headers,
            files={"file": ("a.bin", b"hello", "application/octet-stream")},
            data={"file_type": "4"},
        )
        assert bad_type.status_code == 400
        assert bad_type.json()["code"] == 10001

        mismatched = await client.post(
            f"/v2/groups/{group_openid}/files",
            headers=headers,
            files={"file": ("a.txt", b"hello", "text/plain")},
            data={"file_type": "1"},
        )
        assert mismatched.status_code == 400

        big = b"\x89PNG\r\n\x1a\n" + b"0" * (10 * 1024 * 1024 + 1)
        oversize = await client.post(
            f"/v2/groups/{group_openid}/files",
            headers=headers,
            files={"file": ("big.png", big, "image/png")},
            data={"file_type": "1"},
        )
        assert oversize.status_code == 400
        assert "too large" in oversize.json()["message"]


async def test_media_path_is_safe(mqs) -> None:
    async with httpx.AsyncClient(base_url=mqs.sim_base) as client:
        assert (await client.get("/media/not-a-hash")).status_code == 400
        assert (await client.get("/media/" + "0" * 64)).status_code == 400
        traversal = await client.get("/media/..%2F..%2Fetc%2Fpasswd")
        assert traversal.status_code in (400, 404)
