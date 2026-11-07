"""Async SQLite storage: one connection, one writer, WAL, short transactions."""

from __future__ import annotations

import json
import sys
from collections.abc import AsyncIterator, Iterable, Sequence
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any

import aiosqlite

from .config import SCHEMA_VERSION

_PACKAGE_DIR = Path(__file__).resolve().parent


def schema_path(name: str) -> Path:
    """Locate a SQL schema file in the source tree and in the packaged sidecar.

    Source tree (and PyInstaller/Nuitka onefile): the files sit next to this
    module. Nuitka standalone puts data files under ``mqs_data/`` because a
    directory literally named ``mqs`` would collide with the ``mqs`` executable
    in the distribution root.
    """
    candidates = [
        _PACKAGE_DIR / name,
        _PACKAGE_DIR.parent / "mqs_data" / name,
        Path(sys.executable).resolve().parent / "mqs_data" / name,
    ]
    for candidate in candidates:
        if candidate.exists():
            return candidate
    looked = ", ".join(str(candidate) for candidate in candidates)
    raise FileNotFoundError(f"schema file {name} is missing (looked in: {looked})")

DEFAULT_SETTINGS: dict[str, Any] = {
    "clock_mode": "real",
    "clock_now_ms": 0,
    "seed": 42,
    "rate_mode": "certified",
    "require_online": True,
    "feature.whitelist": True,
    "feature.expose": "all",
    "chat.auto_at_bot": False,
    "active_quota_per_day": 1000,
    "channel_active_quota_per_day": 20,
    "channel_active_channel_limit": 2,
    "passive_window_c2c_ms": 60 * 60 * 1000,
    "passive_reply_limit_c2c": 4,
    "passive_window_group_ms": 5 * 60 * 1000,
    "passive_reply_limit_group": 5,
    "active_c2c_require_friend": False,
    "dms_active_user_limit": 2,
    "dms_active_bot_limit": 200,
    "language": "zh-CN",
    "theme": "auto",
    "telemetry": False,
    "autostart": False,
    "group.ensure_operator_admin": True,
    "timezone": "Asia/Shanghai",
    "heartbeat_interval_ms": 41250,
    "log_level": "info",
}


class Database:
    def __init__(self, path: Path) -> None:
        self.path = Path(path)
        self._conn: aiosqlite.Connection | None = None
        self._lock = None

    @property
    def conn(self) -> aiosqlite.Connection:
        if self._conn is None:
            raise RuntimeError("database is not connected")
        return self._conn

    async def connect(self) -> None:
        import asyncio

        self._lock = asyncio.Lock()
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = await aiosqlite.connect(self.path, isolation_level=None)
        self._conn.row_factory = aiosqlite.Row
        await self._conn.execute("PRAGMA journal_mode=WAL")
        await self._conn.execute("PRAGMA busy_timeout=5000")
        await self._conn.execute("PRAGMA synchronous=NORMAL")
        await self._conn.execute("PRAGMA foreign_keys=ON")
        await self._migrate()

    async def close(self) -> None:
        if self._conn is not None:
            await self._conn.close()
            self._conn = None

    async def _migrate(self) -> None:
        cur = await self.conn.execute("PRAGMA user_version")
        row = await cur.fetchone()
        await cur.close()
        version = int(row[0]) if row else 0
        if version >= SCHEMA_VERSION:
            await self._seed_settings()
            return
        if version == 0:
            # schema.sql is the L0+L1+L2 baseline (it already contains v2/v3
            # structures); only the newest delta has to be applied on top.
            await self.conn.executescript(schema_path("schema.sql").read_text(encoding="utf-8"))
            version = 3
        if version == 1:
            migration = schema_path("schema_v2.sql")
            await self.conn.executescript(migration.read_text(encoding="utf-8"))
            version = 2
        if version == 2:
            migration = schema_path("schema_v3.sql")
            await self.conn.executescript(migration.read_text(encoding="utf-8"))
            version = 3
        if version == 3:
            migration = schema_path("schema_v4.sql")
            await self.conn.executescript(migration.read_text(encoding="utf-8"))
            version = 4
        await self.conn.execute(f"PRAGMA user_version={SCHEMA_VERSION}")
        await self._seed_settings()

    async def _seed_settings(self) -> None:
        for key, value in DEFAULT_SETTINGS.items():
            await self.conn.execute(
                "INSERT INTO settings(key, value) VALUES(?, ?) ON CONFLICT(key) DO NOTHING",
                (key, json.dumps(value, ensure_ascii=False)),
            )

    # ------------------------------------------------------------------ helpers
    async def fetchall(self, sql: str, params: Sequence[Any] = ()) -> list[aiosqlite.Row]:
        async with self._lock:  # type: ignore[union-attr]
            cur = await self.conn.execute(sql, params)
            try:
                return list(await cur.fetchall())
            finally:
                await cur.close()

    async def fetchone(self, sql: str, params: Sequence[Any] = ()) -> aiosqlite.Row | None:
        async with self._lock:  # type: ignore[union-attr]
            cur = await self.conn.execute(sql, params)
            try:
                return await cur.fetchone()
            finally:
                await cur.close()

    async def execute(self, sql: str, params: Sequence[Any] = ()) -> int:
        """Execute a single statement and return lastrowid (or rowcount)."""
        async with self._lock:  # type: ignore[union-attr]
            cur = await self.conn.execute(sql, params)
            try:
                await self.conn.commit()
                return cur.lastrowid if cur.lastrowid is not None else cur.rowcount
            finally:
                await cur.close()

    async def executemany(self, sql: str, params: Iterable[Sequence[Any]]) -> None:
        async with self._lock:  # type: ignore[union-attr]
            await self.conn.executemany(sql, list(params))
            await self.conn.commit()

    @asynccontextmanager
    async def tx(self) -> AsyncIterator[aiosqlite.Connection]:
        """Write transaction. Only use ``conn`` inside; do not call db helpers."""
        async with self._lock:  # type: ignore[union-attr]
            await self.conn.execute("BEGIN IMMEDIATE")
            try:
                yield self.conn
            except BaseException:
                await self.conn.rollback()
                raise
            else:
                await self.conn.commit()

    # ------------------------------------------------------------------ settings
    async def get_setting(self, key: str, default: Any = None) -> Any:
        row = await self.fetchone("SELECT value FROM settings WHERE key=?", (key,))
        if row is None:
            return DEFAULT_SETTINGS.get(key, default)
        try:
            return json.loads(row["value"])
        except (TypeError, ValueError):
            return row["value"]

    async def set_setting(self, key: str, value: Any) -> None:
        await self.execute(
            "INSERT INTO settings(key, value) VALUES(?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            (key, json.dumps(value, ensure_ascii=False)),
        )

    async def all_settings(self) -> dict[str, Any]:
        rows = await self.fetchall("SELECT key, value FROM settings")
        out: dict[str, Any] = {}
        for row in rows:
            try:
                out[row["key"]] = json.loads(row["value"])
            except (TypeError, ValueError):
                out[row["key"]] = row["value"]
        return out

    # ------------------------------------------------------------------ maintenance
    async def prune_frames(self, keep: int = 2000) -> None:
        row = await self.fetchone("SELECT MAX(id) AS m FROM frames")
        max_id = int(row["m"] or 0) if row else 0
        if max_id > keep:
            await self.execute("DELETE FROM frames WHERE id <= ?", (max_id - keep,))

    def size_bytes(self) -> int:
        try:
            total = self.path.stat().st_size
            for suffix in ("-wal", "-shm"):
                p = Path(str(self.path) + suffix)
                if p.exists():
                    total += p.stat().st_size
            return total
        except OSError:
            return 0
