"""Control-plane FastAPI app (token + origin protected) with the Vue GUI."""

from __future__ import annotations

import secrets
import time
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

from fastapi import FastAPI, Request, Response
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse, HTMLResponse, JSONResponse, RedirectResponse
from fastapi.staticfiles import StaticFiles
from starlette.middleware.base import BaseHTTPMiddleware

from ..api.common import TraceMiddleware, install_error_handlers
from ..config import ProcessConfig
from ..domain.world import World
from .routes import control_websocket, install_control_routes

REPO_ROOT = Path(__file__).resolve().parents[3]
WEB_DIST = REPO_ROOT / "web" / "dist"

FALLBACK_HTML = """<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><title>MQS</title>
<style>body{font-family:system-ui,sans-serif;background:#0f1115;color:#e6e6e6;display:grid;place-items:center;height:100vh;margin:0}
.card{max-width:640px;padding:32px;border:1px solid #2a2f3a;border-radius:16px;background:#171a21;line-height:1.7}
code{background:#0b0d12;padding:2px 6px;border-radius:6px;color:#9ecbff} a{color:#7cc4ff}</style></head>
<body><div class="card"><h1>MQS 控制台</h1>
<p>前端尚未构建。开发模式：</p>
<pre><code>cd web &amp;&amp; pnpm install &amp;&amp; pnpm dev</code></pre>
<p>生产构建：</p>
<pre><code>cd web &amp;&amp; pnpm install &amp;&amp; pnpm build</code></pre>
<p>也可以直接用 CLI：<code>mqs status --json</code></p>
<p><a href="/docs">控制 API 文档</a> · <a href="/_mqs/health">健康检查</a></p>
<p style="color:#8a8f99;font-size:13px">MQS 是非官方沙箱，与腾讯无关联。</p></div></body></html>
"""


class OriginGuardMiddleware(BaseHTTPMiddleware):
    async def dispatch(self, request: Request, call_next: Any):
        path = request.url.path
        if path.startswith("/_mqs") or path == "/":
            origin = request.headers.get("origin")
            if origin:
                host = urlparse(origin).hostname or ""
                config = request.app.state.config
                allowed = {"127.0.0.1", "localhost", "::1", "0.0.0.0", "::"}
                if config.public_host:
                    allowed.add(config.public_host)
                if config.dial_host:
                    allowed.add(config.dial_host)
                for extra in config.control_origins:
                    allowed.add(urlparse(extra).hostname or extra)
                if host not in allowed:
                    return JSONResponse(
                        {"code": 40040, "message": "origin not allowed", "trace_id": ""}, status_code=403
                    )
        return await call_next(request)


def _bind_host(config: ProcessConfig) -> str:
    return "127.0.0.1" if config.host in ("0.0.0.0", "::") else config.host


def create_control_app(world: World, config: ProcessConfig) -> FastAPI:
    app = FastAPI(
        title="MQS Control API",
        version="0.1.0",
        docs_url="/docs",
        redoc_url=None,
        openapi_url="/openapi.json",
    )
    app.state.world = world
    app.state.config = config
    app.state.sessions = set()
    app.state.ws_tickets: dict[str, float] = {}
    app.state.login_nonce = secrets.token_urlsafe(24)
    app.state.login_nonce_created = time.monotonic()
    config.gui_url = f"http://{_bind_host(config)}:{config.control_port}/#t={app.state.login_nonce}"

    app.add_middleware(
        CORSMiddleware,
        allow_origin_regex=r"https?://(127\.0\.0\.1|localhost|\[::1\]|0\.0\.0\.0)(:\d+)?",
        allow_credentials=True,
        allow_methods=["*"],
        allow_headers=["*"],
    )
    app.add_middleware(OriginGuardMiddleware)
    app.add_middleware(TraceMiddleware)
    install_error_handlers(app)
    install_control_routes(app)
    app.add_api_websocket_route("/_mqs/ws", control_websocket)

    @app.get("/avatars/{key}.svg", include_in_schema=False)
    async def avatar(key: str) -> Response:
        import re

        from ..ids import avatar_svg

        if not re.fullmatch(r"[A-Za-z0-9_]{1,64}", key):
            return JSONResponse({"code": 10001, "message": "invalid avatar key"}, status_code=400)
        return Response(
            content=avatar_svg(key),
            media_type="image/svg+xml",
            headers={"Cache-Control": "public, max-age=3600"},
        )

    @app.get("/media/{mhash}", include_in_schema=False)
    async def media(mhash: str) -> Response:
        import re

        world: World = app.state.world
        if not re.fullmatch(r"[0-9a-f]{64}", mhash):
            return JSONResponse({"code": 10001, "message": "invalid media hash"}, status_code=400)
        row = await world.media_by_hash(mhash)
        if row is None:
            return JSONResponse({"code": 10001, "message": "media not found"}, status_code=400)
        path = (world.paths.media / mhash).resolve()
        if not str(path).startswith(str(world.paths.media.resolve())) or not path.exists():
            return JSONResponse({"code": 10001, "message": "media not found"}, status_code=400)
        return FileResponse(path, media_type=row["mime"])

    if WEB_DIST.exists():
        app.mount("/", StaticFiles(directory=str(WEB_DIST), html=True), name="web")
    else:

        @app.get("/", response_class=HTMLResponse, include_in_schema=False)
        async def fallback() -> str:
            return FALLBACK_HTML

        @app.get("/favicon.ico", include_in_schema=False)
        async def favicon() -> RedirectResponse:
            return RedirectResponse("/avatars/mqs.svg")

    return app
