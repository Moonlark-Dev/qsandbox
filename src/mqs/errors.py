"""QQ-style error envelope, error catalog and rate-limit exceptions."""

from __future__ import annotations

import contextvars
import secrets
from typing import Any

_trace_id: contextvars.ContextVar[str] = contextvars.ContextVar("mqs_trace_id", default="")


def current_trace_id() -> str:
    tid = _trace_id.get()
    if not tid:
        tid = new_trace_id()
        _trace_id.set(tid)
    return tid


def new_trace_id() -> str:
    return secrets.token_hex(8)


def set_trace_id(value: str | None = None) -> str:
    tid = value or new_trace_id()
    _trace_id.set(tid)
    return tid


# code -> (http status, default message)
# NOTE: the codes marked "近似" in PLAN.md §4.4 are best-effort; they are
# centralised here so a real-machine capture only needs to touch this table.
ERRORS: dict[int, tuple[int, str]] = {
    10001: (400, "invalid parameter"),
    10002: (500, "internal error"),
    11241: (401, "invalid token"),
    11242: (401, "token expired"),
    11243: (401, "invalid app id"),
    11244: (401, "invalid app secret"),
    40034: (404, "message not found or msg_id invalid"),
    40040: (403, "no permission to perform this action"),
    404: (404, "endpoint is not exposed by this sandbox"),
    11253: (403, "no interface access permission"),
    40054: (400, "duplicate reply: msg_id + msg_seq already used"),
    40055: (429, "active message quota exhausted"),
    40056: (403, "bot is not online on any gateway"),
    40057: (400, "passive reply window expired or reply count exhausted"),
    40063: (400, "captcha / message audit required"),
    50001: (500, "upstream service error"),
}


class MQSException(Exception):
    """Base class for everything that must surface as a QQ-style error body."""

    code: int = 10002

    def __init__(self, message: str | None = None, *, code: int | None = None, hint: str | None = None) -> None:
        self.code = code or self.code
        self.status, default_message = ERRORS.get(self.code, (500, "internal error"))
        self.message = message or default_message
        self.hint = hint
        super().__init__(self.message)

    def body(self) -> dict[str, Any]:
        body: dict[str, Any] = {"code": self.code, "message": self.message, "trace_id": current_trace_id()}
        if self.hint:
            body["hint"] = self.hint
        return body


class InvalidParam(MQSException):
    code = 10001


class InvalidToken(MQSException):
    code = 11241


class TokenExpired(MQSException):
    code = 11242


class InvalidAppId(MQSException):
    code = 11243


class InvalidSecret(MQSException):
    code = 11244


class MessageNotFound(MQSException):
    code = 40034


class NoPermission(MQSException):
    code = 40040


class NoInterfaceAccess(MQSException):
    code = 11253

    def __init__(self, message: str | None = None) -> None:
        super().__init__(
            message or "应用无接口访问权限",
            code=11253,
            hint="该接口仅白名单机器人可用，请联系平台运营申请权限；或在 QSandbox 里把 Bot 标记为白名单",
        )


class EndpointNotExposed(MQSException):
    code = 404


class DuplicateReply(MQSException):
    code = 40054


class QuotaExhausted(MQSException):
    code = 40055


class BotOffline(MQSException):
    code = 40056


class PassiveWindowExpired(MQSException):
    code = 40057


ERRORS[42900] = (429, "rate limit exceeded")


class RateLimited(MQSException):
    code = 42900

    def __init__(self, retry_after: float, *, limit: int, remaining: int = 0, bucket: str = "") -> None:
        super().__init__(f"rate limit exceeded for {bucket or 'bucket'}, retry after {retry_after:.2f}s", code=42900)
        self.retry_after = retry_after
        self.limit = limit
        self.remaining = remaining
        self.bucket = bucket

    def body(self) -> dict[str, Any]:
        return {
            "code": self.code,
            "message": self.message,
            "trace_id": current_trace_id(),
            "retry_after": round(self.retry_after, 3),
        }

