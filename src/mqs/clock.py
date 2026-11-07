"""Virtual clock: everything time-sensitive must go through here."""

from __future__ import annotations

import time
from datetime import UTC, datetime
from zoneinfo import ZoneInfo

TZ = ZoneInfo("Asia/Shanghai")


class Clock:
    """``real`` follows wall time; ``virtual`` is frozen and only moves on demand."""

    def __init__(self, mode: str = "real", virtual_now_ms: int | None = None) -> None:
        self.mode = mode if mode in ("real", "virtual") else "real"
        self._virtual_now_ms = virtual_now_ms if virtual_now_ms is not None else self._wall_ms()

    @staticmethod
    def _wall_ms() -> int:
        return int(time.time() * 1000)

    def set_mode(self, mode: str) -> None:
        if mode not in ("real", "virtual"):
            raise ValueError(f"unknown clock mode: {mode}")
        if mode == "virtual" and self.mode != "virtual":
            self._virtual_now_ms = self._wall_ms()
        self.mode = mode

    def now_ms(self) -> int:
        return self._virtual_now_ms if self.mode == "virtual" else self._wall_ms()

    def now(self) -> datetime:
        return datetime.fromtimestamp(self.now_ms() / 1000, tz=UTC).astimezone(TZ)

    def iso(self, ms: int | None = None) -> str:
        """QQ-style RFC3339 timestamp, e.g. ``2026-01-01T12:00:00+08:00``."""
        value = self.now_ms() if ms is None else ms
        return datetime.fromtimestamp(value / 1000, tz=UTC).astimezone(TZ).isoformat(timespec="seconds")

    def advance(self, ms: int) -> int:
        if self.mode == "virtual":
            self._virtual_now_ms += ms
        else:
            # advancing a real clock is implemented as a persistent offset
            self._virtual_now_ms += ms
            self.mode = "virtual"
        return self.now_ms()

    def set_ms(self, ms: int) -> int:
        self._virtual_now_ms = ms
        self.mode = "virtual"
        return self.now_ms()

    def snapshot(self) -> dict:
        return {"mode": self.mode, "now_ms": self.now_ms(), "now": self.iso()}

    def restore(self, mode: str, now_ms: int) -> None:
        self.mode = mode if mode in ("real", "virtual") else "real"
        self._virtual_now_ms = int(now_ms)


def human_duration(ms: int) -> str:
    """Human friendly duration used by CLI/scenario logging."""
    sign = "-" if ms < 0 else ""
    ms = abs(ms)
    units = [(86400000, "d"), (3600000, "h"), (60000, "m"), (1000, "s")]
    for size, suffix in units:
        if ms >= size:
            value = ms / size
            return f"{sign}{value:g}{suffix}"
    return f"{sign}{ms}ms"


def parse_duration(text: str) -> int:
    """Parse ``500ms`` / ``6m`` / ``2h`` / ``1d`` into milliseconds."""
    text = text.strip().lower()
    if not text:
        raise ValueError("empty duration")
    if text.isdigit():
        return int(text) * 1000
    if text.endswith("ms"):
        return int(float(text[:-2]))
    suffix = text[-1]
    number = float(text[:-1])
    factors = {"s": 1000, "m": 60_000, "h": 3_600_000, "d": 86_400_000}
    if suffix in factors:
        return int(number * factors[suffix])
    raise ValueError(f"cannot parse duration: {text!r} (use e.g. 500ms, 3s, 6m, 2h)")
