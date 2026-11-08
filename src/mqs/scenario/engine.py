"""Declarative scenario engine: stimulus in, assertions on events/messages out."""

from __future__ import annotations

import asyncio
import json
import re
import time
import xml.sax.saxutils as saxutils
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import httpx
import yaml

from ..clock import parse_duration
from ..domain.world import World
from ..errors import InvalidParam

VAR_RE = re.compile(r"\{\{([a-zA-Z0-9_.]+)\}\}")


class StepFailed(AssertionError):
    pass


@dataclass
class StepResult:
    name: str
    ok: bool
    detail: str = ""
    duration_ms: int = 0

    def as_dict(self) -> dict[str, Any]:
        return {"name": self.name, "ok": self.ok, "detail": self.detail, "duration_ms": self.duration_ms}


@dataclass
class ScenarioResult:
    name: str
    ok: bool
    steps: list[StepResult] = field(default_factory=list)
    duration_ms: int = 0
    error: str = ""

    def as_dict(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "ok": self.ok,
            "duration_ms": self.duration_ms,
            "error": self.error,
            "steps": [s.as_dict() for s in self.steps],
            "junit": self.junit_xml(),
            "tap": self.tap(),
        }

    def junit_xml(self) -> str:
        failures = sum(1 for s in self.steps if not s.ok)
        lines = [
            '<?xml version="1.0" encoding="UTF-8"?>',
            f'<testsuite name="{saxutils.escape(self.name)}" tests="{len(self.steps)}" '
            f'failures="{failures}" errors="0" time="{self.duration_ms / 1000:.3f}">',
        ]
        for step in self.steps:
            lines.append(
                f'  <testcase classname="mqs.scenario" name="{saxutils.escape(step.name)}" '
                f'time="{step.duration_ms / 1000:.3f}">'
            )
            if not step.ok:
                lines.append(f'    <failure message="{saxutils.escape(step.detail[:500])}" />')
            lines.append("  </testcase>")
        lines.append("</testsuite>")
        return "\n".join(lines)

    def tap(self) -> str:
        lines = ["TAP version 13", f"1..{len(self.steps)}"]
        for i, step in enumerate(self.steps, 1):
            if step.ok:
                lines.append(f"ok {i} - {step.name}")
            else:
                lines.append(f"not ok {i} - {step.name}")
                lines.append("  ---")
                lines.append(f"  message: {step.detail[:500]}")
                lines.append("  ...")
        return "\n".join(lines)


class ScenarioRunner:
    def __init__(self, world: World, sim_app: Any = None) -> None:
        self.world = world
        self.sim_app = sim_app
        self.vars: dict[str, str] = {}
        self.bots: dict[str, str] = {}
        self.users: dict[str, str] = {}
        self.groups: dict[str, str] = {}
        self.baseline_event_seq = 0
        self.baseline_message_row = 0
        self.consumed_events: set[int] = set()
        self.consumed_messages: set[int] = set()

    # ------------------------------------------------------------------ entry
    async def run(self, script: str, *, name: str = "scenario") -> ScenarioResult:
        started = time.perf_counter()
        doc = yaml.safe_load(script) or {}
        result = ScenarioResult(name=str(doc.get("name") or name), ok=True)
        try:
            self.baseline_event_seq = await self._max_event_seq()
            self.baseline_message_row = await self._max_message_row()
            await self._setup(doc.get("setup") or {})
            for index, raw_step in enumerate(doc.get("steps") or []):
                label = self._label(index, raw_step)
                step_started = time.perf_counter()
                try:
                    detail = await self._step(raw_step)
                    result.steps.append(
                        StepResult(label, True, detail or "", int((time.perf_counter() - step_started) * 1000))
                    )
                except StepFailed as exc:
                    result.steps.append(
                        StepResult(
                            label, False, str(exc), int((time.perf_counter() - step_started) * 1000)
                        )
                    )
                    result.ok = False
                    result.error = str(exc)
                    break
        except Exception as exc:  # noqa: BLE001 - setup failure
            result.ok = False
            result.error = f"setup failed: {exc}"
            result.steps.append(StepResult("setup", False, str(exc), 0))
        result.duration_ms = int((time.perf_counter() - started) * 1000)
        return result

    @staticmethod
    def _label(index: int, raw: dict[str, Any]) -> str:
        if not isinstance(raw, dict) or not raw:
            return f"step-{index + 1}"
        kind, value = next(iter(raw.items()))
        if isinstance(value, dict):
            hint = value.get("name") or value.get("text") or value.get("type") or value.get("path") or ""
            return f"{kind}: {hint}".strip()
        return f"{kind}: {value}"

    # ------------------------------------------------------------------ setup
    async def _setup(self, setup: dict[str, Any]) -> None:
        if not setup:
            return
        if setup.get("settings"):
            await self.world.update_settings(setup["settings"])
        for spec in setup.get("bots") or []:
            alias, bot = await self._ensure_bot(spec)
            self.bots[alias] = bot["id"]
        for spec in setup.get("users") or []:
            alias = spec.get("alias") or spec.get("name") or f"user{len(self.users) + 1}"
            user = await self.world.create_user(
                nickname=spec.get("name") or None,
                actor_type=spec.get("actor_type", "human"),
                behavior=spec.get("behavior", "normal"),
                seed=spec.get("seed"),
            )
            self.users[alias] = user["id"]
            self.vars[alias] = user["id"]
        for spec in setup.get("groups") or []:
            alias = spec.get("alias") or spec.get("name") or f"group{len(self.groups) + 1}"
            owner_alias = spec.get("owner")
            owner_id = self.users.get(owner_alias, owner_alias) if owner_alias else None
            if owner_id is None:
                users = await self.world.list_users()
                if not users:
                    raise InvalidParam("scenario group needs at least one user")
                owner_id = users[0]["id"]
            member_ids = [self.users.get(m, m) for m in spec.get("members") or [] if m != owner_alias]
            bot_ids = [self.bots.get(b, str(b)) for b in spec.get("bots") or list(self.bots.values())]
            if not bot_ids:
                bots = await self.world.list_bots()
                bot_ids = [b["id"] for b in bots]
            group = await self.world.create_group(
                name=spec.get("name"),
                owner_id=owner_id,
                member_ids=member_ids,
                bot_ids=bot_ids,
            )
            # Per-group receive setting, applied to every bot in the group.  A
            # group defaults to "all" (全量消息), which delivers @ messages as
            # GROUP_MESSAGE_CREATE; SDKs that only know GROUP_AT_MESSAGE_CREATE
            # (botpy 1.2.1) need "only_mention" to ever see a group message.
            recv_msg_setting = spec.get("recv_msg_setting")
            if recv_msg_setting:
                for bid in dict.fromkeys(bot_ids):
                    await self.world.update_group_bot_settings(
                        group["id"], bid, {"recv_msg_setting": recv_msg_setting}
                    )
            self.groups[alias] = group["id"]
            self.vars[alias] = group["id"]
            self.vars[f"{alias}_conv"] = group["conversation_id"]

    async def _ensure_bot(self, spec: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        if isinstance(spec, str):
            spec = {"app_id": spec}
        app_id = str(spec.get("app_id") or spec.get("id") or "10000")
        alias = spec.get("alias") or spec.get("name") or app_id
        existing = await self.world.bot_by_app_id(app_id)
        if existing is None:
            bot = await self.world.create_bot(
                app_id=app_id,
                secret=spec.get("secret") or f"mqs-secret-{app_id}",
                name=spec.get("name") or f"MQS Bot {app_id}",
                scopes=spec.get("scopes") or ["group_message", "c2c_message", "media", "member"],
            )
        else:
            bot = self.world.bot_out(existing)
        return alias, bot

    # ------------------------------------------------------------------ steps
    async def _step(self, raw: dict[str, Any]) -> str:
        if not isinstance(raw, dict) or not raw:
            raise InvalidParam(f"invalid step: {raw!r}")
        kind, value = next(iter(raw.items()))
        value = value or {}
        handlers = {
            "send": self._send,
            "expect_event": self._expect_event,
            "expect_message": self._expect_message,
            "advance": self._advance,
            "wait": self._wait,
            "http": self._http,
        }
        if kind not in handlers:
            raise InvalidParam(f"unknown step type: {kind}")
        return await handlers[kind](value)

    def _subst(self, text: str) -> str:
        def repl(match: re.Match[str]) -> str:
            key = match.group(1)
            if key not in self.vars:
                raise StepFailed(f"unknown variable in template: {key}")
            return str(self.vars[key])

        return VAR_RE.sub(repl, text)

    def _subst_obj(self, value: Any) -> Any:
        if isinstance(value, str):
            return self._subst(value)
        if isinstance(value, list):
            return [self._subst_obj(v) for v in value]
        if isinstance(value, dict):
            return {k: self._subst_obj(v) for k, v in value.items()}
        return value

    async def _send(self, value: dict[str, Any]) -> str:
        user = self.users.get(value.get("as"), value.get("as"))
        if not user:
            raise StepFailed("send.as is required")
        target = value.get("to") or value.get("conversation")
        if not target:
            raise StepFailed("send.to is required")
        target = self._subst(str(target))
        if target in self.groups:
            conversation = f"group:{self.groups[target]}"
        else:
            conversation = await self.world.resolve_conversation(to=target)
        at_bots = value.get("at_bots")
        if at_bots is not None:
            at_bots = [self.bots.get(b, str(b)) for b in at_bots]
        reply_to = self._subst(str(value["reply_to"])) if value.get("reply_to") else None
        result = await self.world.send_as_user(
            conversation,
            user,
            value.get("text") or "",
            mentions=value.get("mentions"),
            at_bots=at_bots,
            reply_to=reply_to,
        )
        message = result["message"]
        self.vars["last_message_id"] = message["msg_id"]
        self.vars["incoming_msg_id"] = message["msg_id"]
        self.vars["last_conversation"] = conversation
        return f"sent {message['msg_id']} to {conversation}"

    async def _expect_event(self, value: dict[str, Any]) -> str:
        event_type = str(value.get("type") or value.get("event") or "").upper()
        if not event_type:
            raise StepFailed("expect_event.type is required")
        conversation = value.get("conversation")
        if conversation in self.groups:
            conversation = f"group:{self.groups[conversation]}"
        if conversation:
            conversation = self._subst(str(conversation))
        contains = value.get("content_contains")
        payload_contains = value.get("payload_contains")
        timeout = parse_duration(str(value.get("timeout") or "3s")) / 1000
        baseline = self.baseline_event_seq
        deadline = time.monotonic() + timeout
        last_error = "no matching event"
        while time.monotonic() < deadline:
            rows = await self.world.db.fetchall(
                "SELECT * FROM events WHERE seq > ? ORDER BY seq", (baseline,)
            )
            for row in rows:
                if row["seq"] in self.consumed_events:
                    continue
                if row["type"] != event_type:
                    continue
                if conversation and row["conversation_id"] != conversation:
                    continue
                payload = json.loads(row["payload_json"] or "{}")
                if contains and contains not in (payload.get("content") or ""):
                    continue
                if payload_contains and payload_contains not in json.dumps(payload, ensure_ascii=False):
                    continue
                self.consumed_events.add(row["seq"])
                self.vars["last_event_id"] = row["event_id"]
                self.vars["last_event_seq"] = str(row["seq"])
                return f"event {event_type} seq={row['seq']}"
            await asyncio.sleep(0.05)
        raise StepFailed(f"timeout after {timeout:.1f}s: {last_error} ({event_type})")

    async def _expect_message(self, value: dict[str, Any]) -> str:
        conversation = value.get("conversation") or value.get("to")
        if conversation in self.groups:
            conversation = f"group:{self.groups[conversation]}"
        if not conversation:
            raise StepFailed("expect_message.conversation is required")
        conversation = self._subst(str(conversation))
        direction = value.get("direction")
        contains = value.get("content_contains")
        timeout = parse_duration(str(value.get("timeout") or "5s")) / 1000
        baseline = self.baseline_message_row
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            sql = "SELECT * FROM messages WHERE id > ? AND conversation_id=?"
            params: list[Any] = [baseline, conversation]
            if direction:
                sql += " AND direction=?"
                params.append(direction)
            rows = await self.world.db.fetchall(sql + " ORDER BY id", params)
            for row in rows:
                if row["id"] in self.consumed_messages:
                    continue
                if contains and contains not in (row["content"] or ""):
                    continue
                self.consumed_messages.add(row["id"])
                self.vars["last_message_id"] = row["msg_id"]
                return f"message {row['msg_id']} ({row['direction']})"
            await asyncio.sleep(0.05)
        raise StepFailed(f"timeout after {timeout:.1f}s: no matching message in {conversation}")

    async def _advance(self, value: Any) -> str:
        ms = parse_duration(str(value))
        self.world.clock.advance(ms)
        await self.world.persist_clock()
        return f"clock advanced by {ms}ms -> {self.world.clock.iso()}"

    async def _wait(self, value: Any) -> str:
        ms = parse_duration(str(value))
        await asyncio.sleep(ms / 1000)
        return f"waited {ms}ms"

    async def _http(self, value: dict[str, Any]) -> str:
        method = str(value.get("method") or "POST").upper()
        bot_ref = value.get("bot") or next(iter(self.bots.values()), "10000")
        bot_id = self.bots.get(str(bot_ref), str(bot_ref))
        bot = await self.world.bot_by_id(bot_id)
        path = self._subst(str(value.get("path") or "/"))
        if "{openid}" in path:
            kind = value.get("target") or "group"
            if kind == "group":
                group_ref = value.get("group") or value.get("conversation")
                gid = self.groups.get(str(group_ref), str(group_ref))
                openid = await self.world.openid(bot_id, "group", gid)
            else:
                user_ref = value.get("user")
                uid = self.users.get(str(user_ref), str(user_ref))
                openid = await self.world.openid(bot_id, "user", uid)
            path = path.replace("{openid}", openid)
        token = await self.world.issue_token(bot["app_id"], bot["secret"])
        headers = {"Authorization": f"QQBot {token['access_token']}"}
        json_body = self._subst_obj(value.get("json"))
        data = self._subst_obj(value.get("form"))
        files = None
        multipart = value.get("multipart")
        if multipart:
            import base64

            files = {}
            spec = multipart.get("file") or {}
            if spec.get("base64"):
                files["file"] = (
                    spec.get("filename") or "file.bin",
                    base64.b64decode(spec["base64"]),
                    spec.get("mime") or "application/octet-stream",
                )
            data = self._subst_obj(multipart.get("fields") or {})
        if self.sim_app is None:
            raise StepFailed("http steps need a running sim app")
        transport = httpx.ASGITransport(app=self.sim_app)
        async with httpx.AsyncClient(transport=transport, base_url="http://mqs.local") as client:
            response = await client.request(
                method, path, json=json_body if files is None else None, data=data,
                files=files, headers=headers, timeout=15,
            )
        expected_status = value.get("expect_status")
        if expected_status is not None and response.status_code != int(expected_status):
            raise StepFailed(
                f"expected HTTP {expected_status}, got {response.status_code}: {response.text[:300]}"
            )
        expected_code = value.get("expect_code")
        payload: Any = None
        try:
            payload = response.json()
        except ValueError:
            payload = None
        if expected_code is not None:
            code = payload.get("code") if isinstance(payload, dict) else None
            if code != int(expected_code):
                raise StepFailed(f"expected code {expected_code}, got {code}: {response.text[:300]}")
        capture = value.get("capture") or {}
        for alias, dotted in capture.items():
            node: Any = payload
            for part in str(dotted).split("."):
                if isinstance(node, dict):
                    node = node.get(part)
                elif isinstance(node, list):
                    node = node[int(part)]
                else:
                    node = None
                    break
            if node is not None:
                self.vars[alias] = str(node)
        if isinstance(payload, dict) and isinstance(payload.get("id"), str):
            self.vars["last_message_id"] = payload["id"]
        return f"{method} {path} -> {response.status_code}"

    # ------------------------------------------------------------------ helpers
    async def _max_event_seq(self) -> int:
        row = await self.world.db.fetchone("SELECT COALESCE(MAX(seq),0) AS m FROM events")
        return int(row["m"]) if row else 0

    async def _max_message_row(self) -> int:
        row = await self.world.db.fetchone("SELECT COALESCE(MAX(id),0) AS m FROM messages")
        return int(row["m"]) if row else 0


async def run_scenario_payload(world: World, script: str | None, path: str | None, sim_app: Any = None) -> ScenarioResult:
    if script:
        text = script
        name = "inline"
    elif path:
        p = Path(path).expanduser()
        if not p.is_absolute():
            candidates = [Path.cwd() / p, Path(__file__).resolve().parents[3] / p]
            p = next((c for c in candidates if c.exists()), p)
        if not p.exists():
            raise InvalidParam(f"scenario not found: {path}")
        text = p.read_text(encoding="utf-8")
        name = p.stem
    else:
        raise InvalidParam("script or path is required")
    runner = ScenarioRunner(world, sim_app=sim_app)
    return await runner.run(text, name=name)
