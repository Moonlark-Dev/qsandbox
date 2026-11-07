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
from .common import SimLogMiddleware, TraceMiddleware, get_world, install_error_handlers, require_bot

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
async def bot_guilds(bot: dict = Depends(require_bot)) -> list:
    return []


@router.post("/users/@me/dms")
async def create_dm(bot: dict = Depends(require_bot)) -> dict:
    return {"guild_id": "mqs-stub-guild", "channel_id": new_msg_id("MQS_DM"), "create_time": 0}


# ---------------------------------------------------------------------------
# group messages
# ---------------------------------------------------------------------------

@router.post("/v2/groups/{group_openid}/messages")
async def send_group_message(
    group_openid: str, payload: MessageSendRequest, request: Request, bot: dict = Depends(require_bot)
) -> dict:
    world: World = get_world(request)
    return await world.send_as_bot(
        bot["id"],
        target_kind="group",
        target_openid=group_openid,
        body=payload.model_dump(exclude_none=True),
    )


@router.delete("/v2/groups/{group_openid}/messages/{message_id}")
async def delete_group_message(
    group_openid: str, message_id: str, request: Request, bot: dict = Depends(require_bot)
) -> Response:
    world: World = get_world(request)
    await world.recall_message(bot["id"], "group", group_openid, message_id)
    return Response(status_code=204)


@router.post("/v2/users/{openid}/messages")
async def send_c2c_message(openid: str, payload: MessageSendRequest, request: Request, bot: dict = Depends(require_bot)) -> dict:
    world: World = get_world(request)
    return await world.send_as_bot(
        bot["id"],
        target_kind="user",
        target_openid=openid,
        body=payload.model_dump(exclude_none=True),
    )


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
        raise InvalidParam("file_type 4 (generic file) is not supported in MQS v0.1")
    if file_type not in FILE_LIMITS:
        raise InvalidParam(f"invalid file_type: {file_type}")
    return FILE_LIMITS[file_type]


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
        if not req.url:
            raise InvalidParam("url is required for JSON upload")
        media = await _resolve_media_url(request, req.url, file_type)
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


# ---------------------------------------------------------------------------
# avatars (generated locally, no external assets)
# ---------------------------------------------------------------------------

@router.get("/avatars/{key}.svg")
async def avatar(key: str) -> Response:
    if not AVATAR_RE.match(key):
        raise InvalidParam("invalid avatar key")
    return Response(content=avatar_svg(key), media_type="image/svg+xml", headers={"Cache-Control": "public, max-age=3600"})


# ---------------------------------------------------------------------------
# stub guild family (L2) — structure only
# ---------------------------------------------------------------------------

@router.get("/guilds/{guild_id}")
async def stub_guild(guild_id: str, bot: dict = Depends(require_bot)) -> dict:
    return {"id": guild_id, "name": "MQS Stub Guild", "icon": "", "owner_id": "1", "member_count": 0, "description": ""}


@router.get("/guilds/{guild_id}/channels")
async def stub_channels(guild_id: str, bot: dict = Depends(require_bot)) -> list:
    return []


@router.get("/guilds/{guild_id}/members")
async def stub_members(guild_id: str, bot: dict = Depends(require_bot)) -> list:
    return []


@router.get("/guilds/{guild_id}/members/{user_id}")
async def stub_member(guild_id: str, user_id: str, bot: dict = Depends(require_bot)) -> dict:
    return {"user": {"id": user_id, "username": "stub", "bot": False}, "roles": [], "joined_at": "2026-01-01T00:00:00+08:00"}


@router.get("/guilds/{guild_id}/roles")
async def stub_roles(guild_id: str, bot: dict = Depends(require_bot)) -> dict:
    return {"roles": [], "role_members": [], "role_members_num": 0}


@router.get("/channels/{channel_id}")
async def stub_channel(channel_id: str, bot: dict = Depends(require_bot)) -> dict:
    return {"id": channel_id, "name": "stub", "type": 0, "guild_id": "mqs-stub-guild", "position": 0}


@router.get("/channels/{channel_id}/messages")
async def stub_channel_messages(channel_id: str, bot: dict = Depends(require_bot)) -> list:
    return []


@router.get("/channels/{channel_id}/messages/{message_id}")
async def stub_channel_message(channel_id: str, message_id: str, bot: dict = Depends(require_bot)) -> dict:
    return {"id": message_id, "channel_id": channel_id, "content": "", "timestamp": "2026-01-01T00:00:00+08:00"}


@router.post("/channels/{channel_id}/messages")
async def stub_channel_send(channel_id: str, request: Request, bot: dict = Depends(require_bot)) -> dict:
    return {"id": new_msg_id(), "channel_id": channel_id, "timestamp": "2026-01-01T00:00:00+08:00"}


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
        title="MQS Simulate API",
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
