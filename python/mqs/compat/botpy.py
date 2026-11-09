"""Point the real botpy SDK at QSandbox.

botpy hardcodes both the REST domain (``botpy.http.Route.DOMAIN``) and the
access-token URL (``https://bots.qq.com/app/getAppAccessToken``), so a
``base_url`` config alone cannot work. This module monkeypatches both.

Usage::

    import mqs.compat.botpy  # noqa: F401  — before creating botpy.Client
    from botpy import Client

    Client(intents=Intents(public_messages=True), is_sandbox=False)

Set ``MQS_BASE_URL`` (default ``http://127.0.0.1:5700``) or call
``mqs.compat.botpy.patch("http://127.0.0.1:1234")`` explicitly.
"""

from __future__ import annotations

import json
import os
import time
from pathlib import Path
from urllib.parse import urlparse


def patch(base_url: str | None = None) -> str:
    base_url = (base_url or os.environ.get("MQS_BASE_URL") or "http://127.0.0.1:5700").rstrip("/")
    parsed = urlparse(base_url)
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        raise ValueError(f"invalid base_url: {base_url}")

    try:
        import botpy.robot as bot_robot
        from botpy import http as bot_http
    except ImportError as exc:  # pragma: no cover
        raise RuntimeError("botpy is not installed; pip install qq-botpy") from exc

    bot_http.Route.SCHEME = parsed.scheme
    bot_http.Route.DOMAIN = parsed.netloc
    bot_http.Route.SANDBOX_DOMAIN = parsed.netloc

    async def update_access_token(self) -> None:  # type: ignore[no-untyped-def]
        import aiohttp

        async with aiohttp.ClientSession() as session:
            async with session.post(
                f"{base_url}/app/getAppAccessToken",
                json={"appId": self.app_id, "clientSecret": self.secret},
                timeout=aiohttp.ClientTimeout(total=20),
            ) as response:
                data = await response.json()
        if "access_token" not in data or "expires_in" not in data:
            raise RuntimeError(f"mqs rejected the bot credentials: {data}")
        self.access_token = data["access_token"]
        self.expires_in = int(data["expires_in"]) + int(time.time())

    bot_robot.Token.update_access_token = update_access_token
    return base_url


def patch_from_runtime(home: str | None = None, profile: str | None = None) -> str:
    """Read ``profiles/<name>/runtime.json`` (written by ``mqs serve``) and patch."""
    home_path = Path(home or os.environ.get("MQS_HOME") or "~/.mqs").expanduser()
    name = profile or os.environ.get("MQS_PROFILE") or "default"
    runtime = home_path / "profiles" / name / "runtime.json"
    try:
        payload = json.loads(runtime.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise RuntimeError(f"no running QSandbox server for profile {name!r} ({runtime} missing)") from exc
    return patch(payload["base_url"])


# importing the module is enough for the default local setup
try:
    patch()
except (RuntimeError, ValueError):  # botpy not installed — the shim keeps working
    pass
