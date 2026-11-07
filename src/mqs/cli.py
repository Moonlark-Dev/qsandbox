"""The `mqs` command line interface.

Every command is a thin client of the control API — there is no second
implementation path (PLAN.md §7).
"""

from __future__ import annotations

import json
import shutil
import socket
import sqlite3
import sys
import time
import zipfile
from pathlib import Path
from typing import Annotated, Any

import httpx
import typer

from .clock import parse_duration
from .config import (
    DEFAULT_PROFILE,
    ProcessConfig,
    ProfilePaths,
    read_runtime,
    resolve_home,
)
from .contract import coverage_markdown, typescript

app = typer.Typer(add_completion=False, no_args_is_help=True, help="QSandbox — QQ Bot API v2 sandbox")
user_app = typer.Typer(no_args_is_help=True, help="虚拟用户")
group_app = typer.Typer(no_args_is_help=True, help="群")
bot_app = typer.Typer(no_args_is_help=True, help="Bot")
clock_app = typer.Typer(no_args_is_help=True, help="虚拟时钟")
script_app = typer.Typer(no_args_is_help=True, help="场景脚本")
contract_app = typer.Typer(no_args_is_help=True, help="契约生成物")
profile_app = typer.Typer(no_args_is_help=True, help="profile 快照")
app.add_typer(user_app, name="user")
app.add_typer(group_app, name="group")
app.add_typer(bot_app, name="bot")
app.add_typer(clock_app, name="clock")
app.add_typer(script_app, name="script")
app.add_typer(contract_app, name="contract")
app.add_typer(profile_app, name="profile")

ProfileOpt = Annotated[str, typer.Option("--profile", envvar="MQS_PROFILE", help="profile 名称")]
HomeOpt = Annotated[str, typer.Option("--home", envvar="MQS_HOME", help="MQS 数据目录")]
JsonOpt = Annotated[bool, typer.Option("--json", help="输出 JSON")]
TimeoutOpt = Annotated[float, typer.Option("--timeout", help="HTTP 超时（秒）")]

EXIT_USAGE = 2
EXIT_CONN = 3
EXIT_LOCK = 4
EXIT_FAIL = 5


class Client:
    def __init__(self, home: str, profile: str, timeout: float = 30.0) -> None:
        self.home = resolve_home(home or None)
        self.profile = profile or DEFAULT_PROFILE
        try:
            runtime = read_runtime(self.home, self.profile)
        except RuntimeError as exc:
            typer.secho(f"error: {exc}", fg=typer.colors.RED, err=True)
            typer.secho("hint: 先运行 `mqs serve`，或用 --profile 指定已运行的 profile", err=True)
            raise typer.Exit(EXIT_CONN) from None
        self.runtime = runtime
        self.base = f"http://127.0.0.1:{runtime['control_port']}"
        self.headers = {"X-MQS-Token": runtime["token"]}
        self.timeout = timeout

    def request(self, method: str, path: str, **kwargs: Any) -> Any:
        try:
            with httpx.Client(timeout=self.timeout) as client:
                response = client.request(method, self.base + path, headers=self.headers, **kwargs)
        except httpx.HTTPError as exc:
            typer.secho(f"error: cannot reach MQS control API at {self.base}: {exc}", fg=typer.colors.RED, err=True)
            raise typer.Exit(EXIT_CONN) from None
        if response.status_code >= 400:
            try:
                body = response.json()
            except ValueError:
                body = {"code": response.status_code, "message": response.text[:300]}
            typer.secho(json.dumps(body, ensure_ascii=False), fg=typer.colors.RED, err=True)
            raise typer.Exit(EXIT_FAIL)
        if response.status_code == 204:
            return None
        return response.json()

    def get(self, path: str, **kwargs: Any) -> Any:
        return self.request("GET", path, **kwargs)

    def post(self, path: str, **kwargs: Any) -> Any:
        return self.request("POST", path, **kwargs)

    def patch(self, path: str, **kwargs: Any) -> Any:
        return self.request("PATCH", path, **kwargs)

    def delete(self, path: str, **kwargs: Any) -> Any:
        return self.request("DELETE", path, **kwargs)


def _emit(data: Any, as_json: bool) -> None:
    if as_json:
        typer.echo(json.dumps(data, ensure_ascii=False, indent=2))
        return
    if isinstance(data, list):
        for item in data:
            typer.echo(json.dumps(item, ensure_ascii=False))
        return
    typer.echo(json.dumps(data, ensure_ascii=False, indent=2))


def _table(headers: list[str], rows: list[list[Any]]) -> None:
    widths = [len(h) for h in headers]
    text_rows = [[("" if c is None else str(c)) for c in row] for row in rows]
    for row in text_rows:
        for i, cell in enumerate(row):
            widths[i] = max(widths[i], len(cell))
    typer.echo("  ".join(h.ljust(widths[i]) for i, h in enumerate(headers)))
    typer.echo("  ".join("-" * widths[i] for i in range(len(headers))))
    for row in text_rows:
        typer.echo("  ".join(cell.ljust(widths[i]) for i, cell in enumerate(row)))


# ---------------------------------------------------------------------------
# serve / status / token
# ---------------------------------------------------------------------------

@app.command()
def serve(
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
    host: Annotated[str, typer.Option("--host", help="监听地址，默认仅本机")] = "127.0.0.1",
    port: Annotated[int, typer.Option("--port", help="模拟 API 端口（0 = 随机）")] = 5700,
    control_port: Annotated[int, typer.Option("--control-port", help="控制 API 端口（0 = 随机）")] = 5701,
    token: Annotated[str, typer.Option("--token", help="控制 API token（0.0.0.0 时必填）")] = "",
    log_level: Annotated[str, typer.Option("--log-level")] = "info",
    no_docs: Annotated[bool, typer.Option("--no-docs", help="关闭 /docs 与 /openapi.json")] = False,
) -> None:
    """启动沙箱（模拟 API + 控制 API + 网关）。"""
    from .config import ProfileLocked, warn
    from .server import run

    if host not in ("127.0.0.1", "localhost", "::1"):
        if not token:
            typer.secho("error: --host 非本机地址时必须显式提供 --token", fg=typer.colors.RED, err=True)
            raise typer.Exit(EXIT_USAGE)
        warn(f"listening on {host}: 任何能访问该地址的人都能连接沙箱（已显式提供 token）")
    cfg = ProcessConfig(
        profile=profile or DEFAULT_PROFILE,
        home=resolve_home(home or None),
        host=host,
        sim_port=port if port else 0,
        control_port=control_port if control_port else 0,
        control_token=token,
        log_level=log_level,
        expose_docs=not no_docs,
    )
    if cfg.sim_port == 0:
        cfg.sim_port = 0
    if cfg.control_port == 0:
        cfg.control_port = 0
    try:
        run(cfg)
    except ProfileLocked as exc:
        typer.secho(f"error: {exc}", fg=typer.colors.RED, err=True)
        raise typer.Exit(EXIT_LOCK) from None


@app.command()
def status(profile: ProfileOpt = DEFAULT_PROFILE, home: HomeOpt = "", as_json: JsonOpt = False) -> None:
    """显示运行状态、接入信息与覆盖率。"""
    client = Client(home, profile)
    data = client.get("/_mqs/status")
    if as_json:
        _emit(data, True)
        return
    config = data
    typer.echo(f"profile    {config['profile']}   uptime {config['uptime_ms'] / 1000:.0f}s   clock {config['clock']['mode']}")
    typer.echo(f"simulate   {config['base_url']}")
    typer.echo(f"gateway    {config['ws_url']}")
    typer.echo(f"control    http://127.0.0.1:{config['control_port']}   token {client.runtime['token']}")
    typer.echo(f"gui        {config.get('gui_url') or '-'}")
    counts = config["counts"]
    typer.echo(
        "world      "
        + "  ".join(f"{k}={v}" for k, v in counts.items() if k in ("users", "bots", "groups", "messages", "events"))
    )
    gw = config["gateway"]
    typer.echo(f"gateway    {gw.get('connections', 0)} connection(s) {gw.get('bots', [])}")
    rate = config["rate"]
    typer.echo(f"rate       {rate['mode']}  allowed={rate['allowed']} rejected={rate['rejected']}")
    typer.echo(f"storage    {config['db_size'] / 1024:.0f} KiB  {config['profile_dir']}")
    typer.echo(f"contract   {config['coverage']['ascii']}")


@app.command()
def token(
    bot: Annotated[str, typer.Option("--bot", help="app_id，省略则用第一个 Bot")] = "",
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
    as_json: JsonOpt = False,
) -> None:
    """打印 Bot 的 App Access Token（7200s）。"""
    client = Client(home, profile)
    bots = client.get("/_mqs/bots")
    if not bots:
        typer.secho("no bot in this profile", fg=typer.colors.RED, err=True)
        raise typer.Exit(EXIT_FAIL)
    app_id = bot or bots[0]["id"]
    data = client.post(f"/_mqs/bots/{app_id}/token")
    if as_json:
        _emit({**data, "app_id": app_id}, True)
    else:
        typer.echo(data["access_token"])
        typer.echo(f"# app_id={app_id} expires_in={data['expires_in']}", err=True)


# ---------------------------------------------------------------------------
# users
# ---------------------------------------------------------------------------

@user_app.command("create")
def user_create(
    name: Annotated[str, typer.Option("--name")] = "",
    random: Annotated[bool, typer.Option("--random", help="随机昵称/资料")] = False,
    count: Annotated[int, typer.Option("--count")] = 1,
    seed: Annotated[int | None, typer.Option("--seed")] = None,
    behavior: Annotated[str | None, typer.Option("--behavior")] = None,
    actor_type: Annotated[str, typer.Option("--actor-type")] = "human",
    group: Annotated[str, typer.Option("--group", help="创建后加入该群")] = "",
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
    as_json: JsonOpt = False,
) -> None:
    """创建虚拟用户（--random 走种子生成器）。"""
    client = Client(home, profile)
    if random or count > 1:
        data = client.post(
            "/_mqs/users/random",
            json={
                "count": count,
                "seed": seed,
                "behavior": behavior,
                "actor_type": actor_type,
                "group_id": group or None,
            },
        )
    else:
        data = [
            client.post(
                "/_mqs/users",
                json={"nickname": name or None, "actor_type": actor_type, "seed": seed, "behavior": behavior or "normal"},
            )
        ]
    if as_json:
        _emit(data, True)
    else:
        _table(["id", "nickname", "actor_type", "behavior"], [[u["id"], u["nickname"], u["actor_type"], u["behavior"]] for u in data])


@user_app.command("list")
def user_list(
    available: Annotated[bool, typer.Option("--available", help="只看可操作账号")] = False,
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
    as_json: JsonOpt = False,
) -> None:
    """列出虚拟用户。"""
    data = Client(home, profile).get("/_mqs/users", params={"available": available})
    if as_json:
        _emit(data, True)
    else:
        _table(["id", "nickname", "actor_type", "behavior"], [[u["id"], u["nickname"], u["actor_type"], u["behavior"]] for u in data])


# ---------------------------------------------------------------------------
# groups
# ---------------------------------------------------------------------------

@group_app.command("create")
def group_create(
    name: Annotated[str, typer.Option("--name")] = "",
    members: Annotated[int, typer.Option("--members", help="随机成员数")] = 0,
    bots: Annotated[list[str] | None, typer.Option("--bot", help="加入的 Bot app_id（可重复）")] = None,
    owner: Annotated[str, typer.Option("--owner", help="群主 user_id")] = "",
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
    as_json: JsonOpt = False,
) -> None:
    """创建群（默认把所有 Bot 拉进来）。"""
    client = Client(home, profile)
    data = client.post(
        "/_mqs/groups",
        json={
            "name": name or None,
            "owner_id": owner or None,
            "random_members": members,
            "bot_ids": bots or [],
        },
    )
    if as_json:
        _emit(data, True)
    else:
        typer.echo(f"created {data['id']}  {data['name']}  members={data['member_count']}  bots={data['bot_ids']}")


@group_app.command("list")
def group_list(profile: ProfileOpt = DEFAULT_PROFILE, home: HomeOpt = "", as_json: JsonOpt = False) -> None:
    """列出群。"""
    data = Client(home, profile).get("/_mqs/groups")
    if as_json:
        _emit(data, True)
    else:
        _table(["id", "name", "members", "bots"], [[g["id"], g["name"], g["member_count"], ",".join(g["bot_ids"])] for g in data])


# ---------------------------------------------------------------------------
# bots
# ---------------------------------------------------------------------------

@bot_app.command("whitelist")
def bot_whitelist(
    app_id: Annotated[str, typer.Argument(help="Bot app_id")],
    state: Annotated[str, typer.Argument(help="on / off")],
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
) -> None:
    """设置 Bot 的内邀接口白名单（模拟腾讯 11253）。"""
    enabled = state.lower() in ("on", "true", "1", "yes")
    data = Client(home, profile).patch(f"/_mqs/bots/{app_id}", json={"whitelisted": enabled})
    typer.echo(f"{data['app_id']} whitelisted={data['whitelisted']}")


@bot_app.command("list")
def bot_list(profile: ProfileOpt = DEFAULT_PROFILE, home: HomeOpt = "", as_json: JsonOpt = False) -> None:
    """列出 Bot。"""
    data = Client(home, profile).get("/_mqs/bots")
    if as_json:
        _emit(data, True)
    else:
        _table(
            ["app_id", "name", "secret", "online", "whitelist", "scopes"],
            [
                [b["app_id"], b["name"], b["secret"], b["online"], "yes" if b.get("whitelisted") else "no", ",".join(b["scopes"])]
                for b in data
            ],
        )


@bot_app.command("create")
def bot_create(
    app_id: Annotated[str, typer.Option("--app-id")] = "",
    name: Annotated[str, typer.Option("--name")] = "",
    secret: Annotated[str, typer.Option("--secret")] = "",
    delivery: Annotated[str, typer.Option("--delivery")] = "ws",
    random: Annotated[bool, typer.Option("--random", help="随机名称与能力集")] = False,
    whitelisted: Annotated[bool, typer.Option("--whitelisted", help="加入内邀接口白名单")] = False,
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
    as_json: JsonOpt = False,
) -> None:
    """创建 Bot。"""
    data = Client(home, profile).post(
        "/_mqs/bots",
        json={
            "app_id": app_id or None,
            "name": name or None,
            "secret": secret or None,
            "delivery": delivery,
            "random": random,
            "whitelisted": whitelisted,
        },
    )
    _emit(data, as_json) if as_json else typer.echo(f"created bot {data['app_id']} secret={data['secret']}")


# ---------------------------------------------------------------------------
# send / expect / clock
# ---------------------------------------------------------------------------

@app.command()
def send(
    as_user: Annotated[str, typer.Option("--as-user", help="以哪个用户身份发言")],
    text: Annotated[str, typer.Option("--text", help="消息内容")] = "",
    to: Annotated[str, typer.Option("--to", help="group:<id> | dm:<bot>:<user>")] = "",
    conversation: Annotated[str, typer.Option("--conversation")] = "",
    at_bot: Annotated[list[str] | None, typer.Option("--at-bot", help="指定要 @ 的 Bot，可重复")] = None,
    no_at: Annotated[bool, typer.Option("--no-at", help="不 @ 任何 Bot（普通群消息）")] = False,
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
    as_json: JsonOpt = False,
) -> None:
    """以虚拟用户身份发送消息（触发网关事件）。"""
    client = Client(home, profile)
    data = client.post(
        "/_mqs/chat/send",
        json={
            "as_user": as_user,
            "content": text,
            "to": to or None,
            "conversation_id": conversation or None,
            "at_bots": [] if no_at else (at_bot or None),
        },
    )
    if as_json:
        _emit(data, True)
    else:
        message = data["message"]
        typer.echo(f"{message['msg_id']}  {message['conversation_id']}  seq={message['seq']}")


@app.command()
def expect(
    event: Annotated[str, typer.Option("--event", help="事件类型，大小写不敏感")] = "",
    timeout: Annotated[str, typer.Option("--timeout")] = "3s",
    content: Annotated[str, typer.Option("--content", help="事件内容需包含该子串")] = "",
    conversation: Annotated[str, typer.Option("--conversation")] = "",
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
    as_json: JsonOpt = False,
) -> None:
    """等待一个事件（超时退出码 5）。"""
    client = Client(home, profile)
    state = client.get("/_mqs/status")
    baseline = int(state["max_event_seq"])
    deadline = time.monotonic() + parse_duration(timeout) / 1000
    needle = event.upper()
    while time.monotonic() < deadline:
        events = client.get("/_mqs/debug/events", params={"since_seq": baseline, "limit": 200})["items"]
        for ev in events:
            baseline = max(baseline, int(ev["seq"]))
            if needle and ev["type"] != needle:
                continue
            if conversation and ev.get("conversation_id") != conversation:
                continue
            if content and content not in json.dumps(ev.get("payload") or {}, ensure_ascii=False):
                continue
            if as_json:
                _emit(ev, True)
            else:
                typer.echo(f"{ev['type']}  seq={ev['seq']}  {ev.get('conversation_id')}  {json.dumps(ev['payload'], ensure_ascii=False)[:200]}")
            return
        time.sleep(0.1)
    typer.secho(f"timeout: no event matched within {timeout}", fg=typer.colors.RED, err=True)
    raise typer.Exit(EXIT_FAIL)


@clock_app.command("advance")
def clock_advance(
    duration: Annotated[str, typer.Argument(help="例如 6m / 2h / 30s")],
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
    as_json: JsonOpt = False,
) -> None:
    """快进虚拟时钟。"""
    data = Client(home, profile).post("/_mqs/clock/advance", json={"duration": duration})
    _emit(data if as_json else data["clock"], as_json) if as_json else typer.echo(f"clock now {data['clock']['now']} ({data['clock']['mode']})")


@clock_app.command("set")
def clock_set(
    iso: Annotated[str, typer.Argument(help="ISO 时间或毫秒时间戳")],
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
    as_json: JsonOpt = False,
) -> None:
    """把虚拟时钟设为指定时间。"""
    payload: dict[str, Any] = {"ms": int(iso)} if iso.isdigit() else {"iso": iso}
    data = Client(home, profile).post("/_mqs/clock/set", json=payload)
    typer.echo(json.dumps(data, ensure_ascii=False) if as_json else f"clock now {data['clock']['now']}")


# ---------------------------------------------------------------------------
# scenarios
# ---------------------------------------------------------------------------

@script_app.command("list")
def script_list(profile: ProfileOpt = DEFAULT_PROFILE, home: HomeOpt = "", as_json: JsonOpt = False) -> None:
    """列出内置与本地场景。"""
    data = Client(home, profile).get("/_mqs/scenarios")
    if as_json:
        _emit(data, True)
    else:
        _table(["name", "source", "steps", "description"], [[s["name"], s["source"], s["steps"], s["description"]] for s in data])


@script_app.command("run")
def script_run(
    script: Annotated[str, typer.Argument(help="内置场景名或 YAML 路径")],
    junit: Annotated[str, typer.Option("--junit", help="写入 JUnit XML")] = "",
    tap: Annotated[bool, typer.Option("--tap", help="额外输出 TAP")] = False,
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
    as_json: JsonOpt = False,
) -> None:
    """运行场景（失败退出码 5）。"""
    from .scenario import get_builtin

    client = Client(home, profile, timeout=120)
    builtin = get_builtin(script)
    payload: dict[str, Any] = {}
    if builtin is not None:
        payload["script"] = builtin
    else:
        path = Path(script).expanduser()
        if not path.exists():
            typer.secho(f"scenario not found: {script}", fg=typer.colors.RED, err=True)
            raise typer.Exit(EXIT_FAIL)
        payload["path"] = str(path.resolve())
    data = client.post("/_mqs/scenarios/run", json=payload)
    if as_json:
        _emit({k: v for k, v in data.items() if k != "junit"}, True)
    else:
        for step in data["steps"]:
            mark = "PASS" if step["ok"] else "FAIL"
            color = typer.colors.GREEN if step["ok"] else typer.colors.RED
            typer.secho(f"[{mark}] {step['name']}  ({step['duration_ms']}ms)", fg=color)
            if not step["ok"] and step["detail"]:
                typer.echo(f"       {step['detail']}")
        typer.echo(f"\n{data['name']}: {'ok' if data['ok'] else 'FAILED'}  {data['duration_ms']}ms")
        if tap:
            typer.echo(data["tap"])
    if junit:
        Path(junit).parent.mkdir(parents=True, exist_ok=True)
        Path(junit).write_text(data["junit"], encoding="utf-8")
        typer.echo(f"junit written to {junit}", err=True)
    if not data["ok"]:
        raise typer.Exit(EXIT_FAIL)


# ---------------------------------------------------------------------------
# logs / schema / doctor
# ---------------------------------------------------------------------------

@app.command()
def logs(
    follow: Annotated[bool, typer.Option("-f", "--follow")] = False,
    kind: Annotated[str, typer.Option("--kind", help="http | ws | event")] = "",
    bot: Annotated[str, typer.Option("--bot")] = "",
    limit: Annotated[int, typer.Option("--limit")] = 50,
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
    as_json: JsonOpt = False,
) -> None:
    """查看调试帧（HTTP/WS 双向）。"""
    client = Client(home, profile)
    state = client.get("/_mqs/status")
    since = max(0, int(state["max_frame_id"]) - limit)
    try:
        while True:
            data = client.get(
                "/_mqs/debug/frames",
                params={"since_id": since, "limit": 200, "kind": kind or None, "bot_id": bot or None},
            )
            for frame in data["items"]:
                since = max(since, int(frame["id"]))
                if as_json:
                    typer.echo(json.dumps(frame, ensure_ascii=False))
                else:
                    stamp = time.strftime("%H:%M:%S", time.localtime(frame["ts"] / 1000))
                    typer.echo(f"{stamp} {frame['kind']:6} {frame['direction']:8} {frame['summary']}")
            if not follow:
                return
            time.sleep(0.5)
    except KeyboardInterrupt:  # pragma: no cover
        return


@app.command()
def schema(profile: ProfileOpt = DEFAULT_PROFILE, home: HomeOpt = "", as_json: JsonOpt = False) -> None:
    """输出控制 API 的 OpenAPI。"""
    client = Client(home, profile)
    with httpx.Client(timeout=10) as http:
        data = http.get(f"http://127.0.0.1:{client.runtime['control_port']}/openapi.json").json()
    _emit(data, True)


@app.command()
def doctor(profile: ProfileOpt = DEFAULT_PROFILE, home: HomeOpt = "") -> None:
    """环境自检：目录、DB、端口、运行时。"""
    home_path = resolve_home(home or None)
    paths = ProfilePaths(home_path, profile or DEFAULT_PROFILE)
    checks: list[tuple[str, bool, str]] = []
    checks.append(("python", sys.version_info >= (3, 11), sys.version.split()[0]))
    try:
        paths.ensure()
        probe = paths.directory / ".doctor"
        probe.write_text("ok", encoding="utf-8")
        probe.unlink()
        checks.append(("profile dir writable", True, str(paths.directory)))
    except OSError as exc:
        checks.append(("profile dir writable", False, str(exc)))
    if paths.db.exists():
        try:
            con = sqlite3.connect(paths.db)
            ok = con.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
            con.close()
            checks.append(("sqlite integrity", ok, str(paths.db)))
        except sqlite3.Error as exc:
            checks.append(("sqlite integrity", False, str(exc)))
    else:
        checks.append(("sqlite integrity", True, "no db yet (first run will create it)"))
    try:
        runtime = read_runtime(home_path, paths.name)
        alive = False
        try:
            with httpx.Client(timeout=2) as http:
                alive = http.get(f"http://127.0.0.1:{runtime['control_port']}/_mqs/health").status_code == 200
        except httpx.HTTPError:
            alive = False
        checks.append(("running server", alive, f"control port {runtime['control_port']}"))
        for label, port in (("sim port", runtime.get("sim_port")), ("control port", runtime.get("control_port"))):
            if not port:
                continue
            sock = socket.socket()
            try:
                sock.bind(("127.0.0.1", int(port)))
                free = True
            except OSError:
                free = False
            finally:
                sock.close()
            checks.append((label, free or alive, f"{port} {'free' if free else 'in use'}"))
    except RuntimeError:
        checks.append(("running server", True, "not running"))
    failed = 0
    for label, ok, detail in checks:
        mark = "ok " if ok else "ERR"
        color = typer.colors.GREEN if ok else typer.colors.RED
        typer.secho(f"[{mark}] {label:22} {detail}", fg=color)
        failed += 0 if ok else 1
    if failed:
        raise typer.Exit(EXIT_CONN)


@app.command()
def mcp() -> None:
    """以 stdio MCP server 方式运行（给 Agent 用）。"""
    from .mcp_server import main as mcp_main

    mcp_main()


# ---------------------------------------------------------------------------
# contract / profile
# ---------------------------------------------------------------------------

@contract_app.command("export")
def contract_export(
    out: Annotated[str, typer.Option("--out", help="覆盖率文档路径")] = "docs/COVERAGE.md",
    types: Annotated[str, typer.Option("--types", help="TS 类型路径")] = "web/src/types/api.ts",
    as_json: JsonOpt = False,
) -> None:
    """生成覆盖率文档与前端 TS 类型（不依赖运行中的服务）。"""
    from .contract import summary

    out_path = Path(out)
    types_path = Path(types)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    types_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(coverage_markdown(), encoding="utf-8")
    types_path.write_text(typescript(), encoding="utf-8")
    result = {"coverage": str(out_path), "types": str(types_path), "summary": summary()}
    if as_json:
        _emit(result, True)
    else:
        typer.echo(f"wrote {out_path} and {types_path}")
        typer.echo(summary()["ascii"])


@profile_app.command("list")
def profile_list(home: HomeOpt = "", as_json: JsonOpt = False) -> None:
    """列出本机 profile（读 profiles.json 索引）。"""
    home_path = resolve_home(home or None)
    index_path = home_path / "profiles.json"
    entries: list[dict] = []
    try:
        data = json.loads(index_path.read_text(encoding="utf-8")) if index_path.exists() else {}
        entries = list((data.get("profiles") or {}).values())
    except (OSError, ValueError):
        entries = []
    if not entries and (home_path / "profiles").exists():
        for directory in sorted((home_path / "profiles").iterdir()):
            if directory.is_dir():
                entries.append({"name": directory.name, "path": str(directory)})
    if as_json:
        _emit(entries, True)
    else:
        _table(["name", "path", "last_opened"], [[e.get("name"), e.get("path"), e.get("last_opened_at", "")] for e in entries])


@profile_app.command("export")
def profile_export(
    path: Annotated[str, typer.Argument(help="输出 .mqsprofile")] = "mqs-profile.zip",
    profile: ProfileOpt = DEFAULT_PROFILE,
    home: HomeOpt = "",
) -> None:
    """导出当前 profile 快照（zip）。"""
    client = Client(home, profile)
    with httpx.Client(timeout=60) as http:
        response = http.post(f"{client.base}/_mqs/snapshot", headers=client.headers)
    if response.status_code != 200:
        typer.secho(response.text[:300], fg=typer.colors.RED, err=True)
        raise typer.Exit(EXIT_FAIL)
    Path(path).write_bytes(response.content)
    typer.echo(f"wrote {path} ({len(response.content)} bytes)")


@profile_app.command("import")
def profile_import(
    path: Annotated[str, typer.Argument(help=".mqsprofile / zip 路径")],
    name: Annotated[str, typer.Option("--name", help="新 profile 名")] = "",
    force: Annotated[bool, typer.Option("--force")] = False,
    home: HomeOpt = "",
) -> None:
    """导入 profile 快照（服务需已停止）。"""
    source = Path(path).expanduser()
    if not source.exists():
        typer.secho(f"not found: {source}", fg=typer.colors.RED, err=True)
        raise typer.Exit(EXIT_FAIL)
    target_name = name or source.stem.replace(".mqsprofile", "")
    home_path = resolve_home(home or None)
    target = ProfilePaths(home_path, target_name).directory
    if target.exists() and not force:
        typer.secho(f"profile already exists: {target} (use --force)", fg=typer.colors.RED, err=True)
        raise typer.Exit(EXIT_USAGE)
    staging = target.with_suffix(".importing")
    shutil.rmtree(staging, ignore_errors=True)
    staging.mkdir(parents=True)
    with zipfile.ZipFile(source) as zf:
        zf.extractall(staging)
    db = staging / "db.sqlite"
    if not db.exists():
        shutil.rmtree(staging, ignore_errors=True)
        typer.secho("invalid snapshot: db.sqlite missing", fg=typer.colors.RED, err=True)
        raise typer.Exit(EXIT_FAIL)
    shutil.rmtree(target, ignore_errors=True)
    staging.rename(target)
    meta = target / "profile.json"
    if not meta.exists():
        meta.write_text(json.dumps({"name": target_name, "schema_version": 1}), encoding="utf-8")
    typer.echo(f"imported profile {target_name} -> {target}")


def main() -> None:
    app()


if __name__ == "__main__":  # pragma: no cover
    main()
