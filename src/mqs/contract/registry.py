"""The contract registry: one row per simulated QQ endpoint.

This is the single source of truth for coverage, docs and the contract test.
Routes register themselves through :func:`endpoint`.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Literal

CoverageLevel = Literal["full", "partial", "stub", "n/a"]
DOC_DATE = "2026-11-07"


@dataclass(frozen=True)
class Endpoint:
    key: str
    method: str
    path: str
    summary: str
    auth: str = "bot"  # bot | app | none
    scopes: tuple[str, ...] = ()
    rate_class: str = "read"
    errors: tuple[int, ...] = ()
    coverage: CoverageLevel = "full"
    layer: str = "L0"
    tag: str = "general"
    doc_date: str = DOC_DATE
    notes: str = ""
    surface: str = "sim"


REGISTRY: dict[str, Endpoint] = {}


def endpoint(
    key: str,
    *,
    method: str,
    path: str,
    summary: str,
    auth: str = "bot",
    scopes: tuple[str, ...] = (),
    rate_class: str = "read",
    errors: tuple[int, ...] = (),
    coverage: CoverageLevel = "full",
    layer: str = "L0",
    tag: str = "general",
    notes: str = "",
    surface: str = "sim",
) -> Callable:
    def decorator(func: Callable) -> Callable:
        if key in REGISTRY:
            raise RuntimeError(f"duplicate contract endpoint: {key}")
        REGISTRY[key] = Endpoint(
            key=key,
            method=method.upper(),
            path=path,
            summary=summary,
            auth=auth,
            scopes=scopes,
            rate_class=rate_class,
            errors=tuple(sorted(errors)),
            coverage=coverage,
            layer=layer,
            tag=tag,
            notes=notes,
            surface=surface,
        )
        func.__mqs_endpoint__ = key  # type: ignore[attr-defined]
        return func

    return decorator


def define(
    key: str,
    *,
    method: str,
    path: str,
    summary: str,
    auth: str = "bot",
    scopes: tuple[str, ...] = (),
    rate_class: str = "read",
    errors: tuple[int, ...] = (),
    coverage: CoverageLevel = "full",
    layer: str = "L0",
    tag: str = "general",
    notes: str = "",
    surface: str = "sim",
) -> Endpoint:
    """Register a contract row without attaching it to a handler."""
    if key in REGISTRY:
        raise RuntimeError(f"duplicate contract endpoint: {key}")
    entry = Endpoint(
        key=key,
        method=method.upper(),
        path=path,
        summary=summary,
        auth=auth,
        scopes=tuple(scopes),
        rate_class=rate_class,
        errors=tuple(sorted(errors)),
        coverage=coverage,
        layer=layer,
        tag=tag,
        notes=notes,
        surface=surface,
    )
    REGISTRY[key] = entry
    return entry


def sim_endpoints() -> list[Endpoint]:
    return [e for e in REGISTRY.values() if e.surface == "sim"]


@dataclass
class CoverageSummary:
    total: int
    by_level: dict[str, int] = field(default_factory=dict)
    by_layer: dict[str, dict[str, int]] = field(default_factory=dict)

    def as_dict(self) -> dict:
        implemented = self.by_level.get("full", 0) + self.by_level.get("partial", 0)
        return {
            "total": self.total,
            "by_level": self.by_level,
            "by_layer": self.by_layer,
            "ascii": (
                f"{self.total} endpoints — {implemented} implemented, "
                f"{self.by_level.get('stub', 0)} stubs, {self.by_level.get('n/a', 0)} n/a"
            ),
        }


def summary() -> dict:
    endpoints = sim_endpoints()
    levels: dict[str, int] = {}
    layers: dict[str, dict[str, int]] = {}
    for e in endpoints:
        levels[e.coverage] = levels.get(e.coverage, 0) + 1
        layers.setdefault(e.layer, {})
        layers[e.layer][e.coverage] = layers[e.layer].get(e.coverage, 0) + 1
    return CoverageSummary(total=len(endpoints), by_level=levels, by_layer=layers).as_dict()


# ---------------------------------------------------------------------------
# Registry entries. Handlers attach to these keys in src/mqs/api/*.py.
# ---------------------------------------------------------------------------

define(
    "app.access_token",
    method="POST",
    path="/app/getAppAccessToken",
    summary="获取 App Access Token（7200s）",
    auth="app",
    rate_class="default",
    errors=(11243, 11244),
    layer="L0",
    tag="auth",
)
define(
    "gateway.url",
    method="GET",
    path="/gateway",
    summary="获取网关 WebSocket 地址",
    auth="bot",
    errors=(11241, 11242, 11243),
    layer="L0",
    tag="gateway",
)
define(
    "gateway.bot",
    method="GET",
    path="/gateway/bot",
    summary="获取网关地址与分片建议",
    auth="bot",
    errors=(11241, 11242),
    layer="L0",
    tag="gateway",
)
define(
    "bot.me",
    method="GET",
    path="/users/@me",
    summary="获取机器人自身信息",
    auth="bot",
    tag="bot",
)
define(
    "bot.guilds",
    method="GET",
    path="/users/@me/guilds",
    summary="获取机器人所在频道列表",
    auth="bot",
    coverage="full",
    layer="L2",
    tag="guild",
)
define(
    "group.send_message",
    method="POST",
    path="/v2/groups/{group_openid}/messages",
    summary="发送群聊消息（被动 / 主动）",
    auth="bot",
    scopes=("group_message",),
    rate_class="message",
    errors=(10001, 40034, 40040, 40054, 40055, 40056, 40057),
    layer="L0",
    tag="message",
)
define(
    "group.delete_message",
    method="DELETE",
    path="/v2/groups/{group_openid}/messages/{message_id}",
    summary="撤回群聊消息",
    auth="bot",
    scopes=("group_message",),
    rate_class="message",
    errors=(40034, 40040),
    coverage="partial",
    layer="L1",
    tag="message",
)
define(
    "group.upload_file",
    method="POST",
    path="/v2/groups/{group_openid}/files",
    summary="上传群聊富媒体文件",
    auth="bot",
    scopes=("group_message", "media"),
    rate_class="media",
    errors=(10001, 40040),
    layer="L1",
    tag="media",
)
define(
    "c2c.send_message",
    method="POST",
    path="/v2/users/{openid}/messages",
    summary="发送单聊消息（被动 / 主动）",
    auth="bot",
    scopes=("c2c_message",),
    rate_class="message",
    errors=(10001, 40034, 40040, 40054, 40055, 40056, 40057),
    layer="L0",
    tag="message",
)
define(
    "c2c.delete_message",
    method="DELETE",
    path="/v2/users/{openid}/messages/{message_id}",
    summary="撤回单聊消息",
    auth="bot",
    scopes=("c2c_message",),
    rate_class="message",
    errors=(40034, 40040),
    coverage="partial",
    layer="L1",
    tag="message",
)
define(
    "c2c.upload_file",
    method="POST",
    path="/v2/users/{openid}/files",
    summary="上传单聊富媒体文件",
    auth="bot",
    scopes=("c2c_message", "media"),
    rate_class="media",
    errors=(10001, 40040),
    layer="L1",
    tag="media",
)
define(
    "guild.get",
    method="GET",
    path="/guilds/{guild_id}",
    summary="获取频道信息",
    auth="bot",
    coverage="full",
    layer="L2",
    tag="guild",
)
define(
    "guild.channels",
    method="GET",
    path="/guilds/{guild_id}/channels",
    summary="获取子频道列表",
    auth="bot",
    coverage="full",
    layer="L2",
    tag="guild",
)
define(
    "guild.members",
    method="GET",
    path="/guilds/{guild_id}/members",
    summary="获取频道成员列表",
    auth="bot",
    coverage="full",
    layer="L2",
    tag="guild",
)
define(
    "guild.member.get",
    method="GET",
    path="/guilds/{guild_id}/members/{user_id}",
    summary="获取单个频道成员",
    auth="bot",
    coverage="full",
    layer="L2",
    tag="guild",
)
define(
    "guild.roles",
    method="GET",
    path="/guilds/{guild_id}/roles",
    summary="获取频道身份组列表",
    auth="bot",
    coverage="full",
    layer="L2",
    tag="guild",
)
define(
    "channel.get",
    method="GET",
    path="/channels/{channel_id}",
    summary="获取子频道信息",
    auth="bot",
    coverage="full",
    layer="L2",
    tag="guild",
)
define(
    "channel.messages.get",
    method="GET",
    path="/channels/{channel_id}/messages/{message_id}",
    summary="获取频道消息",
    auth="bot",
    coverage="full",
    layer="L2",
    tag="guild",
)
define(
    "channel.messages.list",
    method="GET",
    path="/channels/{channel_id}/messages",
    summary="获取频道消息列表",
    auth="bot",
    coverage="full",
    layer="L2",
    tag="guild",
)
define(
    "channel.send_message",
    method="POST",
    path="/channels/{channel_id}/messages",
    summary="发送频道消息",
    auth="bot",
    coverage="full",
    layer="L2",
    tag="guild",
)
define(
    "dms.create",
    method="POST",
    path="/users/@me/dms",
    summary="创建私信会话（频道私信）",
    auth="bot",
    coverage="stub",
    layer="L2",
    tag="guild",
)
define(
    "webhook.verify",
    method="POST",
    path="/webhook/{bot_id}",
    summary="Webhook 回调入口（含 URL 校验握手）",
    auth="none",
    coverage="partial",
    layer="L3",
    tag="webhook",
    notes="MQS 自建入口，用于模拟 webhook 部署形态",
    surface="sim",
)
define(
    "media.download",
    method="GET",
    path="/media/{hash}",
    summary="沙箱媒体下载（内容哈希寻址）",
    auth="none",
    coverage="full",
    layer="L1",
    tag="media",
    notes="MQS 自建入口，非 QQ 接口",
)
