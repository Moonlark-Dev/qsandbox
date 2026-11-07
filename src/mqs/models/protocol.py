"""Pydantic protocol models — the single source of truth for the QQ surface."""

from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field


class AppAccessTokenRequest(BaseModel):
    model_config = ConfigDict(extra="allow")

    appId: str | int
    clientSecret: str


class AppAccessTokenResponse(BaseModel):
    access_token: str
    expires_in: str


class ErrorPayload(BaseModel):
    code: int
    message: str
    trace_id: str


class MessageReference(BaseModel):
    model_config = ConfigDict(extra="allow")

    message_id: str
    ignore_get_message_error: bool = False


class MediaInfo(BaseModel):
    model_config = ConfigDict(extra="allow")

    file_uuid: str | None = None
    file_info: str | None = None
    ttl: int = 0


class MessageSendRequest(BaseModel):
    """Body of POST /v2/groups/{group_openid}/messages and /v2/users/{openid}/messages."""

    model_config = ConfigDict(extra="allow")

    content: str | None = None
    msg_type: int = 0
    markdown: dict[str, Any] | None = None
    keyboard: dict[str, Any] | None = None
    ark: dict[str, Any] | None = None
    embed: dict[str, Any] | None = None
    media: MediaInfo | None = None
    message_reference: MessageReference | None = None
    event_id: str | None = None
    msg_id: str | None = None
    msg_seq: int | None = None
    timestamp: int | None = None

    def is_empty(self) -> bool:
        return not any([self.content, self.markdown, self.keyboard, self.ark, self.embed, self.media])


class MessageResponse(BaseModel):
    model_config = ConfigDict(extra="allow")

    id: str
    timestamp: str
    seq: int | None = None


class MediaUploadRequest(BaseModel):
    model_config = ConfigDict(extra="allow")

    file_type: int = 1
    url: str | None = None
    srv_send_msg: bool = False


class MediaUploadResponse(BaseModel):
    file_uuid: str
    file_info: str
    ttl: int = 0


class BotUser(BaseModel):
    id: str
    username: str
    bot: bool = True


class ReadyData(BaseModel):
    version: int = 1
    session_id: str
    shard: list[int] = Field(default_factory=lambda: [0, 1])
    user: BotUser


class IdentifyData(BaseModel):
    model_config = ConfigDict(extra="allow")

    token: str
    intents: int = 0
    shard: list[int] = Field(default_factory=lambda: [0, 1])
    properties: dict[str, Any] | None = None


class ResumeData(BaseModel):
    token: str
    session_id: str
    seq: int = 0


class GatewayInfo(BaseModel):
    url: str
    shards: int = 1
    session_start_limit: dict[str, int] = Field(
        default_factory=lambda: {"total": 1000, "remaining": 999, "reset_after": 86400000, "max_concurrency": 1}
    )


class UserPayload(BaseModel):
    model_config = ConfigDict(extra="allow")

    id: str
    member_openid: str | None = None
    user_openid: str | None = None
    union_openid: str | None = None
    username: str | None = None


class MessagePayload(BaseModel):
    model_config = ConfigDict(extra="allow")

    id: str
    content: str = ""
    group_openid: str | None = None
    group_id: str | None = None
    author: UserPayload
    timestamp: str
    msg_seq: int | None = None
    mentions: list[dict[str, Any]] = Field(default_factory=list)
    attachments: list[dict[str, Any]] = Field(default_factory=list)
    message_reference: dict[str, Any] | None = None


class GroupSummary(BaseModel):
    group_openid: str
    group_id: str
    name: str
    member_count: int
    bot_ids: list[str]


class GatewayFrame(BaseModel):
    """A generic gateway frame (op/s/t/d/id)."""

    model_config = ConfigDict(extra="allow")

    op: int
    s: int | None = None
    t: str | None = None
    id: str | None = None
    d: Any = None


class WebhookAck(BaseModel):
    """Response to the QQ webhook URL-verification handshake."""

    plain_token: str
    signature: str


CoverageLevel = Literal["full", "partial", "stub", "n/a"]
