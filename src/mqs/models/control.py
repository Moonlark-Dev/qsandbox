"""Control-API request/response models."""

from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field


class UserCreate(BaseModel):
    nickname: str | None = None
    avatar: str | None = None
    gender: int = 0
    age: int = 0
    city: str = ""
    actor_type: Literal["human", "npc", "bot"] = "human"
    behavior: Literal["silent", "normal", "chatty", "spammer"] = "normal"
    seed: int | None = None
    random: bool = False


class UserPatch(BaseModel):
    nickname: str | None = None
    avatar: str | None = None
    gender: int | None = None
    age: int | None = None
    city: str | None = None
    behavior: Literal["silent", "normal", "chatty", "spammer"] | None = None
    actor_type: Literal["human", "npc", "bot"] | None = None


class GroupCreate(BaseModel):
    name: str | None = None
    owner_id: str | None = None
    member_ids: list[str] = Field(default_factory=list)
    bot_ids: list[str] = Field(default_factory=list)
    join_policy: str = "open"
    random_members: int = 0
    seed: int | None = None


class GroupPatch(BaseModel):
    name: str | None = None
    owner_id: str | None = None
    join_policy: str | None = None
    finger_memo: str | None = None
    class_text: str | None = None
    tags: list[str] | None = None


class MemberAdd(BaseModel):
    user_id: str
    role: Literal["owner", "admin", "member"] = "member"


class MemberPatch(BaseModel):
    role: Literal["owner", "admin", "member"] | None = None
    mute_seconds: int | None = None


class BotCreate(BaseModel):
    app_id: str | None = None
    secret: str | None = None
    name: str | None = None
    avatar: str | None = None
    scopes: list[str] = Field(default_factory=lambda: ["group_message", "c2c_message", "media"])
    delivery: Literal["ws", "webhook"] = "ws"
    random: bool = False
    whitelisted: bool = False


class BotPatch(BaseModel):
    name: str | None = None
    avatar: str | None = None
    scopes: list[str] | None = None
    delivery: Literal["ws", "webhook"] | None = None
    whitelisted: bool | None = None
    allow_proactive_msg: bool | None = None


class ChatSend(BaseModel):
    """Send a message as a virtual user (the main stimulus API)."""

    as_user: str
    conversation_id: str | None = None
    to: str | None = None  # group:<id> | dm:<bot>:<user>
    content: str = ""
    msg_type: int = 0
    mentions: list[str] = Field(default_factory=list)
    at_bots: list[str] | None = None
    reply_to: str | None = None
    attachments: list[dict[str, Any]] = Field(default_factory=list)


class GroupBotPatch(BaseModel):
    allow_proactive_msg: bool | None = None
    recv_msg_setting: Literal["all", "only_mention", "mention_and_context"] | None = None


class JoinRequestCreate(BaseModel):
    user_id: str
    verify_message: str = ""
    source: Literal["self_apply", "invited"] = "self_apply"
    invited_by: str = ""


class JoinRequestDecision(BaseModel):
    op: Literal["approve", "decline"]
    join_request_id: str = ""
    reject_reason: str = ""
    add_to_blacklist: bool = False


class KeyboardClick(BaseModel):
    """A virtual user clicks one button of an outbound inline keyboard."""

    user_id: str
    index: int = 0


class ClockAdvance(BaseModel):
    duration: str | None = None  # "6m"
    ms: int | None = None


class ClockSet(BaseModel):
    iso: str | None = None
    ms: int | None = None
    mode: Literal["real", "virtual"] | None = None


class SettingsPatch(BaseModel):
    model_config = ConfigDict(extra="allow")

    clock_mode: Literal["real", "virtual"] | None = None
    seed: int | None = None
    rate_mode: Literal["off", "unverified", "certified", "strict", "loose"] | None = None
    require_online: bool | None = None
    feature_whitelist: bool | None = None
    feature_expose: str | None = None
    chat_auto_at_bot: bool | None = None
    active_quota_per_day: int | None = None
    passive_window_c2c_ms: int | None = None
    passive_reply_limit_c2c: int | None = None
    passive_window_group_ms: int | None = None
    passive_reply_limit_group: int | None = None
    theme: Literal["auto", "dark", "light"] | None = None
    telemetry: bool | None = None
    autostart: bool | None = None
    language: str | None = None
    theme: str | None = None
    log_level: str | None = None


class RuleCreate(BaseModel):
    name: str = "rule"
    event_type: str = "group_at_message_create"
    condition: dict[str, Any] = Field(default_factory=dict)
    action: dict[str, Any] = Field(default_factory=dict)
    delay_ms: int = 0
    enabled: bool = True


class ScenarioRunRequest(BaseModel):
    script: str | None = None  # inline YAML
    path: str | None = None  # server-side path
    junit: bool = False


class RandomUsersRequest(BaseModel):
    count: int = 1
    seed: int | None = None
    behavior: Literal["silent", "normal", "chatty", "spammer"] | None = None
    actor_type: Literal["human", "npc"] = "human"
    group_id: str | None = None


class ExpectEvent(BaseModel):
    event_type: str | None = None
    conversation_id: str | None = None
    content_contains: str | None = None
    timeout_ms: int = 3000


class SuccessResponse(BaseModel):
    ok: bool = True
    data: Any = None
