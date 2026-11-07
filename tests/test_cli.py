"""CLI contract: stable JSON output and meaningful exit codes."""

import asyncio
import json
import os
import sys
from types import SimpleNamespace


async def _run(args: list[str], env: dict, cwd: str) -> SimpleNamespace:
    process = await asyncio.create_subprocess_exec(
        sys.executable,
        "-m",
        "mqs",
        *args,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        env=env,
        cwd=cwd,
    )
    try:
        stdout, stderr = await asyncio.wait_for(process.communicate(), timeout=60)
    except TimeoutError:
        process.kill()
        raise
    return SimpleNamespace(
        returncode=process.returncode,
        stdout=stdout.decode("utf-8", "replace"),
        stderr=stderr.decode("utf-8", "replace"),
    )


def _env(mqs) -> dict:
    env = dict(os.environ)
    env["MQS_HOME"] = str(mqs.config.home)
    env["MQS_PROFILE"] = mqs.config.profile
    env.pop("MQS_BASE_URL", None)
    return env


async def test_status_json(mqs, tmp_path) -> None:
    env = _env(mqs)
    proc = await _run(["status", "--json"], env, str(tmp_path))
    assert proc.returncode == 0, proc.stderr
    data = json.loads(proc.stdout)
    assert data["profile"] == "test"
    assert data["counts"]["bots"] >= 1
    assert data["coverage"]["total"] > 0
    assert data["max_event_seq"] >= 0


async def test_token_json(mqs, tmp_path) -> None:
    proc = await _run(["token", "--json"], _env(mqs), str(tmp_path))
    assert proc.returncode == 0, proc.stderr
    data = json.loads(proc.stdout)
    assert data["access_token"].startswith("MQS_")
    assert data["app_id"] == "10000"


async def test_user_and_group_commands(mqs, tmp_path) -> None:
    env = _env(mqs)
    proc = await _run(["user", "create", "--name", "命令行用户", "--json"], env, str(tmp_path))
    assert proc.returncode == 0, proc.stderr
    users = json.loads(proc.stdout)
    assert users[0]["nickname"] == "命令行用户"

    proc = await _run(["group", "create", "--name", "命令行群", "--members", "2", "--json"], env, str(tmp_path))
    assert proc.returncode == 0, proc.stderr
    group = json.loads(proc.stdout)
    assert group["name"] == "命令行群"
    assert group["member_count"] >= 3


async def test_script_run_and_junit(mqs, echo_bot, tmp_path) -> None:
    junit = tmp_path / "out.xml"
    proc = await _run(["script", "run", "smoke", "--junit", str(junit)], _env(mqs), str(tmp_path))
    assert proc.returncode == 0, proc.stdout + proc.stderr
    assert "smoke: ok" in proc.stdout
    assert junit.exists()
    assert "<testsuite" in junit.read_text(encoding="utf-8")


async def test_missing_runtime_exits_3(tmp_path) -> None:
    env = dict(os.environ)
    env["MQS_HOME"] = str(tmp_path / "nope")
    env["MQS_PROFILE"] = "missing"
    proc = await _run(["status", "--json"], env, str(tmp_path))
    assert proc.returncode == 3
    assert "mqs serve" in proc.stderr


async def test_doctor(mqs, tmp_path) -> None:
    proc = await _run(["doctor"], _env(mqs), str(tmp_path))
    assert proc.returncode == 0, proc.stdout + proc.stderr
    assert "sqlite integrity" in proc.stdout
