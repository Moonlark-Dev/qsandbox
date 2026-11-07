"""Profile layout, process settings and the single-process file lock."""

from __future__ import annotations

import json
import os
import secrets
import sys
from dataclasses import dataclass, field
from pathlib import Path

DEFAULT_HOME = Path(os.environ.get("MQS_HOME", "~/.mqs")).expanduser()
DEFAULT_PROFILE = "default"
DEFAULT_SIM_PORT = 5700
DEFAULT_CONTROL_PORT = 5701

RUNTIME_FILE = "runtime.json"
PROFILE_FILE = "profile.json"
DB_FILE = "db.sqlite"
MEDIA_DIR = "media"
LOCK_FILE = "profile.lock"

SCHEMA_VERSION = 1


class ProfileLocked(RuntimeError):
    """Raised when another process already owns this profile."""


@dataclass(frozen=True)
class ProfilePaths:
    home: Path
    name: str

    @property
    def directory(self) -> Path:
        return self.home / "profiles" / self.name

    @property
    def db(self) -> Path:
        return self.directory / DB_FILE

    @property
    def media(self) -> Path:
        return self.directory / MEDIA_DIR

    @property
    def runtime(self) -> Path:
        return self.directory / RUNTIME_FILE

    @property
    def meta(self) -> Path:
        return self.directory / PROFILE_FILE

    @property
    def lock(self) -> Path:
        return self.directory / LOCK_FILE

    def ensure(self) -> ProfilePaths:
        self.directory.mkdir(parents=True, exist_ok=True)
        self.media.mkdir(parents=True, exist_ok=True)
        if not self.meta.exists():
            self.meta.write_text(
                json.dumps(
                    {
                        "name": self.name,
                        "schema_version": SCHEMA_VERSION,
                        "created_at": __import__("datetime").datetime.now().isoformat(timespec="seconds"),
                        "notice": "MQS sandbox profile — fake data only, not affiliated with Tencent",
                    },
                    ensure_ascii=False,
                    indent=2,
                ),
                encoding="utf-8",
            )
        return self


@dataclass
class ProcessConfig:
    """Everything needed to boot one MQS process."""

    profile: str = DEFAULT_PROFILE
    home: Path = DEFAULT_HOME
    host: str = "127.0.0.1"
    sim_port: int = DEFAULT_SIM_PORT
    control_port: int = DEFAULT_CONTROL_PORT
    control_token: str = ""
    log_level: str = "info"
    control_origins: list[str] = field(default_factory=list)
    clock_mode: str = "real"
    seed: int = 42
    expose_docs: bool = True
    gui_url: str = ""
    public_host: str = ""

    @property
    def paths(self) -> ProfilePaths:
        return ProfilePaths(self.home, self.profile)

    def __post_init__(self) -> None:
        self.home = Path(self.home).expanduser()
        if not self.control_token:
            self.control_token = secrets.token_urlsafe(24)

    @property
    def dial_host(self) -> str:
        if self.public_host:
            return self.public_host
        if self.host in ("0.0.0.0", "::"):
            return "127.0.0.1"
        return self.host

    @property
    def base_url(self) -> str:
        return f"http://{self.dial_host}:{self.sim_port}"

    @property
    def ws_url(self) -> str:
        return f"ws://{self.dial_host}:{self.sim_port}/websocket"

    def handshake(self) -> dict:
        return {
            "mqs": 1,
            "profile": self.profile,
            "host": self.host,
            "sim_port": self.sim_port,
            "control_port": self.control_port,
            "token": self.control_token,
            "base_url": self.base_url,
            "ws_url": self.ws_url,
            "pid": os.getpid(),
        }


class ProfileLock:
    """Exclusive lock so a profile is only ever written by one process."""

    def __init__(self, paths: ProfilePaths) -> None:
        self.paths = paths
        self._file = None

    def acquire(self) -> None:
        self.paths.ensure()
        import fcntl

        f = open(self.paths.lock, "a+", encoding="utf-8")
        try:
            fcntl.flock(f.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as exc:  # pragma: no cover - platform dependent
            f.close()
            raise ProfileLocked(f"profile {self.paths.name!r} is locked by another mqs process") from exc
        f.seek(0)
        f.truncate()
        f.write(str(os.getpid()))
        f.flush()
        self._file = f

    def release(self) -> None:
        if self._file is None:
            return
        try:
            import fcntl

            fcntl.flock(self._file.fileno(), fcntl.LOCK_UN)
        finally:
            self._file.close()
            self._file = None
            try:
                self.paths.lock.unlink()
            except OSError:
                pass

    def __enter__(self) -> ProfileLock:
        self.acquire()
        return self

    def __exit__(self, *exc: object) -> None:
        self.release()


def resolve_home(value: str | Path | None = None) -> Path:
    if value is None:
        return DEFAULT_HOME
    return Path(value).expanduser().resolve()


def read_runtime(home: Path, profile: str) -> dict:
    path = ProfilePaths(home, profile).runtime
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise RuntimeError(f"no running MQS server for profile {profile!r} ({path} missing)") from exc


def write_runtime(home: Path, profile: str, payload: dict) -> None:
    paths = ProfilePaths(home, profile).ensure()
    tmp = paths.runtime.with_suffix(".tmp")
    tmp.write_text(json.dumps(payload, indent=2), encoding="utf-8")
    tmp.replace(paths.runtime)


def clear_runtime(home: Path, profile: str) -> None:
    try:
        ProfilePaths(home, profile).runtime.unlink()
    except FileNotFoundError:
        pass


def warn(msg: str) -> None:
    print(f"mqs: {msg}", file=sys.stderr)
