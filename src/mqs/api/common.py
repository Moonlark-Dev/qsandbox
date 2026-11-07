"""Shared FastAPI plumbing: QQ-shaped errors, trace ids, bot auth, rate limits."""

from __future__ import annotations

import json
import secrets
import time
from collections.abc import Callable
from typing import Any

from fastapi import Request, Response
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from starlette.middleware.base import BaseHTTPMiddleware

from ..domain.ratelimit import route_class
from ..errors import (
    InvalidToken,
    MQSException,
    RateLimited,
    current_trace_id,
    new_trace_id,
    set_trace_id,
)


def get_world(request: Request) -> Any:
    return request.app.state.world


def get_config(request: Request) -> Any:
    return request.app.state.config


def _extract_bearer(request: Request) -> str:
    header = request.headers.get("authorization") or ""
    parts = header.split(None, 1)
    if len(parts) == 2 and parts[0].lower() == "qqbot":
        return parts[1].strip()
    if len(parts) == 2 and parts[0].lower() == "bearer":
        return parts[1].strip()
    token = request.headers.get("x-bot-token")
    if token:
        return token
    raise InvalidToken("missing Authorization: QQBot <access_token>")


async def require_bot(request: Request) -> dict[str, Any]:
    world = get_world(request)
    token = _extract_bearer(request)
    bot = await world.authenticate_bot(token)
    request.state.bot = bot
    klass = route_class(request.url.path)
    target = (
        request.path_params.get("group_openid")
        or request.path_params.get("openid")
        or request.path_params.get("channel_id")
        or ""
    )
    info = world.rate.check(bot["id"], klass, str(target))
    request.state.rate_info = info
    return bot


async def optional_bot(request: Request) -> dict[str, Any] | None:
    try:
        return await require_bot(request)
    except MQSException:
        return None


# ---------------------------------------------------------------------------
# error / trace middleware
# ---------------------------------------------------------------------------

def install_error_handlers(app: Any) -> None:
    @app.exception_handler(MQSException)
    async def _mqs_error(request: Request, exc: MQSException) -> JSONResponse:
        headers = {"X-Tps-Trace-ID": current_trace_id()}
        if isinstance(exc, RateLimited):
            headers["Retry-After"] = f"{exc.retry_after:.2f}"
            headers["X-RateLimit-Limit"] = str(exc.limit)
            headers["X-RateLimit-Remaining"] = str(max(0, exc.remaining))
            headers["X-RateLimit-Reset"] = f"{exc.retry_after:.2f}"
        if hasattr(request, "state") and getattr(request.state, "rate_info", None) is not None:
            info = request.state.rate_info
            headers.setdefault("X-RateLimit-Limit", str(info.limit))
            headers.setdefault("X-RateLimit-Remaining", str(info.remaining))
            headers.setdefault("X-RateLimit-Reset", f"{info.reset_after:.2f}")
        return JSONResponse(exc.body(), status_code=exc.status, headers=headers)

    @app.exception_handler(RequestValidationError)
    async def _validation_error(request: Request, exc: RequestValidationError) -> JSONResponse:
        first = exc.errors()[0] if exc.errors() else {}
        loc = ".".join(str(p) for p in first.get("loc", []) if p not in ("body", "query", "path"))
        message = f"invalid parameter: {loc}: {first.get('msg', 'invalid')}" if loc else "invalid parameter"
        body = {"code": 10001, "message": message, "trace_id": current_trace_id()}
        return JSONResponse(body, status_code=400, headers={"X-Tps-Trace-ID": current_trace_id()})

    @app.exception_handler(Exception)
    async def _unhandled(request: Request, exc: Exception) -> JSONResponse:
        import logging

        logging.getLogger("mqs").exception("unhandled error on %s %s", request.method, request.url.path)
        body = {"code": 10002, "message": "internal error", "trace_id": current_trace_id()}
        return JSONResponse(body, status_code=500, headers={"X-Tps-Trace-ID": current_trace_id()})


class TraceMiddleware(BaseHTTPMiddleware):
    async def dispatch(self, request: Request, call_next: Callable) -> Response:
        set_trace_id(request.headers.get("x-tps-trace-id") or new_trace_id())
        response = await call_next(request)
        response.headers.setdefault("X-Tps-Trace-ID", current_trace_id())
        info = getattr(request.state, "rate_info", None)
        if info is not None:
            response.headers.setdefault("X-RateLimit-Limit", str(info.limit))
            response.headers.setdefault("X-RateLimit-Remaining", str(info.remaining))
            response.headers.setdefault("X-RateLimit-Reset", f"{info.reset_after:.2f}")
        return response


class SimLogMiddleware(BaseHTTPMiddleware):
    """Log every simulated HTTP call as a frame, for the debugger page."""

    async def dispatch(self, request: Request, call_next: Callable) -> Response:
        path = request.url.path
        if path.startswith(("/media/", "/avatars/", "/docs", "/openapi", "/redoc")):
            return await call_next(request)
        body_bytes = b""
        if request.method in ("POST", "PUT", "PATCH", "DELETE"):
            try:
                body_bytes = await request.body()
            except Exception:  # noqa: BLE001
                body_bytes = b""
        request_body: Any = None
        if body_bytes:
            try:
                request_body = json.loads(body_bytes)
            except ValueError:
                request_body = body_bytes.decode("utf-8", "replace")[:2000]
        if isinstance(request_body, dict) and "clientSecret" in request_body:
            request_body = {**request_body, "clientSecret": "***"}
        started = time.perf_counter()
        response = await call_next(request)
        elapsed = (time.perf_counter() - started) * 1000
        headers = {
            k: ("***" if k.lower() == "authorization" else v)
            for k, v in request.headers.items()
            if k.lower() in ("authorization", "content-type", "origin", "user-agent", "x-tps-trace-id")
        }
        world = getattr(request.app.state, "world", None)
        if world is not None:
            world.rate_info = None
            info = getattr(request.state, "rate_info", None)
            summary = f"{request.method} {path} -> {response.status_code} ({elapsed:.0f}ms"
            if info is not None:
                summary += f", rl remaining={info.remaining}"
            summary += ")"
            await world.log_frame(
                "in",
                "http",
                summary,
                {
                    "method": request.method,
                    "path": path,
                    "query": dict(request.query_params),
                    "headers": headers,
                    "request": request_body,
                    "status": response.status_code,
                    "duration_ms": round(elapsed, 2),
                    "rate_limit": ({"limit": info.limit, "remaining": info.remaining} if info else None),
                    "trace_id": current_trace_id(),
                },
                bot_id=getattr(getattr(request, "state", None), "bot", {}).get("id") if getattr(request, "state", None) else None,
            )
        return response


def new_nonce() -> str:
    return secrets.token_urlsafe(24)


def constant_time_equals(a: str, b: str) -> bool:
    return secrets.compare_digest(a, b)
