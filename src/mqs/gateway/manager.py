"""QQ gateway protocol: hello/identify/ready/heartbeat/resume, intents and sharding."""

from __future__ import annotations

import asyncio
import json
import time
import zlib
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

from fastapi import WebSocket, WebSocketDisconnect

from ..ids import new_session_id

if TYPE_CHECKING:  # pragma: no cover
    from ..domain.world import World

# op codes (identical to the real gateway)
OP_DISPATCH = 0
OP_HEARTBEAT = 1
OP_IDENTIFY = 2
OP_RESUME = 6
OP_RECONNECT = 7
OP_INVALID_SESSION = 9
OP_HELLO = 10
OP_HEARTBEAT_ACK = 11
OP_HTTP_CALLBACK_ACK = 12

# event -> required intent bit
INTENT_BY_EVENT: dict[str, int] = {
    "GROUP_AT_MESSAGE_CREATE": 1 << 25,
    "C2C_MESSAGE_CREATE": 1 << 25,
    "GROUP_ADD_ROBOT": 1 << 25,
    "GROUP_DEL_ROBOT": 1 << 25,
    "GROUP_MSG_REJECT": 1 << 25,
    "GROUP_MSG_RECEIVE": 1 << 25,
    "FRIEND_ADD": 1 << 25,
    "FRIEND_DEL": 1 << 25,
    "AT_MESSAGE_CREATE": 1 << 30,
    "PUBLIC_MESSAGE_DELETE": 1 << 30,
    "MESSAGE_CREATE": 1 << 9,
    "MESSAGE_DELETE": 1 << 9,
    "DIRECT_MESSAGE_CREATE": 1 << 12,
    "DIRECT_MESSAGE_DELETE": 1 << 12,
    "GUILD_CREATE": 1 << 0,
    "GUILD_UPDATE": 1 << 0,
    "GUILD_DELETE": 1 << 0,
    "CHANNEL_CREATE": 1 << 0,
    "CHANNEL_UPDATE": 1 << 0,
    "CHANNEL_DELETE": 1 << 0,
    "GUILD_MEMBER_ADD": 1 << 1,
    "GUILD_MEMBER_UPDATE": 1 << 1,
    "GUILD_MEMBER_REMOVE": 1 << 1,
    "INTERACTION_CREATE": 1 << 26,
}

CLOSE_AUTH_FAILED = 4004
CLOSE_INVALID_SESSION = 4006
CLOSE_SESSION_TIMEOUT = 4009
CLOSE_INVALID_SHARD = 4010
CLOSE_SHARDING_REQUIRED = 4011


@dataclass(eq=False)
class Connection:
    ws: WebSocket
    bot_id: str = ""
    session_id: str = ""
    intents: int = 0
    shard_id: int = 0
    shard_count: int = 1
    last_seq: int = 0
    last_seen: float = field(default_factory=time.monotonic)
    connected_at: float = field(default_factory=time.monotonic)
    identified: bool = False
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)

    def wants(self, event_type: str) -> bool:
        required = INTENT_BY_EVENT.get(event_type)
        if required is None:
            return True
        intents = self.intents or 1
        return bool(intents & required)

    def shard_owns(self, conversation_id: str | None) -> bool:
        if self.shard_count <= 1 or not conversation_id:
            return True
        return zlib.crc32(conversation_id.encode("utf-8")) % self.shard_count == self.shard_id


class GatewayManager:
    def __init__(self, world: World) -> None:
        self.world = world
        self.clock = world.clock
        self.connections: set[Connection] = set()
        self._watchdog: asyncio.Task | None = None
        self._closed = False

    async def start(self) -> None:
        self._watchdog = asyncio.create_task(self._watch())

    async def stop(self) -> None:
        self._closed = True
        if self._watchdog:
            self._watchdog.cancel()
        for conn in list(self.connections):
            try:
                await conn.ws.close(code=1001)
            except Exception:  # noqa: BLE001
                pass
        self.connections.clear()

    # ------------------------------------------------------------------ public API
    def is_online(self, bot_id: str) -> bool:
        return any(c.identified and c.bot_id == bot_id for c in self.connections)

    def online_bots(self) -> list[str]:
        return sorted({c.bot_id for c in self.connections if c.identified})

    def stats(self) -> dict[str, Any]:
        return {
            "connections": len([c for c in self.connections if c.identified]),
            "sessions": len({c.session_id for c in self.connections if c.identified}),
            "bots": self.online_bots(),
            "shards": [
                {
                    "bot_id": c.bot_id,
                    "session_id": c.session_id,
                    "shard": [c.shard_id, c.shard_count],
                    "intents": c.intents,
                    "last_seq": c.last_seq,
                }
                for c in self.connections
                if c.identified
            ],
        }

    async def dispatch(self, bot_id: str, frame: dict[str, Any], conversation_id: str | None = None) -> int:
        delivered = 0
        for conn in list(self.connections):
            if not conn.identified or conn.bot_id != bot_id:
                continue
            if not conn.wants(frame.get("t", "")):
                continue
            if not conn.shard_owns(conversation_id):
                continue
            await self._send(conn, frame)
            conn.last_seq = int(frame.get("s") or conn.last_seq)
            await self.world.db.execute(
                "UPDATE gateway_sessions SET last_seq=? WHERE session_id=?", (conn.last_seq, conn.session_id)
            )
            delivered += 1
        return delivered

    # ------------------------------------------------------------------ socket loop
    async def handle(self, ws: WebSocket) -> None:
        await ws.accept()
        conn = Connection(ws=ws)
        self.connections.add(conn)
        interval = int(self.world.setting("heartbeat_interval_ms", 41250))
        await self._send(conn, {"op": OP_HELLO, "d": {"heartbeat_interval": interval}})
        timeout = interval / 1000 * 2
        try:
            while True:
                payload = await asyncio.wait_for(ws.receive_json(), timeout=timeout)
                conn.last_seen = time.monotonic()
                await self._on_frame(conn, payload)
        except TimeoutError:
            await self._safe_close(conn, CLOSE_SESSION_TIMEOUT, "heartbeat timeout")
        except (WebSocketDisconnect, RuntimeError):
            pass
        except (json.JSONDecodeError, ValueError):
            await self._safe_close(conn, 4002, "invalid json")
        except Exception as exc:  # noqa: BLE001 - never let a socket take the app down
            await self.world.log_frame("in", "ws", f"gateway error: {exc}", {"error": str(exc)}, bot_id=conn.bot_id)
            await self._safe_close(conn, 4000, "internal error")
        finally:
            self.connections.discard(conn)
            if conn.session_id:
                await self.world.db.execute(
                    "UPDATE gateway_sessions SET closed_at=? WHERE session_id=?",
                    (self.clock.now_ms(), conn.session_id),
                )
            await self.world.log_frame(
                "internal",
                "ws",
                f"gateway disconnected (bot={conn.bot_id or '-'}, session={conn.session_id or '-'})",
                {},
                bot_id=conn.bot_id or None,
                session_id=conn.session_id or None,
            )

    async def _on_frame(self, conn: Connection, payload: Any) -> None:
        if not isinstance(payload, dict):
            raise ValueError("frame must be an object")
        op = payload.get("op")
        await self.world.log_frame(
            "in",
            "ws",
            f"<- op={op} {payload.get('t') or ''}".strip(),
            payload,
            bot_id=conn.bot_id or None,
            session_id=conn.session_id or None,
        )
        if op == OP_HEARTBEAT:
            seq = payload.get("d")
            if isinstance(seq, int) and seq > conn.last_seq:
                conn.last_seq = seq
                if conn.session_id:
                    await self.world.db.execute(
                        "UPDATE gateway_sessions SET last_seq=? WHERE session_id=?", (seq, conn.session_id)
                    )
            await self._send(conn, {"op": OP_HEARTBEAT_ACK, "d": None})
        elif op == OP_IDENTIFY:
            await self._identify(conn, payload.get("d") or {})
        elif op == OP_RESUME:
            await self._resume(conn, payload.get("d") or {})
        elif op == OP_HTTP_CALLBACK_ACK:
            return
        else:
            await self.world.log_frame(
                "in", "ws", f"<- unsupported op {op}", payload, bot_id=conn.bot_id or None
            )

    async def _identify(self, conn: Connection, data: dict[str, Any]) -> None:
        token = str(data.get("token") or "")
        try:
            bot = await self.world.authenticate_bot(token)
        except Exception as exc:  # noqa: BLE001
            await self.world.log_frame("internal", "ws", f"identify rejected: {exc}", {"error": str(exc)})
            await self._safe_close(conn, CLOSE_AUTH_FAILED, "invalid token")
            raise WebSocketDisconnect(CLOSE_AUTH_FAILED) from None
        shard = data.get("shard") or [0, 1]
        shard_id, shard_count = int(shard[0]), int(shard[1])
        if shard_count < 1 or shard_id < 0 or shard_id >= shard_count:
            await self._safe_close(conn, CLOSE_INVALID_SHARD, "invalid shard")
            raise WebSocketDisconnect(CLOSE_INVALID_SHARD)
        intents = int(data.get("intents") or 0)
        if intents == 0:
            intents = 1  # botpy behaves this way
        conn.bot_id = bot["id"]
        conn.intents = intents
        conn.shard_id = shard_id
        conn.shard_count = shard_count
        conn.session_id = new_session_id()
        conn.identified = True
        session_id = conn.session_id
        await self.world.db.execute(
            "INSERT INTO gateway_sessions(session_id,bot_id,intents,last_seq,shard_id,shard_count,created_at)"
            " VALUES(?,?,?,?,?,?,?)",
            (session_id, bot["id"], intents, 0, shard_id, shard_count, self.clock.now_ms()),
        )
        frame = {
            "op": OP_DISPATCH,
            "s": 0,
            "t": "READY",
            "d": {
                "version": 1,
                "session_id": session_id,
                "shard": [shard_id, shard_count],
                "user": {"id": bot["app_id"], "username": bot["name"], "bot": True},
            },
        }
        await self._send(conn, frame)
        await self.world.log_frame(
            "out", "ws", f"READY bot={bot['id']} shard={shard_id}/{shard_count}", frame,
            bot_id=bot["id"], session_id=session_id,
        )
        await self.world.hub.publish({"kind": "gateway", "event": "ready", "bot_id": bot["id"], "session_id": session_id})

    async def _resume(self, conn: Connection, data: dict[str, Any]) -> None:
        token = str(data.get("token") or "")
        session_id = str(data.get("session_id") or "")
        last_seq = int(data.get("seq") or 0)
        try:
            bot = await self.world.authenticate_bot(token)
        except Exception:  # noqa: BLE001
            await self._safe_close(conn, CLOSE_AUTH_FAILED, "invalid token")
            raise WebSocketDisconnect(CLOSE_AUTH_FAILED) from None
        row = await self.world.db.fetchone("SELECT * FROM gateway_sessions WHERE session_id=?", (session_id,))
        valid = row is not None and row["bot_id"] == bot["id"]
        if valid and int(row["closed_at"]) and self.clock.now_ms() - int(row["closed_at"]) > 5 * 60 * 1000:
            valid = False
        if not valid:
            await self._send(conn, {"op": OP_INVALID_SESSION, "d": False})
            await self.world.log_frame(
                "out", "ws", f"INVALID_SESSION session={session_id[:12]}", {"op": OP_INVALID_SESSION},
                bot_id=bot["id"],
            )
            await asyncio.sleep(0.05)
            await self._safe_close(conn, CLOSE_INVALID_SESSION, "invalid session")
            raise WebSocketDisconnect(CLOSE_INVALID_SESSION)
        conn.bot_id = bot["id"]
        conn.session_id = session_id
        conn.intents = int(row["intents"])
        conn.shard_id = int(row["shard_id"])
        conn.shard_count = int(row["shard_count"])
        conn.last_seq = max(last_seq, int(row["last_seq"]))
        conn.identified = True
        await self.world.db.execute("UPDATE gateway_sessions SET closed_at=0 WHERE session_id=?", (session_id,))
        await self._send(conn, {"op": OP_DISPATCH, "s": conn.last_seq, "t": "RESUMED", "d": ""})
        await self.world.log_frame(
            "out", "ws", f"RESUMED bot={bot['id']} seq={conn.last_seq}", {"session_id": session_id},
            bot_id=bot["id"], session_id=session_id,
        )
        rows = await self.world.db.fetchall(
            "SELECT seq,event_id,type,payload_json FROM events WHERE target_bot_id=? AND seq>? ORDER BY seq LIMIT 1000",
            (bot["id"], conn.last_seq),
        )
        for ev in rows:
            frame = {
                "op": OP_DISPATCH,
                "s": int(ev["seq"]),
                "t": ev["type"],
                "id": ev["event_id"],
                "d": json.loads(ev["payload_json"]),
            }
            await self._send(conn, frame)
            conn.last_seq = int(ev["seq"])
        if rows:
            await self.world.db.execute(
                "UPDATE gateway_sessions SET last_seq=? WHERE session_id=?", (conn.last_seq, session_id)
            )
            await self.world.log_frame(
                "out", "ws", f"resume replay {len(rows)} events", {"count": len(rows)}, bot_id=bot["id"],
                session_id=session_id,
            )
        await self.world.hub.publish({"kind": "gateway", "event": "resumed", "bot_id": bot["id"], "session_id": session_id})

    async def _send(self, conn: Connection, frame: dict[str, Any]) -> None:
        async with conn.lock:
            try:
                await conn.ws.send_json(frame)
            except Exception:  # noqa: BLE001 - closed socket; dispatcher will clean up
                pass

    async def _safe_close(self, conn: Connection, code: int, reason: str) -> None:
        try:
            await conn.ws.close(code=code, reason=reason)
        except Exception:  # noqa: BLE001
            pass

    async def _watch(self) -> None:
        try:
            while not self._closed:
                await asyncio.sleep(2)
                interval = int(self.world.setting("heartbeat_interval_ms", 41250)) / 1000
                now = time.monotonic()
                for conn in list(self.connections):
                    if now - conn.last_seen > interval * 2 + 10:
                        await self.world.log_frame(
                            "internal", "ws", "heartbeat timeout", {}, bot_id=conn.bot_id or None
                        )
                        await self._safe_close(conn, CLOSE_SESSION_TIMEOUT, "heartbeat timeout")
        except asyncio.CancelledError:
            raise
