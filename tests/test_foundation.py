import json
from pathlib import Path

import pytest

from mqs.clock import Clock, human_duration, parse_duration
from mqs.config import ProfilePaths
from mqs.db import DEFAULT_SETTINGS, Database
from mqs.errors import InvalidParam, RateLimited
from mqs.ids import FakeData, openid_for


def test_parse_duration() -> None:
    assert parse_duration("500ms") == 500
    assert parse_duration("3s") == 3000
    assert parse_duration("6m") == 360_000
    assert parse_duration("2h") == 7_200_000
    assert parse_duration("1d") == 86_400_000
    assert parse_duration("42") == 42_000
    with pytest.raises(ValueError):
        parse_duration("nope")


def test_clock_virtual_and_real() -> None:
    clock = Clock("real")
    real_now = clock.now_ms()
    assert abs(real_now - Clock("real").now_ms()) < 5000
    clock.set_mode("virtual")
    frozen = clock.now_ms()
    clock.advance(6 * 60 * 1000)
    assert clock.now_ms() == frozen + 360_000
    assert clock.mode == "virtual"
    assert human_duration(360_000) == "6m"


def test_seeded_fake_data_is_reproducible() -> None:
    a = FakeData(42)
    b = FakeData(42)
    seq_a = [(a.nickname(), a.city(), a.behavior(), a.age()) for _ in range(5)]
    seq_b = [(b.nickname(), b.city(), b.behavior(), b.age()) for _ in range(5)]
    assert seq_a == seq_b
    assert len({name for name, *_ in seq_a}) == 5


def test_openid_is_scoped_per_bot() -> None:
    assert openid_for("10000", "user", "u_1") != openid_for("10001", "user", "u_1")
    assert openid_for("10000", "user", "u_1") == openid_for("10000", "user", "u_1")
    assert len(openid_for("10000", "group", "g_1")) == 32


def test_error_envelope_shape() -> None:
    err = InvalidParam("bad thing")
    body = err.body()
    assert body["code"] == 10001
    assert body["message"] == "bad thing"
    assert len(body["trace_id"]) == 16
    assert err.status == 400


def test_rate_limited_envelope() -> None:
    err = RateLimited(1.5, limit=2, bucket="message")
    assert err.status == 429
    assert err.body()["retry_after"] == 1.5


async def test_migration_is_idempotent_and_seeds_settings(tmp_path: Path) -> None:
    db_path = tmp_path / "db.sqlite"
    for _ in range(2):
        db = Database(db_path)
        await db.connect()
        settings = await db.all_settings()
        assert settings["seed"] == DEFAULT_SETTINGS["seed"]
        await db.close()
    assert db_path.exists()


async def test_profile_layout(tmp_path: Path) -> None:
    paths = ProfilePaths(tmp_path, "dev").ensure()
    assert paths.directory.exists()
    assert paths.media.exists()
    meta = json.loads(paths.meta.read_text(encoding="utf-8"))
    assert meta["name"] == "dev"
    assert "not affiliated" in meta["notice"]
