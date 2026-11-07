"""Token-bucket rate limiting, mirroring QQ's per-(bot, route class, target) style."""

from __future__ import annotations

import time
from dataclasses import dataclass

from ..clock import Clock
from ..errors import RateLimited

# class -> (refill tokens per second, burst capacity)
PRESETS: dict[str, dict[str, tuple[float, int]]] = {
    "strict": {
        "message": (0.5, 2),
        "media": (0.1, 1),
        "member": (1.0, 2),
        "read": (5.0, 5),
        "default": (1.0, 3),
    },
    "loose": {
        "message": (10.0, 20),
        "media": (2.0, 4),
        "member": (5.0, 10),
        "read": (50.0, 50),
        "default": (20.0, 30),
    },
    "off": {},
}


@dataclass
class RateInfo:
    limit: int
    remaining: int
    reset_after: float


class RateLimiter:
    def __init__(self, clock: Clock, mode: str = "loose") -> None:
        self.clock = clock
        self.mode = mode
        self._buckets: dict[str, tuple[float, int]] = {}
        self.rejected = 0
        self.allowed = 0

    def set_mode(self, mode: str) -> None:
        if mode not in PRESETS:
            raise ValueError(f"unknown rate mode: {mode}")
        self.mode = mode
        self._buckets.clear()

    def reset(self) -> None:
        self._buckets.clear()
        self.rejected = 0
        self.allowed = 0

    def _key(self, bot_id: str, klass: str, target: str) -> str:
        return f"{self.mode}:{bot_id}:{klass}:{target}"

    def check(self, bot_id: str, klass: str, target: str = "") -> RateInfo:
        preset = PRESETS[self.mode]
        if not preset:  # off
            return RateInfo(limit=0, remaining=0, reset_after=0.0)

        refill, burst = preset.get(klass, preset["default"])
        key = self._key(bot_id, klass, target)
        now = self.clock.now_ms() / 1000.0
        tokens, updated = self._buckets.get(key, (float(burst), now))
        tokens = min(float(burst), tokens + (now - updated) * refill)
        if tokens < 1.0:
            wait = (1.0 - tokens) / refill if refill else 1.0
            self._buckets[key] = (tokens, now)
            self.rejected += 1
            raise RateLimited(wait, limit=burst, remaining=0, bucket=f"{klass}:{target or '*'}")
        tokens -= 1.0
        self._buckets[key] = (tokens, now)
        self.allowed += 1
        reset_after = (burst - tokens) / refill if refill else 0.0
        return RateInfo(limit=burst, remaining=int(tokens), reset_after=reset_after)

    def stats(self) -> dict:
        return {
            "mode": self.mode,
            "buckets": len(self._buckets),
            "allowed": self.allowed,
            "rejected": self.rejected,
        }


def route_class(path: str) -> str:
    """Map an API path to a QQ-like rate-limit bucket class."""
    if "/messages" in path:
        return "message"
    if "/files" in path or path.startswith("/media"):
        return "media"
    if "/members" in path:
        return "member"
    if path.startswith("/gateway"):
        return "default"
    return "read"


def now_seconds() -> float:
    return time.monotonic()
