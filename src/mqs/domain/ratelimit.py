"""Active-message rate limiting, following QQ's published policy.

Source: bot.q.qq.com → 开发文档 v2 → 消息收发 → 消息收发概述 → 频率与时效规则
(fetched 2026-11-07):

  passive   C2C   valid 60min, 4 replies per msg_id
  passive   group valid 5min,  5 replies per msg_id

  active    C2C   certified    bot 10 qps,  relation 20 qpm, daily 1000/user
  active    C2C   unverified   bot 5 qps & 30 qpm, relation 20 qpm, daily 1000/user
  active    group certified    bot 60 qpm,  relation 20 qpm, daily 1000/group
  active    group unverified   bot 30 qpm,  relation 20 qpm, daily 1000/group

个人认证与企业认证的限额完全一致，因此合并为 ``certified``。三档：
``off`` / ``unverified`` / ``certified``。
"""

from __future__ import annotations

import time
from collections import deque
from dataclasses import dataclass

from ..clock import Clock
from ..errors import RateLimited

DAILY_LIMIT = 1000
RELATION_QPM = 20


@dataclass(frozen=True)
class Window:
    limit: int
    period_ms: int


# scene -> bot-dimension windows
PLANS: dict[str, dict[str, tuple[Window, ...]]] = {
    "off": {},
    "unverified": {
        "c2c": (Window(5, 1_000), Window(30, 60_000)),
        "group": (Window(30, 60_000),),
    },
    "certified": {
        "c2c": (Window(10, 1_000),),
        "group": (Window(60, 60_000),),
    },
}

ALIASES = {
    "strict": "unverified",
    "loose": "certified",
    "personal": "certified",
    "enterprise": "certified",
    "personal_certified": "certified",
    "enterprise_certified": "certified",
    "on": "certified",
    "off": "off",
    "unverified": "unverified",
    "certified": "certified",
}


@dataclass
class RateInfo:
    limit: int
    remaining: int
    reset_after: float
    bucket: str = ""

    def headers(self) -> dict[str, str]:
        if not self.limit:
            return {}
        return {
            "X-RateLimit-Limit": str(self.limit),
            "X-RateLimit-Remaining": str(max(0, self.remaining)),
            "X-RateLimit-Reset": f"{self.reset_after:.2f}",
        }

    def as_dict(self) -> dict:
        return {
            "limit": self.limit,
            "remaining": self.remaining,
            "reset_after": round(self.reset_after, 3),
            "bucket": self.bucket,
        }


class RateLimiter:
    def __init__(self, clock: Clock, mode: str = "certified") -> None:
        self.clock = clock
        self.mode = self._normalize(mode)
        self._windows: dict[str, deque[float]] = {}
        self.rejected = 0
        self.allowed = 0

    @staticmethod
    def _normalize(mode: str) -> str:
        normalized = ALIASES.get((mode or "").lower())
        if normalized is None:
            raise ValueError(f"unknown rate mode: {mode}")
        return normalized

    def set_mode(self, mode: str) -> None:
        self.mode = self._normalize(mode)
        self._windows.clear()

    def reset(self) -> None:
        self._windows.clear()
        self.rejected = 0
        self.allowed = 0

    # ------------------------------------------------------------------ buckets
    def _take(self, key: str, window: Window, now: float) -> tuple[bool, float, int]:
        """Sliding-window quota: at most ``limit`` events in any ``period``.

        Token buckets refill continuously and let a burst slip through at
        window boundaries; QQ documents fixed qps/qpm quotas, so a sliding
        window is the faithful model.
        """
        period = window.period_ms / 1000.0
        stamps = self._windows.setdefault(key, deque())
        cutoff = now - period
        while stamps and stamps[0] <= cutoff:
            stamps.popleft()
        if len(stamps) >= window.limit:
            wait = max(0.0, stamps[0] + period - now)
            return False, wait, 0
        stamps.append(now)
        remaining = window.limit - len(stamps)
        reset_after = max(0.0, stamps[0] + period - now) if stamps else period
        return True, reset_after, remaining

    def check_read(self, bot_id: str, bucket: str, limit: int, period_ms: int) -> RateInfo:
        """Fixed QPM/QPS limits on read endpoints (e.g. group members 60 QPM)."""
        if self.mode == "off":
            return RateInfo(limit=0, remaining=0, reset_after=0.0)
        window = Window(limit, period_ms)
        now = self.clock.now_ms() / 1000.0
        ok, wait, remaining = self._take(f"{bot_id}:read:{bucket}", window, now)
        if not ok:
            self.rejected += 1
            raise RateLimited(wait, limit=limit, remaining=0, bucket=bucket)
        self.allowed += 1
        return RateInfo(limit, remaining, wait, bucket)

    def check_channel_send(self, bot_id: str, channel_id: str) -> RateInfo:
        """Channels: at most 5 messages/second per channel, active or passive."""
        if self.mode == "off":
            return RateInfo(limit=0, remaining=0, reset_after=0.0)
        window = Window(5, 1_000)
        now = self.clock.now_ms() / 1000.0
        ok, wait, remaining = self._take(f"{bot_id}:channel:send:{channel_id}", window, now)
        if not ok:
            self.rejected += 1
            raise RateLimited(wait, limit=window.limit, remaining=0, bucket="channel 5/s")
        self.allowed += 1
        return RateInfo(window.limit, remaining, wait, "channel 5/s")

    def check_active(self, bot_id: str, scene: str, target: str) -> RateInfo:
        """Check *active* (unsolicited) message quotas. Passive replies bypass this."""
        plan = PLANS.get(self.mode, {})
        if self.mode == "off" or not plan:
            return RateInfo(limit=0, remaining=0, reset_after=0.0)
        scene_key = "c2c" if scene in ("c2c", "user", "dm") else "group"
        windows = plan.get(scene_key, ())
        now = self.clock.now_ms() / 1000.0
        worst: RateInfo | None = None
        for window in windows:
            key = f"{bot_id}:{scene_key}:bot:{window.limit}/{window.period_ms}"
            ok, wait, remaining = self._take(key, window, now)
            if not ok:
                self.rejected += 1
                raise RateLimited(
                    wait,
                    limit=window.limit,
                    remaining=0,
                    bucket=f"bot {scene_key} {window.limit}/{window.period_ms}ms",
                )
            info = RateInfo(window.limit, remaining, wait, f"bot {scene_key}")
            if worst is None or info.remaining < worst.remaining:
                worst = info
        relation = Window(RELATION_QPM, 60_000)
        key = f"{bot_id}:{scene_key}:relation:{target}"
        ok, wait, remaining = self._take(key, relation, now)
        if not ok:
            self.rejected += 1
            raise RateLimited(
                wait,
                limit=relation.limit,
                remaining=0,
                bucket=f"relation {scene_key} {target[:8]}",
            )
        info = RateInfo(relation.limit, remaining, wait, f"relation {scene_key}")
        if worst is None or info.remaining < worst.remaining:
            worst = info
        self.allowed += 1
        return worst

    def stats(self) -> dict:
        return {
            "mode": self.mode,
            "buckets": len(self._windows),
            "allowed": self.allowed,
            "rejected": self.rejected,
            "daily_limit": DAILY_LIMIT,
            "relation_qpm": RELATION_QPM,
        }

    def plan_table(self) -> dict:
        return {
            "off": {},
            "unverified": {
                "c2c": {"bot": ["5/s", "30/min"], "relation": ["20/min"], "daily": 1000},
                "group": {"bot": ["30/min"], "relation": ["20/min"], "daily": 1000},
            },
            "certified": {
                "c2c": {"bot": ["10/s"], "relation": ["20/min"], "daily": 1000},
                "group": {"bot": ["60/min"], "relation": ["20/min"], "daily": 1000},
            },
            "note": "个人认证与企业认证限额一致，已合并为 certified",
        }


def route_class(path: str) -> str:
    """Kept for API compatibility; message limits are enforced in the domain layer."""
    if "/messages" in path:
        return "message"
    if "/files" in path or path.startswith("/media"):
        return "media"
    if "/members" in path:
        return "member"
    return "read"


def now_seconds() -> float:
    return time.monotonic()
