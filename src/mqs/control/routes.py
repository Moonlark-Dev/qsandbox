"""Sandbox control API: everything the GUI/CLI/MCP needs, and nothing more."""

from __future__ import annotations

import datetime as dt
import json
import secrets
import shutil
import tempfile
import time
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

from fastapi import Depends, Request, WebSocket, WebSocketDisconnect
from fastapi.responses import JSONResponse, Response
from fastapi.routing import APIRouter

from ..api.common import constant_time_equals
from ..clock import parse_duration
from ..config import ProcessConfig
from ..contract import coverage_markdown, routes_table, summary, typescript
from ..domain.world import World
from ..errors import InvalidParam, InvalidToken
from ..models import (
    BotCreate,
    BotPatch,
    ChatSend,
    ClockAdvance,
    ClockSet,
    GroupBotPatch,
    GroupCreate,
    GroupPatch,
    JoinRequestCreate,
    JoinRequestDecision,
    KeyboardClick,
    MemberAdd,
    MemberPatch,
    RandomUsersRequest,
    RuleCreate,
    ScenarioRunRequest,
    SettingsPatch,
    UserCreate,
    UserPatch,
)

router = APIRouter(prefix="/_mqs")


def get_world(request: Request) -> World:
    return request.app.state.world


def get_config(request: Request) -> ProcessConfig:
    return request.app.state.config


def _authorized(request: Request) -> bool:
    config: ProcessConfig = request.app.state.config
    token = request.headers.get("x-mqs-token") or ""
    if not token:
        auth = request.headers.get("authorization", "")
        if auth.lower().startswith("bearer "):
            token = auth[7:]
    if token and constant_time_equals(token, config.control_token):
        return True
    cookie = request.cookies.get("mqs_session")
    if cookie and cookie in request.app.state.sessions:
        return True
    return False


async def require_control(request: Request) -> None:
    if not _authorized(request):
        raise InvalidToken("missing or invalid MQS control token", code=11241)


def _origin_allowed(request: Request) -> bool:
    origin = request.headers.get("origin")
    if not origin:
        return True
    host = urlparse(origin).hostname or ""
    config: ProcessConfig = request.app.state.config
    allowed = {"127.0.0.1", "localhost", "::1", "0.0.0.0", "::"}
    if config.public_host:
        allowed.add(config.public_host)
    if config.dial_host:
        allowed.add(config.dial_host)
    for extra in config.control_origins:
        try:
            allowed.add(urlparse(extra).hostname or extra)
        except ValueError:
            allowed.add(extra)
    return host in allowed


# ---------------------------------------------------------------------------
# unauthenticated endpoints
# ---------------------------------------------------------------------------

public_router = APIRouter(prefix="/_mqs")


@public_router.get("/health")
async def health(request: Request) -> dict:
    return {"ok": True, "profile": request.app.state.config.profile, "mqs": "0.1.0"}


@public_router.post("/session")
async def create_session(request: Request) -> JSONResponse:
    nonce = request.app.state.login_nonce
    if time.monotonic() - request.app.state.login_nonce_created > 3600:
        return JSONResponse({"code": 11241, "message": "login nonce expired"}, status_code=401)
    if not _origin_allowed(request):
        return JSONResponse({"code": 40040, "message": "origin not allowed"}, status_code=403)
    try:
        body = await request.json()
    except ValueError:
        body = {}
    supplied = str(body.get("nonce") or request.headers.get("x-mqs-nonce") or "")
    if not supplied or not constant_time_equals(supplied, nonce):
        return JSONResponse({"code": 11241, "message": "invalid nonce"}, status_code=401)
    session = secrets.token_urlsafe(32)
    request.app.state.sessions.add(session)
    response = JSONResponse({"ok": True})
    response.set_cookie(
        "mqs_session", session, httponly=True, samesite="strict", max_age=43200, path="/"
    )
    return response


@public_router.get("/auth-check")
async def auth_check(request: Request) -> dict:
    if not _authorized(request):
        raise InvalidToken("missing or invalid MQS control token")
    return {"ok": True}


# ---------------------------------------------------------------------------
# status / settings
# ---------------------------------------------------------------------------

@router.get("/status")
async def status(request: Request) -> dict:
    world: World = get_world(request)
    config: ProcessConfig = get_config(request)
    data = await world.status()
    data.update(
        {
            "sim_port": config.sim_port,
            "control_port": config.control_port,
            "host": config.host,
            "base_url": config.base_url,
            "ws_url": config.ws_url,
            "profile_dir": str(world.paths.directory),
            "gui_url": config.gui_url,
        }
    )
    return data


@router.get("/profiles")
async def list_profiles(request: Request) -> list[dict]:
    import httpx

    world: World = get_world(request)
    home = world.paths.home / "profiles"
    items: list[dict] = []
    if home.exists():
        for directory in sorted(home.iterdir()):
            if not directory.is_dir():
                continue
            meta_path = directory / "profile.json"
            meta: dict[str, Any] = {}
            if meta_path.exists():
                try:
                    meta = json.loads(meta_path.read_text(encoding="utf-8"))
                except ValueError:
                    meta = {}
            runtime_path = directory / "runtime.json"
            runtime: dict[str, Any] = {}
            running = False
            if runtime_path.exists():
                try:
                    runtime = json.loads(runtime_path.read_text(encoding="utf-8"))
                    async with httpx.AsyncClient(timeout=1.0) as client:
                        health = await client.get(f"http://127.0.0.1:{runtime['control_port']}/_mqs/health")
                        running = health.status_code == 200
                except (ValueError, KeyError, OSError):
                    running = False
                except httpx.HTTPError:
                    running = False
            items.append(
                {
                    "name": directory.name,
                    "path": str(directory),
                    "running": running,
                    "current": directory.name == world.profile,
                    "sim_port": runtime.get("sim_port"),
                    "control_port": runtime.get("control_port"),
                    "gui_url": runtime.get("gui_url"),
                    "created_at": meta.get("created_at"),
                }
            )
    return items


@router.get("/settings")
async def get_settings(request: Request) -> dict:
    return (await get_world(request).db.all_settings())


@router.patch("/settings")
async def patch_settings(payload: SettingsPatch, request: Request) -> dict:
    patch = payload.model_dump(exclude_none=True)
    world: World = get_world(request)
    await world.update_settings(patch)
    if "seed" in patch:
        await world.db.set_setting("seed", int(patch["seed"]))
    return await world.db.all_settings()


# ---------------------------------------------------------------------------
# users
# ---------------------------------------------------------------------------

@router.get("/users")
async def list_users(request: Request, actor_type: str | None = None, available: bool = False) -> list[dict]:
    return await get_world(request).list_users(actor_type=actor_type, available=available)


@router.post("/users")
async def create_user(payload: UserCreate, request: Request) -> dict:
    world: World = get_world(request)
    return await world.create_user(
        nickname=payload.nickname,
        avatar=payload.avatar,
        gender=payload.gender,
        age=payload.age,
        city=payload.city,
        actor_type=payload.actor_type,
        behavior=payload.behavior,
        seed=payload.seed,
    )


@router.post("/users/random")
async def create_random_users(payload: RandomUsersRequest, request: Request) -> list[dict]:
    world: World = get_world(request)
    return await world.create_random_users(
        payload.count,
        seed=payload.seed,
        behavior=payload.behavior,
        actor_type=payload.actor_type,
        group_id=payload.group_id,
    )


@router.get("/users/{user_id}")
async def get_user(user_id: str, request: Request) -> dict:
    return await get_world(request).get_user(user_id)


@router.patch("/users/{user_id}")
async def patch_user(user_id: str, payload: UserPatch, request: Request) -> dict:
    return await get_world(request).update_user(user_id, payload.model_dump(exclude_none=True))


@router.delete("/users/{user_id}")
async def delete_user(user_id: str, request: Request) -> Response:
    await get_world(request).delete_user(user_id)
    return Response(status_code=204)


# ---------------------------------------------------------------------------
# media upload (user rich messages)
# ---------------------------------------------------------------------------

@router.post("/media")
async def upload_media(request: Request) -> dict:
    world: World = get_world(request)
    form = await request.form()
    upload = form.get("file")
    if upload is None or not hasattr(upload, "read"):
        raise InvalidParam("multipart upload requires a 'file' field")
    data = await upload.read()
    if not data:
        raise InvalidParam("empty file")
    if len(data) > 10 * 1024 * 1024:
        raise InvalidParam("file too large (max 10 MiB)")
    mime = getattr(upload, "content_type", None) or "application/octet-stream"
    if not mime.startswith(("image/", "video/", "audio/")):
        raise InvalidParam("only image/video/audio files are supported")
    media = await world.store_media(data, mime)
    return {
        "hash": media["hash"],
        "url": f"/media/{media['hash']}",
        "content_type": media["mime"],
        "size": media["size"],
        "filename": getattr(upload, "filename", None) or media["hash"][:12],
    }


# ---------------------------------------------------------------------------
# groups
# ---------------------------------------------------------------------------

@router.get("/groups")
async def list_groups(request: Request) -> list[dict]:
    return await get_world(request).list_groups()


@router.post("/groups")
async def create_group(payload: GroupCreate, request: Request) -> dict:
    world: World = get_world(request)
    return await world.create_group(
        name=payload.name,
        owner_id=payload.owner_id,
        member_ids=payload.member_ids,
        bot_ids=payload.bot_ids or None,
        join_policy=payload.join_policy,
        random_members=payload.random_members,
    )


@router.get("/groups/{group_id}")
async def get_group(group_id: str, request: Request) -> dict:
    return await get_world(request).get_group(group_id)


@router.patch("/groups/{group_id}")
async def patch_group(group_id: str, payload: GroupPatch, request: Request) -> dict:
    return await get_world(request).update_group(group_id, payload.model_dump(exclude_none=True))


@router.delete("/groups/{group_id}")
async def delete_group(group_id: str, request: Request) -> Response:
    await get_world(request).delete_group(group_id)
    return Response(status_code=204)


@router.get("/groups/{group_id}/members")
async def list_members(group_id: str, request: Request) -> list[dict]:
    return await get_world(request).list_members(group_id)


@router.post("/groups/{group_id}/members")
async def add_member(group_id: str, payload: MemberAdd, request: Request) -> dict:
    return await get_world(request).add_member(group_id, payload.user_id, payload.role)


@router.patch("/groups/{group_id}/members/{user_id}")
async def patch_member(group_id: str, user_id: str, payload: MemberPatch, request: Request) -> dict:
    return await get_world(request).update_member(group_id, user_id, payload.model_dump(exclude_none=True))


@router.delete("/groups/{group_id}/members/{user_id}")
async def delete_member(group_id: str, user_id: str, request: Request) -> Response:
    await get_world(request).remove_member(group_id, user_id)
    return Response(status_code=204)


@router.post("/groups/{group_id}/bots/{bot_id}")
async def add_bot_to_group(group_id: str, bot_id: str, request: Request) -> dict:
    return await get_world(request).add_bot_to_group(group_id, bot_id)


@router.delete("/groups/{group_id}/bots/{bot_id}")
async def remove_bot_from_group(group_id: str, bot_id: str, request: Request) -> Response:
    await get_world(request).remove_bot_from_group(group_id, bot_id)
    return Response(status_code=204)


@router.get("/groups/{group_id}/info")
async def control_group_info(group_id: str, request: Request, bot_id: str = "") -> dict:
    world: World = get_world(request)
    if not bot_id:
        bot = await world.db.fetchone("SELECT bot_id FROM group_bots WHERE group_id=? LIMIT 1", (group_id,))
        if bot is None:
            raise InvalidParam("no bot in this group")
        bot_id = bot["bot_id"]
    return await world.group_info(group_id, bot_id)


@router.get("/groups/{group_id}/bot-state")
async def control_group_bot_state(group_id: str, request: Request, bot_id: str = "") -> dict:
    world: World = get_world(request)
    if not bot_id:
        bot = await world.db.fetchone("SELECT bot_id FROM group_bots WHERE group_id=? LIMIT 1", (group_id,))
        if bot is None:
            raise InvalidParam("no bot in this group")
        bot_id = bot["bot_id"]
    return await world.group_bot_state(group_id, bot_id)


@router.patch("/groups/{group_id}/bots/{bot_id}")
async def control_group_bot_patch(
    group_id: str, bot_id: str, payload: GroupBotPatch, request: Request
) -> dict:
    return await get_world(request).update_group_bot_settings(
        group_id, bot_id, payload.model_dump(exclude_none=True)
    )


@router.get("/groups/{group_id}/blacklist")
async def control_group_blacklist(group_id: str, request: Request) -> list[dict]:
    world: World = get_world(request)
    rows = await world.db.fetchall(
        "SELECT gb.user_id, gb.banned_at, u.nickname FROM group_blacklist gb "
        "JOIN users u ON u.id=gb.user_id WHERE gb.group_id=? ORDER BY gb.banned_at",
        (group_id,),
    )
    return [dict(r) for r in rows]


@router.post("/groups/{group_id}/blacklist")
async def control_group_blacklist_update(group_id: str, request: Request) -> dict:
    world: World = get_world(request)
    body = await request.json()
    op = str(body.get("op") or "add")
    user_id = str(body.get("user_id") or "")
    if not user_id:
        raise InvalidParam("user_id is required")
    if op == "add":
        member = await world.db.fetchone(
            "SELECT 1 FROM group_members WHERE group_id=? AND user_id=?", (group_id, user_id)
        )
        if member is not None:
            raise InvalidParam("user is still in this group; remove them first")
        await world.db.execute(
            "INSERT INTO group_blacklist(group_id,user_id,banned_at) VALUES(?,?,?) "
            "ON CONFLICT(group_id,user_id) DO NOTHING",
            (group_id, user_id, world.clock.now_ms()),
        )
    else:
        await world.db.execute(
            "DELETE FROM group_blacklist WHERE group_id=? AND user_id=?", (group_id, user_id)
        )
    return {"ok": True}


@router.get("/groups/{group_id}/join-requests")
async def control_join_requests(group_id: str, request: Request) -> list[dict]:
    world: World = get_world(request)
    rows = await world.db.fetchall(
        "SELECT jr.*, u.nickname FROM group_join_requests jr JOIN users u ON u.id=jr.user_id "
        "WHERE jr.group_id=? AND jr.status='pending' ORDER BY jr.apply_at",
        (group_id,),
    )
    return [dict(r) for r in rows]


@router.post("/groups/{group_id}/join-requests")
async def control_create_join_request(
    group_id: str, payload: JoinRequestCreate, request: Request
) -> dict:
    return await get_world(request).create_join_request(
        group_id, payload.user_id,
        verify_message=payload.verify_message, source=payload.source,
    )


@router.post("/groups/{group_id}/join-requests/{request_id}/decide")
async def control_decide_join_request(
    group_id: str, request_id: str, payload: JoinRequestDecision, request: Request
) -> dict:
    world: World = get_world(request)
    row = await world.db.fetchone("SELECT * FROM group_join_requests WHERE id=? AND group_id=?", (request_id, group_id))
    if row is None:
        raise InvalidParam("join request not found")
    bots = await world.db.fetchall("SELECT bot_id FROM group_bots WHERE group_id=? LIMIT 1", (group_id,))
    if not bots:
        raise InvalidParam("no bot in this group")
    bot_id = bots[0]["bot_id"]
    openid = await world.openid(bot_id, "user", row["user_id"])
    await world.approve_join_request(
        group_id, bot_id, openid,
        op=payload.op,
        join_request_id=request_id,
        reject_reason=payload.reject_reason,
        add_to_blacklist=payload.add_to_blacklist,
    )
    return {"ok": True}


# ---------------------------------------------------------------------------
# guilds (L2 debug / test surface)
# -----------------------------------------------------------------------------

@router.get("/guilds")
async def control_list_guilds(request: Request) -> list[dict]:
    return await get_world(request).list_guilds()


@router.post("/guilds")
async def control_create_guild(request: Request) -> dict:
    world: World = get_world(request)
    body = await request.json()
    return await world.create_guild(name=body.get("name"), owner_id=body.get("owner_id"))


@router.get("/guilds/{guild_id}")
async def control_get_guild(guild_id: str, request: Request) -> dict:
    return await get_world(request).get_guild(guild_id)


@router.get("/guilds/{guild_id}/channels")
async def control_guild_channels(guild_id: str, request: Request) -> list[dict]:
    return await get_world(request).list_channels(guild_id)


@router.get("/guilds/{guild_id}/members")
async def control_guild_members(guild_id: str, request: Request) -> list[dict]:
    return await get_world(request).list_guild_members(guild_id)


@router.get("/guilds/{guild_id}/roles")
async def control_guild_roles(guild_id: str, request: Request) -> list[dict]:
    return await get_world(request).list_guild_roles(guild_id)


@router.post("/channels/{channel_id}/messages")
async def control_channel_send(channel_id: str, request: Request) -> dict:
    world: World = get_world(request)
    body = await request.json()
    user_id = body.get("as_user")
    if not user_id:
        raise InvalidParam("as_user is required")
    return await world.send_as_user_to_channel(
        channel_id, user_id, body.get("content") or "",
        mentions=body.get("mentions"), attachments=body.get("attachments") or [],
    )


@router.get("/channels/{channel_id}/messages")
async def control_channel_messages(channel_id: str, request: Request, limit: int = 50) -> dict:
    world: World = get_world(request)
    messages = await world.list_messages(f"channel:{channel_id}", limit=limit)
    return {"items": messages}


async def _control_guild_bot(world: World, guild_id: str) -> str:
    row = await world.db.fetchone(
        "SELECT user_id FROM guild_members WHERE guild_id=? AND member_type='bot' LIMIT 1", (guild_id,)
    )
    if row is None:
        raise InvalidParam("no bot in this guild")
    return row["user_id"]


@router.post("/guilds/{guild_id}/channels")
async def control_channel_create(guild_id: str, request: Request) -> dict:
    world: World = get_world(request)
    return await world.create_channel(guild_id, await _control_guild_bot(world, guild_id), await request.json())


@router.patch("/channels/{channel_id}")
async def control_channel_update(channel_id: str, request: Request) -> dict:
    world: World = get_world(request)
    channel = await world.get_channel(channel_id)
    return await world.update_channel(
        channel_id, await _control_guild_bot(world, channel["guild_id"]), await request.json()
    )


@router.delete("/channels/{channel_id}")
async def control_channel_delete(channel_id: str, request: Request) -> Response:
    world: World = get_world(request)
    channel = await world.get_channel(channel_id)
    await world.delete_channel(channel_id, await _control_guild_bot(world, channel["guild_id"]))
    return Response(status_code=204)


@router.post("/guilds/{guild_id}/roles")
async def control_role_create(guild_id: str, request: Request) -> dict:
    world: World = get_world(request)
    return await world.create_guild_role(guild_id, await _control_guild_bot(world, guild_id), await request.json())


@router.patch("/guilds/{guild_id}/roles/{role_id}")
async def control_role_update(guild_id: str, role_id: str, request: Request) -> dict:
    world: World = get_world(request)
    return await world.update_guild_role(
        guild_id, await _control_guild_bot(world, guild_id), role_id, await request.json()
    )


@router.delete("/guilds/{guild_id}/roles/{role_id}")
async def control_role_delete(guild_id: str, role_id: str, request: Request) -> Response:
    world: World = get_world(request)
    await world.delete_guild_role(guild_id, await _control_guild_bot(world, guild_id), role_id)
    return Response(status_code=204)


@router.put("/guilds/{guild_id}/members/{user_id}/roles/{role_id}")
async def control_member_role_add(guild_id: str, user_id: str, role_id: str, request: Request) -> Response:
    world: World = get_world(request)
    await world.assign_guild_role(guild_id, await _control_guild_bot(world, guild_id), user_id, role_id)
    return Response(status_code=204)


@router.delete("/guilds/{guild_id}/members/{user_id}/roles/{role_id}")
async def control_member_role_remove(
    guild_id: str, user_id: str, role_id: str, request: Request
) -> Response:
    world: World = get_world(request)
    await world.unassign_guild_role(guild_id, await _control_guild_bot(world, guild_id), user_id, role_id)
    return Response(status_code=204)


@router.delete("/guilds/{guild_id}/members/{user_id}")
async def control_guild_member_delete(guild_id: str, user_id: str, request: Request) -> Response:
    world: World = get_world(request)
    try:
        body = await request.json()
    except ValueError:
        body = {}
    await world.remove_guild_member(guild_id, await _control_guild_bot(world, guild_id), user_id, body)
    return Response(status_code=204)


@router.patch("/guilds/{guild_id}/members/{user_id}/mute")
async def control_guild_member_mute(guild_id: str, user_id: str, request: Request) -> Response:
    world: World = get_world(request)
    await world.set_guild_member_mute(
        guild_id, await _control_guild_bot(world, guild_id), user_id, await request.json()
    )
    return Response(status_code=204)


@router.patch("/guilds/{guild_id}/mute")
async def control_guild_mute(guild_id: str, request: Request) -> Response:
    world: World = get_world(request)
    await world.set_guild_mute(guild_id, await _control_guild_bot(world, guild_id), await request.json())
    return Response(status_code=204)


# ---------------------------------------------------------------------------
# channel DMs / friends
# ---------------------------------------------------------------------------

@router.get("/dms")
async def control_list_dms(request: Request) -> list[dict]:
    return await get_world(request).list_dms_sessions()


@router.post("/dms")
async def control_create_dms(request: Request) -> dict:
    world: World = get_world(request)
    body = await request.json()
    bot_id = str(body.get("bot_id") or "")
    user_id = str(body.get("user_id") or "")
    source = str(body.get("source_guild_id") or "")
    if not bot_id or not user_id or not source:
        raise InvalidParam("bot_id, user_id and source_guild_id are required")
    return await world.create_dms(bot_id, user_id, source)


@router.post("/dms/{dms_guild_id}/messages")
async def control_dms_send(dms_guild_id: str, request: Request) -> dict:
    world: World = get_world(request)
    body = await request.json()
    user_id = str(body.get("as_user") or "")
    if not user_id:
        raise InvalidParam("as_user is required")
    return await world.send_as_user_to_dms(
        dms_guild_id, user_id, body.get("content") or "",
        attachments=body.get("attachments") or [],
    )


@router.post("/dms/{dms_guild_id}/bot-messages")
async def control_dms_bot_send(dms_guild_id: str, request: Request) -> dict:
    world: World = get_world(request)
    session = await world.db.fetchone("SELECT * FROM dm_sessions WHERE dms_guild_id=?", (dms_guild_id,))
    if session is None:
        raise InvalidParam("DM session not found")
    body = await request.json()
    return await world.send_dms(session["bot_id"], dms_guild_id, {"content": body.get("content") or ""})


@router.get("/dms/{dms_guild_id}/messages")
async def control_dms_messages(dms_guild_id: str, request: Request, limit: int = 50) -> dict:
    world: World = get_world(request)
    return {"items": await world.list_messages(f"dms:{dms_guild_id}", limit=limit)}


@router.get("/bots/{bot_id}/friends")
async def control_list_friends(bot_id: str, request: Request) -> list[dict]:
    return await get_world(request).list_friends(bot_id)


@router.post("/bots/{bot_id}/friends/{user_id}")
async def control_add_friend(bot_id: str, user_id: str, request: Request) -> dict:
    world: World = get_world(request)
    body = {}
    try:
        body = await request.json()
    except ValueError:
        pass
    return await world.add_friend(
        bot_id, user_id,
        scene=int(body.get("scene") or 1000),
        scene_param=str(body.get("scene_param") or ""),
    )


@router.delete("/bots/{bot_id}/friends/{user_id}")
async def control_remove_friend(bot_id: str, user_id: str, request: Request) -> Response:
    await get_world(request).remove_friend(bot_id, user_id)
    return Response(status_code=204)


# ---------------------------------------------------------------------------
# bots
# ---------------------------------------------------------------------------

@router.get("/bots")
async def list_bots(request: Request) -> list[dict]:
    return await get_world(request).list_bots()


@router.post("/bots")
async def create_bot(payload: BotCreate, request: Request) -> dict:
    world: World = get_world(request)
    return await world.create_bot(
        app_id=payload.app_id,
        secret=payload.secret,
        name=payload.name,
        avatar=payload.avatar,
        scopes=payload.scopes,
        delivery=payload.delivery,
        random=payload.random,
        whitelisted=payload.whitelisted,
    )


@router.get("/bots/{bot_id}")
async def get_bot(bot_id: str, request: Request) -> dict:
    return await get_world(request).get_bot(bot_id)


@router.patch("/bots/{bot_id}")
async def patch_bot(bot_id: str, payload: BotPatch, request: Request) -> dict:
    return await get_world(request).update_bot(bot_id, payload.model_dump(exclude_none=True))


@router.delete("/bots/{bot_id}")
async def delete_bot(bot_id: str, request: Request) -> Response:
    await get_world(request).delete_bot(bot_id)
    return Response(status_code=204)


@router.post("/bots/{bot_id}/rotate-secret")
async def rotate_secret(bot_id: str, request: Request) -> dict:
    return await get_world(request).rotate_secret(bot_id)


@router.post("/bots/{bot_id}/token")
async def issue_bot_token(bot_id: str, request: Request) -> dict:
    world: World = get_world(request)
    bot = await world.bot_by_id(bot_id)
    return await world.issue_token(bot["app_id"], bot["secret"])


@router.get("/bots/{bot_id}/webhook")
async def list_webhooks(bot_id: str, request: Request) -> list[dict]:
    rows = await get_world(request).db.fetchall("SELECT * FROM webhook_endpoints WHERE bot_id=?", (bot_id,))
    return [dict(r) for r in rows]


@router.post("/bots/{bot_id}/webhook")
async def add_webhook(bot_id: str, request: Request) -> dict:
    world: World = get_world(request)
    await world.bot_by_id(bot_id)
    body = await request.json()
    url = body.get("url")
    if not url:
        raise InvalidParam("url is required")
    endpoint_id = f"wh_{secrets.token_hex(6)}"
    await world.db.execute(
        "INSERT INTO webhook_endpoints(id,bot_id,url,secret,enabled) VALUES(?,?,?,?,1)",
        (endpoint_id, bot_id, url, body.get("secret") or f"mqs-wh-{secrets.token_hex(8)}"),
    )
    return {"id": endpoint_id, "url": url, "bot_id": bot_id}


@router.delete("/bots/{bot_id}/webhook/{endpoint_id}")
async def delete_webhook(bot_id: str, endpoint_id: str, request: Request) -> Response:
    await get_world(request).db.execute("DELETE FROM webhook_endpoints WHERE id=? AND bot_id=?", (endpoint_id, bot_id))
    return Response(status_code=204)


# ---------------------------------------------------------------------------
# conversations / chat
# ---------------------------------------------------------------------------

@router.get("/conversations")
async def list_conversations(request: Request) -> list[dict]:
    return await get_world(request).list_conversations()


@router.get("/conversations/{conversation_id}/messages")
async def list_messages(conversation_id: str, request: Request, limit: int = 50, before_seq: int | None = None) -> dict:
    world: World = get_world(request)
    messages = await world.list_messages(conversation_id, limit=limit, before_seq=before_seq)
    return {"items": messages, "next_cursor": messages[0]["seq"] if messages else None}


@router.post("/messages/{message_id}/click")
async def click_keyboard_button(message_id: str, payload: KeyboardClick, request: Request) -> dict:
    return await get_world(request).click_keyboard_button(payload.user_id, message_id, payload.index)


@router.post("/chat/send")
async def chat_send(payload: ChatSend, request: Request) -> dict:
    world: World = get_world(request)
    conversation_id = await world.resolve_conversation(to=payload.to, conversation_id=payload.conversation_id)
    at_bots = payload.at_bots
    if payload.mentions and at_bots is None:
        at_bots = []
    return await world.send_as_user(
        conversation_id,
        payload.as_user,
        payload.content,
        msg_type=payload.msg_type,
        mentions=payload.mentions,
        at_bots=at_bots,
        reply_to=payload.reply_to,
        attachments=payload.attachments,
    )


# ---------------------------------------------------------------------------
# debugger
# ---------------------------------------------------------------------------

@router.get("/debug/frames")
async def debug_frames(
    request: Request, since_id: int = 0, limit: int = 100, bot_id: str | None = None, kind: str | None = None, q: str | None = None
) -> dict:
    frames = await get_world(request).list_frames(since_id=since_id, limit=limit, bot_id=bot_id, kind=kind, q=q)
    return {"items": frames, "next_cursor": frames[-1]["id"] if frames else since_id}


@router.get("/debug/events")
async def debug_events(request: Request, since_seq: int = 0, limit: int = 100, conversation_id: str | None = None) -> dict:
    events = await get_world(request).list_events(since_seq=since_seq, limit=limit, conversation_id=conversation_id)
    return {"items": events, "next_cursor": events[-1]["seq"] if events else since_seq}


# ---------------------------------------------------------------------------
# clock
# ---------------------------------------------------------------------------

@router.post("/clock/advance")
async def clock_advance(payload: ClockAdvance, request: Request) -> dict:
    world: World = get_world(request)
    ms = payload.ms if payload.ms is not None else parse_duration(payload.duration or "")
    world.clock.advance(ms)
    await world.persist_clock()
    return {"clock": world.clock.snapshot(), "advanced_ms": ms}


@router.post("/clock/set")
async def clock_set(payload: ClockSet, request: Request) -> dict:
    world: World = get_world(request)
    if payload.mode:
        world.clock.set_mode(payload.mode)
    if payload.ms is not None:
        world.clock.set_ms(payload.ms)
    elif payload.iso:
        parsed = dt.datetime.fromisoformat(payload.iso)
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=dt.UTC)
        world.clock.set_ms(int(parsed.timestamp() * 1000))
    await world.persist_clock()
    return {"clock": world.clock.snapshot()}


@router.post("/rate/reset")
async def rate_reset(request: Request) -> dict:
    world: World = get_world(request)
    world.rate.reset()
    await world.db.execute("DELETE FROM rate_buckets")
    return {"ok": True, "rate": world.rate.stats()}


# ---------------------------------------------------------------------------
# rules
# ---------------------------------------------------------------------------

@router.get("/rules")
async def list_rules(request: Request) -> list[dict]:
    return await get_world(request).list_rules()


@router.post("/rules")
async def create_rule(payload: RuleCreate, request: Request) -> dict:
    return await get_world(request).create_rule(payload.model_dump())


@router.delete("/rules/{rule_id}")
async def delete_rule(rule_id: str, request: Request) -> Response:
    await get_world(request).delete_rule(rule_id)
    return Response(status_code=204)


# ---------------------------------------------------------------------------
# scenarios
# ---------------------------------------------------------------------------

@router.get("/scenarios")
async def list_scenarios(request: Request) -> list[dict]:
    from ..scenario import builtin_scenarios, list_scenario_files

    world: World = get_world(request)
    return [*builtin_scenarios(), *list_scenario_files(world)]


@router.post("/scenarios/run")
async def run_scenario(payload: ScenarioRunRequest, request: Request) -> dict:
    from ..scenario import run_scenario_payload

    world: World = get_world(request)
    sim_app = getattr(request.app.state, "sim_app", None)
    result = await run_scenario_payload(world, payload.script, payload.path, sim_app=sim_app)
    return result.as_dict()


# ---------------------------------------------------------------------------
# snapshot / contract
# ---------------------------------------------------------------------------

@router.post("/snapshot")
async def snapshot(request: Request) -> Response:
    world: World = get_world(request)
    tmpdir = Path(tempfile.mkdtemp(prefix="mqs-snap-"))
    outdir = Path(tempfile.mkdtemp(prefix="mqs-snap-out-"))
    db_target = tmpdir / "db.sqlite"
    await world.checkpoint_for_snapshot(str(db_target))
    if world.paths.meta.exists():
        shutil.copy2(world.paths.meta, tmpdir / "profile.json")
    if world.paths.media.exists():
        shutil.copytree(world.paths.media, tmpdir / "media", dirs_exist_ok=True)
    archive_base = outdir / f"mqs-{world.profile}-{dt.datetime.now():%Y%m%d-%H%M%S}"
    archive = shutil.make_archive(str(archive_base), "zip", root_dir=tmpdir, base_dir=".")
    data = Path(archive).read_bytes()
    shutil.rmtree(tmpdir, ignore_errors=True)
    shutil.rmtree(outdir, ignore_errors=True)
    return Response(
        content=data,
        media_type="application/zip",
        headers={"Content-Disposition": f'attachment; filename="{Path(archive).name}"'},
    )


@router.get("/contract")
async def contract(request: Request) -> dict:
    return {"summary": summary(), "endpoints": routes_table()}


@router.get("/contract/types.ts")
async def contract_types() -> Response:
    return Response(content=typescript(), media_type="text/plain; charset=utf-8")


@router.get("/contract/coverage.md")
async def contract_coverage() -> Response:
    return Response(content=coverage_markdown(), media_type="text/plain; charset=utf-8")


# ---------------------------------------------------------------------------
# websocket ticket + live feed
# ---------------------------------------------------------------------------

@router.post("/ws-ticket")
async def ws_ticket(request: Request) -> dict:
    ticket = secrets.token_urlsafe(24)
    request.app.state.ws_tickets[ticket] = time.monotonic() + 30
    return {"ticket": ticket, "expires_in": 30}


async def control_websocket(websocket: WebSocket) -> None:
    ticket = websocket.query_params.get("ticket")
    store = websocket.app.state.ws_tickets
    expires = store.pop(ticket, None) if ticket else None
    if not expires or time.monotonic() > expires:
        await websocket.close(code=4003, reason="invalid ticket")
        return
    await websocket.accept()
    queue = websocket.app.state.world.hub.subscribe()
    try:
        await websocket.send_json({"kind": "hello", "profile": websocket.app.state.config.profile})
        while True:
            item = await queue.get()
            await websocket.send_json(item)
    except (WebSocketDisconnect, RuntimeError):
        pass
    finally:
        websocket.app.state.world.hub.unsubscribe(queue)


# ---------------------------------------------------------------------------
# app wiring
# ---------------------------------------------------------------------------

def install_control_routes(app: Any) -> None:
    from fastapi import FastAPI

    assert isinstance(app, FastAPI)
    router.dependencies.append(Depends(require_control))
    app.include_router(public_router)
    app.include_router(router)
