"""The simulated QQ Bot API (second listener)."""

from __future__ import annotations

import re
from typing import Any

from fastapi import APIRouter, Depends, FastAPI, Request, Response, WebSocket
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse, JSONResponse

from ..config import ProcessConfig
from ..domain.world import World
from ..errors import InvalidParam
from ..ids import avatar_svg, new_msg_id
from ..models import AppAccessTokenRequest, MediaUploadRequest, MessageSendRequest
from .common import (
    SimLogMiddleware,
    TraceMiddleware,
    get_world,
    install_error_handlers,
    require_bot,
    require_whitelisted_bot,
)

router = APIRouter()

HASH_RE = re.compile(r"^[0-9a-f]{64}$")
AVATAR_RE = re.compile(r"^[A-Za-z0-9_]{1,64}$")
FILE_LIMITS = {1: 10 * 1024 * 1024, 2: 30 * 1024 * 1024, 3: 5 * 1024 * 1024}


def _ws_base(request: Request) -> str:
    base = str(request.base_url).rstrip("/")
    if base.startswith("https://"):
        return base.replace("https://", "wss://", 1)
    return base.replace("http://", "ws://", 1)


@router.post("/app/getAppAccessToken")
async def get_app_access_token(request: Request, payload: AppAccessTokenRequest) -> dict:
    world: World = get_world(request)
    return await world.issue_token(str(payload.appId), payload.clientSecret)


@router.get("/gateway")
async def gateway_url(request: Request, bot: dict = Depends(require_bot)) -> dict:
    return {"url": f"{_ws_base(request)}/websocket"}


@router.get("/gateway/bot")
async def gateway_bot(request: Request, bot: dict = Depends(require_bot)) -> dict:
    return {
        "url": f"{_ws_base(request)}/websocket",
        "shards": 1,
        "session_start_limit": {"total": 1000, "remaining": 999, "reset_after": 86400000, "max_concurrency": 1},
    }


@router.get("/users/@me")
async def bot_me(bot: dict = Depends(require_bot)) -> dict:
    return {
        "id": bot["app_id"],
        "username": bot["name"],
        "avatar": bot["avatar"],
        "bot": True,
        "union_openid": None,
        "share_url": None,
    }


@router.get("/users/@me/guilds")
async def bot_guilds(request: Request, bot: dict = Depends(require_bot)) -> list:
    world: World = get_world(request)
    guilds = await world.list_guilds(bot_id=bot["id"])
    return [
        {
            "id": g["id"],
            "name": g["name"],
            "icon": g["icon"],
            "owner_id": g["owner_id"],
            "member_count": g["member_count"],
        }
        for g in guilds
    ]


@router.get("/v2/groups/{group_openid}/members")
async def group_members(
    group_openid: str,
    request: Request,
    response: Response,
    bot: dict = Depends(require_whitelisted_bot),
    cursor: str = "",
) -> dict:
    world: World = get_world(request)
    mapping = await world.resolve_openid(bot["id"], group_openid)
    if mapping["kind"] != "group":
        raise InvalidParam("openid is not a group openid")
    info = world.rate.check_read(bot["id"], "group_members", 60, 60_000)
    response.headers["X-RateLimit-Limit"] = str(info.limit)
    response.headers["X-RateLimit-Remaining"] = str(info.remaining)
    response.headers["X-RateLimit-Reset"] = f"{info.reset_after:.2f}"
    return await world.group_members_page(mapping["entity_id"], bot["id"], cursor=cursor, limit=30)


@router.get("/v2/groups/{group_openid}/members/{member_openid}")
async def group_member_detail(
    group_openid: str,
    member_openid: str,
    request: Request,
    response: Response,
    bot: dict = Depends(require_whitelisted_bot),
) -> dict:
    world: World = get_world(request)
    mapping = await world.resolve_openid(bot["id"], group_openid)
    if mapping["kind"] != "group":
        raise InvalidParam("openid is not a group openid")
    info = world.rate.check_read(bot["id"], "group_member", 30, 60_000)
    response.headers["X-RateLimit-Limit"] = str(info.limit)
    response.headers["X-RateLimit-Remaining"] = str(info.remaining)
    response.headers["X-RateLimit-Reset"] = f"{info.reset_after:.2f}"
    return await world.group_member_detail(mapping["entity_id"], bot["id"], member_openid)


async def _require_guild_member(world: World, bot_id: str, guild_id: str) -> None:
    from ..errors import NoPermission

    row = await world.db.fetchone(
        "SELECT 1 FROM guild_members WHERE guild_id=? AND user_id=? AND member_type='bot'",
        (guild_id, bot_id),
    )
    if row is None:
        raise NoPermission("bot is not a member of this guild")


@router.post("/users/@me/dms")
async def create_dm(bot: dict = Depends(require_bot)) -> dict:
    return {"guild_id": "mqs-stub-guild", "channel_id": new_msg_id("MQS_DM"), "create_time": 0}


def _attach_rate_headers(response: Response, result: dict) -> dict:
    info = result.pop("_rate", None)
    if info:
        response.headers["X-RateLimit-Limit"] = str(info["limit"])
        response.headers["X-RateLimit-Remaining"] = str(max(0, info["remaining"]))
        response.headers["X-RateLimit-Reset"] = f"{info['reset_after']:.2f}"
    return result


# ---------------------------------------------------------------------------
# group / c2c messages
# ---------------------------------------------------------------------------

@router.post("/v2/groups/{group_openid}/messages")
async def send_group_message(
    group_openid: str,
    payload: MessageSendRequest,
    request: Request,
    response: Response,
    bot: dict = Depends(require_bot),
) -> dict:
    world: World = get_world(request)
    result = await world.send_as_bot(
        bot["id"],
        target_kind="group",
        target_openid=group_openid,
        body=payload.model_dump(exclude_none=True),
    )
    return _attach_rate_headers(response, result)


@router.delete("/v2/groups/{group_openid}/messages/{message_id}")
async def delete_group_message(
    group_openid: str, message_id: str, request: Request, bot: dict = Depends(require_bot)
) -> Response:
    world: World = get_world(request)
    await world.recall_message(bot["id"], "group", group_openid, message_id)
    return Response(status_code=204)


@router.post("/v2/users/{openid}/messages")
async def send_c2c_message(
    openid: str,
    payload: MessageSendRequest,
    request: Request,
    response: Response,
    bot: dict = Depends(require_bot),
) -> dict:
    world: World = get_world(request)
    result = await world.send_as_bot(
        bot["id"],
        target_kind="user",
        target_openid=openid,
        body=payload.model_dump(exclude_none=True),
    )
    return _attach_rate_headers(response, result)


@router.delete("/v2/users/{openid}/messages/{message_id}")
async def delete_c2c_message(openid: str, message_id: str, request: Request, bot: dict = Depends(require_bot)) -> Response:
    world: World = get_world(request)
    await world.recall_message(bot["id"], "user", openid, message_id)
    return Response(status_code=204)


# ---------------------------------------------------------------------------
# media
# ---------------------------------------------------------------------------

def _check_file_type(file_type: int) -> int:
    if file_type == 4:
        raise InvalidParam("file_type 4 (generic file) is not supported in QSandbox v0.1")
    if file_type not in FILE_LIMITS:
        raise InvalidParam(f"invalid file_type: {file_type}")
    return FILE_LIMITS[file_type]


def _sniff_mime(data: bytes) -> str:
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "image/png"
    if data.startswith(b"\xff\xd8\xff"):
        return "image/jpeg"
    if data.startswith(b"GIF8"):
        return "image/gif"
    if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return "image/webp"
    if data[4:8] == b"ftyp":
        return "video/mp4"
    if data.startswith(b"#!AMR"):
        return "audio/amr"
    if data.startswith(b"\x02#!SILK"):
        return "audio/silk"
    return "application/octet-stream"


async def _store_upload(request: Request, file_type: int, data: bytes, mime: str | None) -> dict:
    world: World = get_world(request)
    limit = _check_file_type(file_type)
    if not data:
        raise InvalidParam("empty file")
    if len(data) > limit:
        raise InvalidParam(f"file too large: {len(data)} > {limit} bytes")
    mime = mime or "application/octet-stream"
    if file_type == 1 and not mime.startswith("image/"):
        raise InvalidParam("file_type 1 expects an image")
    return await world.store_media(data, mime)


async def _upload(request: Request, *, kind: str, target_openid: str, bot: dict) -> dict:
    world: World = get_world(request)
    content_type = request.headers.get("content-type", "")
    srv_send_msg = False
    file_type = 1
    if content_type.startswith("multipart/"):
        form = await request.form()
        upload = form.get("file") or form.get("media")
        if upload is None or not hasattr(upload, "read"):
            raise InvalidParam("multipart upload requires a 'file' field")
        file_type = int(form.get("file_type") or request.query_params.get("file_type") or 1)
        srv_send_msg = str(form.get("srv_send_msg") or "").lower() in ("1", "true")
        data = await upload.read()
        mime = upload.content_type
        media = await _store_upload(request, file_type, data, mime)
    else:
        body = await request.json()
        req = MediaUploadRequest(**body)
        file_type = req.file_type
        srv_send_msg = req.srv_send_msg
        if body.get("upload_id"):
            return await world.finalize_upload(
                bot["id"], kind, target_openid, str(body["upload_id"]),
                srv_send_msg=body.get("srv_send_msg"),
            )
        file_data = body.get("file_data") or body.get("fileData")
        if file_data:
            import base64
            import binascii

            try:
                decoded = base64.b64decode(str(file_data), validate=False)
            except (ValueError, binascii.Error) as exc:
                raise InvalidParam(f"invalid base64 file_data: {exc}") from None
            media = await _store_upload(request, file_type, decoded, _sniff_mime(decoded))
        elif req.url:
            media = await _resolve_media_url(request, req.url, file_type)
        else:
            raise InvalidParam("url or file_data is required for JSON upload")
    mhash = media["hash"]
    result: dict[str, Any] = {
        "file_uuid": mhash,
        "file_info": f"mqs-file-{mhash}",
        "ttl": 0,
        "file_type": file_type,
    }
    if srv_send_msg:
        sent = await world.send_as_bot(
            bot["id"],
            target_kind="group" if kind == "group" else "user",
            target_openid=target_openid,
            body={"msg_type": 7, "content": "", "media": {"file_info": result["file_info"]}},
        )
        sent.pop("_rate", None)
        result["message"] = sent
    return result


async def _resolve_media_url(request: Request, url: str, file_type: int) -> dict:
    world: World = get_world(request)
    _check_file_type(file_type)
    candidate = url
    if candidate.startswith("mqs://media/"):
        candidate = "/media/" + candidate[len("mqs://media/") :]
    if candidate.startswith("/media/"):
        mhash = candidate[len("/media/") :]
        if not HASH_RE.match(mhash):
            raise InvalidParam("invalid media hash")
        media = await world.media_by_hash(mhash)
        if media is None:
            raise InvalidParam("media not found")
        return media
    if candidate.startswith(("http://", "https://")):
        if not world.setting("allow_remote_media", False):
            raise InvalidParam("remote media URLs are disabled (enable allow_remote_media in settings)")
        import httpx

        async with httpx.AsyncClient(timeout=10) as client:
            resp = await client.get(candidate)
            if resp.status_code != 200:
                raise InvalidParam(f"failed to fetch media: HTTP {resp.status_code}")
            return await _store_upload(request, file_type, resp.content, resp.headers.get("content-type"))
    raise InvalidParam("unsupported media url (use /media/<sha256> or mqs://media/<sha256>)")


async def _prepare_upload(request: Request, kind: str, target_openid: str, bot: dict) -> dict:
    world: World = get_world(request)
    body = await request.json()
    info = await world.prepare_upload(bot["id"], kind, target_openid, body)
    base = str(request.base_url).rstrip("/")
    parts = [
        {"index": index, "presigned_url": f"{base}/v2/uploads/{info['upload_id']}/parts/{index}"}
        for index in range(info["count"])
    ]
    return {
        "upload_id": info["upload_id"],
        "block_size": info["block_size"],
        "parts": parts,
        "upload_config": {"concurrency": 1, "retry_timeout": 0, "retry_delay": 0},
    }


@router.post("/v2/groups/{group_openid}/upload_prepare")
async def group_upload_prepare(group_openid: str, request: Request, bot: dict = Depends(require_bot)) -> dict:
    return await _prepare_upload(request, "group", group_openid, bot)


@router.post("/v2/users/{openid}/upload_prepare")
async def c2c_upload_prepare(openid: str, request: Request, bot: dict = Depends(require_bot)) -> dict:
    return await _prepare_upload(request, "user", openid, bot)


@router.put("/v2/uploads/{upload_id}/parts/{index}")
async def put_upload_part(upload_id: str, index: int, request: Request) -> Response:
    world: World = get_world(request)
    world.put_upload_part(upload_id, index, await request.body())
    return Response(status_code=204)


async def _finish_part(request: Request) -> Response:
    world: World = get_world(request)
    body = await request.json()
    world.finish_upload_part(
        str(body.get("upload_id") or ""),
        int(body.get("part_index") or 0),
        int(body.get("block_size") or 0),
        str(body.get("md5") or ""),
    )
    return Response(status_code=204)


@router.post("/v2/groups/{group_openid}/upload_part_finish")
async def group_upload_part_finish(group_openid: str, request: Request, bot: dict = Depends(require_bot)) -> Response:
    return await _finish_part(request)


@router.post("/v2/users/{openid}/upload_part_finish")
async def c2c_upload_part_finish(openid: str, request: Request, bot: dict = Depends(require_bot)) -> Response:
    return await _finish_part(request)


@router.post("/v2/groups/{group_openid}/files")
async def upload_group_file(group_openid: str, request: Request, bot: dict = Depends(require_bot)) -> dict:
    return await _upload(request, kind="group", target_openid=group_openid, bot=bot)


@router.post("/v2/users/{openid}/files")
async def upload_c2c_file(openid: str, request: Request, bot: dict = Depends(require_bot)) -> dict:
    return await _upload(request, kind="user", target_openid=openid, bot=bot)


@router.get("/media/{hash}")
async def download_media(hash: str, request: Request) -> Response:
    world: World = get_world(request)
    if not HASH_RE.match(hash):
        raise InvalidParam("invalid media hash")
    media = await world.media_by_hash(hash)
    if media is None:
        raise InvalidParam("media not found")
    path = world.paths.media / hash
    resolved = path.resolve()
    if not str(resolved).startswith(str(world.paths.media.resolve())) or not resolved.exists():
        raise InvalidParam("media not found")
    return FileResponse(resolved, media_type=media["mime"], filename=hash[:12])


@router.get("/avatars/{key}.svg")
async def avatar(key: str, label: str = "", kind: str = "user") -> Response:
    if not AVATAR_RE.match(key):
        raise InvalidParam("invalid avatar key")
    if kind not in ("bot", "user", "npc", "group", "guild"):
        kind = "user"
    return Response(
        content=avatar_svg(key, label[:16], kind),
        media_type="image/svg+xml",
        headers={"Cache-Control": "public, max-age=3600"},
    )


@router.get("/guilds/{guild_id}")
async def guild_get(guild_id: str, request: Request, bot: dict = Depends(require_bot)) -> dict:
    world: World = get_world(request)
    await _require_guild_member(world, bot["id"], guild_id)
    guild = await world.get_guild(guild_id)
    return {
        "id": guild["id"],
        "name": guild["name"],
        "icon": guild["icon"],
        "owner_id": guild["owner_id"],
        "member_count": guild["member_count"],
        "description": "",
    }


@router.get("/guilds/{guild_id}/channels")
async def guild_channels(guild_id: str, request: Request, bot: dict = Depends(require_bot)) -> list:
    world: World = get_world(request)
    await _require_guild_member(world, bot["id"], guild_id)
    return [
        {
            "id": c["id"],
            "guild_id": c["guild_id"],
            "name": c["name"],
            "type": c["type"],
            "position": c["position"],
            "owner_id": None,
        }
        for c in await world.list_channels(guild_id)
    ]


@router.get("/guilds/{guild_id}/members")
async def guild_members(
    guild_id: str, request: Request, bot: dict = Depends(require_bot), limit: int = 100, after: str | None = None
) -> list:
    world: World = get_world(request)
    await _require_guild_member(world, bot["id"], guild_id)
    return [
        {
            "user": {"id": m["user_id"], "username": m["nickname"], "bot": m["member_type"] == "bot"},
            "nick": m["nickname"],
            "roles": m["roles"],
            "joined_at": m["joined_at"],
        }
        for m in await world.list_guild_members(guild_id, limit=limit, after=after)
    ]


@router.get("/guilds/{guild_id}/members/{user_id}")
async def guild_member(guild_id: str, user_id: str, request: Request, bot: dict = Depends(require_bot)) -> dict:
    world: World = get_world(request)
    await _require_guild_member(world, bot["id"], guild_id)
    m = await world.get_guild_member(guild_id, user_id)
    return {
        "user": {"id": m["user_id"], "username": m["nickname"], "bot": m["member_type"] == "bot"},
        "nick": m["nickname"],
        "roles": m["roles"],
        "joined_at": m["joined_at"],
    }


@router.get("/guilds/{guild_id}/roles")
async def guild_roles(guild_id: str, request: Request, bot: dict = Depends(require_bot)) -> dict:
    world: World = get_world(request)
    await _require_guild_member(world, bot["id"], guild_id)
    roles = await world.list_guild_roles(guild_id)
    return {
        "roles": [
            {
                "id": role["id"],
                "name": role["name"],
                "color": role["color"],
                "hoist": role["hoist"],
                "permissions": role["permissions"],
            }
            for role in roles
        ],
        "role_members": [],
        "role_members_num": 0,
    }


@router.get("/channels/{channel_id}")
async def channel_get(channel_id: str, request: Request, bot: dict = Depends(require_bot)) -> dict:
    world: World = get_world(request)
    channel = await world.get_channel(channel_id)
    await _require_guild_member(world, bot["id"], channel["guild_id"])
    return {
        "id": channel["id"],
        "guild_id": channel["guild_id"],
        "name": channel["name"],
        "type": channel["type"],
        "position": channel["position"],
        "owner_id": None,
    }


@router.get("/channels/{channel_id}/messages")
async def channel_messages(
    channel_id: str, request: Request, bot: dict = Depends(require_bot), limit: int = 50, before_seq: int | None = None
) -> list:
    world: World = get_world(request)
    channel = await world.get_channel(channel_id)
    await _require_guild_member(world, bot["id"], channel["guild_id"])
    messages = await world.list_messages(f"channel:{channel_id}", limit=limit, before_seq=before_seq)
    return [_channel_message_payload(m, channel) for m in messages]


def _channel_message_payload(message: dict, channel: dict) -> dict:
    return {
        "id": message["msg_id"],
        "content": message["content"],
        "channel_id": channel["id"],
        "guild_id": channel["guild_id"],
        "author": {
            "id": message["sender_id"],
            "username": (message.get("sender") or {}).get("name") or message["sender_id"],
            "bot": message["sender_type"] == "bot",
        },
        "timestamp": message["raw"].get("timestamp", ""),
        "mentions": [],
        "attachments": message.get("attachments") or [],
    }


@router.get("/channels/{channel_id}/messages/{message_id}")
async def channel_message_get(
    channel_id: str, message_id: str, request: Request, bot: dict = Depends(require_bot)
) -> dict:
    world: World = get_world(request)
    channel = await world.get_channel(channel_id)
    await _require_guild_member(world, bot["id"], channel["guild_id"])
    row = await world.db.fetchone(
        "SELECT * FROM messages WHERE msg_id=? AND conversation_id=?", (message_id, f"channel:{channel_id}")
    )
    if row is None:
        from ..errors import MessageNotFound

        raise MessageNotFound("channel message not found")
    message = await world.message_out(row)
    return _channel_message_payload(message, channel)


@router.post("/channels/{channel_id}/messages")
async def channel_send(
    channel_id: str,
    payload: MessageSendRequest,
    request: Request,
    response: Response,
    bot: dict = Depends(require_bot),
) -> dict:
    world: World = get_world(request)
    result = await world.send_channel_message(
        bot["id"], channel_id, payload.model_dump(exclude_none=True)
    )
    return _attach_rate_headers(response, result)


# ---------------------------------------------------------------------------
# webhook helper (sandbox-side echo target for URL verification)
# ---------------------------------------------------------------------------

@router.post("/webhook/{bot_id}")
async def webhook_entry(bot_id: str, request: Request) -> dict:
    from ..domain.webhooks import sign_handshake

    world: World = get_world(request)
    bot = await world.bot_by_id(bot_id)
    body = await request.json()
    data = body.get("d") or body
    plain_token = str(data.get("plain_token") or "")
    event_ts = str(data.get("event_ts") or "")
    if plain_token and event_ts:
        return {"plain_token": plain_token, "signature": sign_handshake(bot["secret"], event_ts, plain_token)}
    return {"op": 12, "d": {}}


# ---------------------------------------------------------------------------
# websocket
# ---------------------------------------------------------------------------

async def websocket_endpoint(websocket: WebSocket) -> None:
    gateway = websocket.app.state.gateway
    await gateway.handle(websocket)


def create_sim_app(world: World, config: ProcessConfig) -> FastAPI:
    app = FastAPI(
        title="QSandbox Simulate API",
        description="QQ Bot API v2 sandbox — not affiliated with Tencent",
        version="0.1.0",
        docs_url="/docs" if config.expose_docs else None,
        redoc_url="/redoc" if config.expose_docs else None,
        openapi_url="/openapi.json" if config.expose_docs else None,
    )
    app.state.world = world
    app.state.config = config
    app.add_middleware(CORSMiddleware, allow_origins=["*"], allow_methods=["*"], allow_headers=["*"])
    app.add_middleware(SimLogMiddleware)
    app.add_middleware(TraceMiddleware)
    install_error_handlers(app)
    app.include_router(router)
    app.add_api_websocket_route("/websocket", websocket_endpoint)
    app.add_api_websocket_route("/websocket/", websocket_endpoint)

    @app.get("/protocol", include_in_schema=False)
    async def protocol_info() -> JSONResponse:
        return JSONResponse({"mqs": 1, "api": "QQ Bot API v2 sandbox", "notice": "non-official sandbox"})

    return app
