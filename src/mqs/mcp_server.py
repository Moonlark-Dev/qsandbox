"""MCP server exposing the sandbox to agents (stdio).

Every tool is a thin wrapper over the control API. If MQS is not running,
tools return a structured error instead of crashing the MCP session.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

import httpx
from mcp.server.mcpserver import MCPServer

from . import __version__
from .config import DEFAULT_PROFILE, read_runtime, resolve_home

server = MCPServer(
    name="mqs",
    version=__version__,
    instructions=(
        "MQS is a local QQ Bot API v2 sandbox. Use send_as_user to drive conversations, "
        "expect_event to assert what the bot received, and inspect_gateway to debug frames."
    ),
)


def _runtime() -> tuple[str, dict[str, Any]]:
    home = resolve_home(os.environ.get("MQS_HOME") or None)
    profile = os.environ.get("MQS_PROFILE") or DEFAULT_PROFILE
    try:
        runtime = read_runtime(home, profile)
        return f"http://127.0.0.1:{runtime['control_port']}", runtime
    except RuntimeError as exc:
        candidates = sorted(
            (home / "profiles").glob("*/runtime.json"), key=lambda p: p.stat().st_mtime, reverse=True
        )
        if not candidates:
            raise RuntimeError("no running MQS server found — start one with `mqs serve`") from exc
        runtime = json.loads(candidates[0].read_text(encoding="utf-8"))
        return f"http://127.0.0.1:{runtime['control_port']}", runtime


async def _call(method: str, path: str, *, json_body: Any = None, params: dict | None = None, timeout: float = 30) -> Any:
    try:
        base, runtime = _runtime()
    except RuntimeError as exc:
        return {"error": str(exc), "hint": "mqs serve --profile <name>"}
    try:
        async with httpx.AsyncClient(timeout=timeout) as client:
            response = await client.request(
                method,
                base + path,
                headers={"X-MQS-Token": runtime["token"]},
                json=json_body,
                params=params,
            )
    except httpx.HTTPError as exc:
        return {"error": f"cannot reach MQS: {exc}"}
    if response.status_code >= 400:
        try:
            return {"error": response.json()}
        except ValueError:
            return {"error": response.text[:500]}
    if response.status_code == 204:
        return {"ok": True}
    return response.json()


@server.tool(description="MQS 运行状态：端口、世界计数、网关连接、覆盖率")
async def get_status() -> Any:
    return await _call("GET", "/_mqs/status")


@server.tool(description="列出会话（群聊与私聊）")
async def list_conversations() -> Any:
    return await _call("GET", "/_mqs/conversations")


@server.tool(description="读取会话最近消息")
async def read_messages(conversation_id: str, limit: int = 20) -> Any:
    return await _call("GET", f"/_mqs/conversations/{conversation_id}/messages", params={"limit": limit})


@server.tool(description="以虚拟用户身份发送消息。to 形如 group:g_000001 或 dm:10000:u_000001")
async def send_as_user(
    as_user: str, to: str, text: str, at_bots: list[str] | None = None, no_at: bool = False
) -> Any:
    body: dict[str, Any] = {"as_user": as_user, "to": to, "content": text}
    if no_at:
        body["at_bots"] = []
    elif at_bots:
        body["at_bots"] = at_bots
    return await _call("POST", "/_mqs/chat/send", json_body=body)


@server.tool(description="等待事件（type 大小写不敏感，timeout 如 3s）")
async def expect_event(event_type: str = "", timeout: str = "3s", content_contains: str = "") -> Any:
    import time as _time

    from .clock import parse_duration

    baseline_data = await _call("GET", "/_mqs/status")
    if isinstance(baseline_data, dict) and baseline_data.get("error"):
        return baseline_data
    baseline = int(baseline_data.get("max_event_seq", 0))
    deadline = _time.monotonic() + parse_duration(timeout) / 1000
    needle = event_type.upper()
    while _time.monotonic() < deadline:
        data = await _call("GET", "/_mqs/debug/events", params={"since_seq": baseline, "limit": 200})
        if isinstance(data, dict) and data.get("error"):
            return data
        for event in data.get("items", []):
            baseline = max(baseline, int(event["seq"]))
            if needle and event["type"] != needle:
                continue
            if content_contains and content_contains not in json.dumps(event.get("payload"), ensure_ascii=False):
                continue
            return {"matched": True, "event": event}
        await _sleep(0.1)
    return {"matched": False, "error": f"timeout after {timeout}"}


async def _sleep(seconds: float) -> None:
    import asyncio

    await asyncio.sleep(seconds)


@server.tool(description="查看网关连接与最近 WS 帧")
async def inspect_gateway(limit: int = 20) -> Any:
    status = await _call("GET", "/_mqs/status")
    frames = await _call("GET", "/_mqs/debug/frames", params={"limit": limit, "kind": "ws"})
    return {"gateway": status.get("gateway") if isinstance(status, dict) else None, "frames": frames}


@server.tool(description="快进虚拟时钟，例如 6m / 2h")
async def advance_clock(duration: str) -> Any:
    return await _call("POST", "/_mqs/clock/advance", json_body={"duration": duration})


@server.tool(description="创建随机虚拟用户")
async def create_random_users(count: int = 1, seed: int | None = None, group_id: str | None = None) -> Any:
    return await _call(
        "POST", "/_mqs/users/random", json_body={"count": count, "seed": seed, "group_id": group_id}
    )


@server.tool(description="导出 profile 快照，返回本地 zip 路径")
async def snapshot() -> Any:
    try:
        base, runtime = _runtime()
    except RuntimeError as exc:
        return {"error": str(exc)}
    try:
        async with httpx.AsyncClient(timeout=60) as client:
            response = await client.post(f"{base}/_mqs/snapshot", headers={"X-MQS-Token": runtime["token"]})
    except httpx.HTTPError as exc:
        return {"error": str(exc)}
    if response.status_code != 200:
        return {"error": response.text[:300]}
    target_dir = Path(runtime.get("profile_dir") or ".") / "snapshots"
    target_dir.mkdir(parents=True, exist_ok=True)
    target = target_dir / f"mqs-{int(__import__('time').time())}.mqsprofile"
    target.write_bytes(response.content)
    return {"path": str(target), "bytes": len(response.content)}


@server.tool(description="执行 YAML 场景（内置名称或路径）")
async def run_scenario(name_or_path: str) -> Any:
    from .scenario import get_builtin

    builtin = get_builtin(name_or_path)
    body: dict[str, Any] = {"script": builtin} if builtin else {"path": name_or_path}
    return await _call("POST", "/_mqs/scenarios/run", json_body=body, timeout=120)


def main() -> None:
    server.run("stdio")


if __name__ == "__main__":  # pragma: no cover
    main()
