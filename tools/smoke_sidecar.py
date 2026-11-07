"""Start a built sidecar and prove it actually serves.

Why a Python script instead of shell: CI used to smoke-test the sidecar with
``MQS_HOME=... ./build-sidecar/mqs serve ... &``, which is bash-only. The
Windows job never executed the binary at all, so a sidecar that only failed on
Windows could not be caught before a user tried to launch the desktop app.

Checks, in order:
  1. the process prints the JSON handshake (``{"mqs": 1, "gui_url": ...}``);
  2. ``/_mqs/health`` answers on the control port;
  3. the control GUI serves the *built* Vue app, not the "前端尚未构建" fallback
     page (this is how a sidecar built without ``web/dist`` gets caught).

Usage:
    python tools/smoke_sidecar.py                 # autodetect the built binary
    python tools/smoke_sidecar.py --binary PATH   # smoke a specific binary
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXE_NAME = "mqs.exe" if sys.platform == "win32" else "mqs"

HANDSHAKE_TIMEOUT = 60.0


class SmokeError(RuntimeError):
    pass


def default_binary() -> Path:
    """Prefer the standalone build the installer ships, fall back to onefile."""
    candidates = [
        ROOT / "build-sidecar" / "mqs.dist" / EXE_NAME,
        ROOT / "build-sidecar" / EXE_NAME,
    ]
    for candidate in candidates:
        if candidate.exists():
            return candidate
    raise SmokeError(
        "no sidecar binary found; run `python tools/build_sidecar.py` first "
        f"(looked in: {', '.join(str(c) for c in candidates)})"
    )


def terminate_tree(process: subprocess.Popen) -> None:
    """Stop the whole process group — a onefile bootstrap has a child server."""
    if process.poll() is not None:
        return
    if sys.platform == "win32":
        subprocess.run(
            ["taskkill", "/F", "/T", "/PID", str(process.pid)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
        return

    try:
        os.killpg(os.getpgid(process.pid), signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        process.terminate()
    try:
        process.wait(timeout=10)
        return
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(os.getpgid(process.pid), signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        process.kill()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        pass


def _read_lines(stream, sink: queue.Queue) -> None:
    try:
        for line in iter(stream.readline, b""):
            sink.put(line.decode("utf-8", "replace"))
    finally:
        sink.put(None)


def http_get(url: str, timeout: float = 10.0) -> tuple[int, str]:
    request = urllib.request.Request(url, headers={"Accept": "text/html,application/json"})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, response.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read().decode("utf-8", "replace")


def run_smoke(binary: Path, timeout: float = HANDSHAKE_TIMEOUT) -> dict:
    """Start ``binary serve``, validate handshake + health + GUI, then stop it."""
    binary = Path(binary).resolve()
    if not binary.exists():
        raise SmokeError(f"sidecar binary is missing: {binary}")

    workdir = Path(tempfile.mkdtemp(prefix="mqs-smoke-"))
    stderr_path = workdir / "stderr.log"
    token = "smoke-token"
    args = [
        str(binary),
        "serve",
        "--profile",
        "smoke",
        "--host",
        "127.0.0.1",
        "--port",
        "0",
        "--control-port",
        "0",
        "--token",
        token,
    ]
    env = {
        **os.environ,
        "MQS_HOME": str(workdir / "home"),
        "MQS_PROFILE": "smoke",
        "PYTHONUNBUFFERED": "1",
    }

    process: subprocess.Popen | None = None
    try:
        with open(stderr_path, "wb") as stderr_file:
            process = subprocess.Popen(
                args,
                env=env,
                cwd=str(workdir),
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=stderr_file,
                start_new_session=sys.platform != "win32",
            )
            assert process.stdout is not None
            lines: queue.Queue[str | None] = queue.Queue()
            threading.Thread(target=_read_lines, args=(process.stdout, lines), daemon=True).start()

            handshake: dict | None = None
            deadline = time.monotonic() + timeout
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise SmokeError(f"no handshake within {timeout:.0f}s")
                try:
                    line = lines.get(timeout=min(remaining, 1.0))
                except queue.Empty:
                    continue
                if line is None:
                    raise SmokeError("sidecar exited before printing a handshake")
                line = line.strip()
                if not line.startswith("{"):
                    continue
                try:
                    parsed = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if parsed.get("mqs") and parsed.get("gui_url"):
                    handshake = parsed
                    break

            if process.poll() is not None:
                raise SmokeError(f"sidecar exited with code {process.returncode} right after the handshake")

            control = f"http://127.0.0.1:{handshake['control_port']}"
            status, body = http_get(f"{control}/_mqs/health")
            if status != 200:
                raise SmokeError(f"GET /_mqs/health returned {status}: {body[:200]}")
            try:
                health = json.loads(body)
            except json.JSONDecodeError:
                raise SmokeError(f"GET /_mqs/health returned non-JSON: {body[:200]}") from None
            if not health.get("ok"):
                raise SmokeError(f"GET /_mqs/health reported not ok: {body[:200]}")

            gui_url = str(handshake["gui_url"]).split("#", 1)[0]
            status, body = http_get(gui_url)
            if status != 200:
                raise SmokeError(f"GET {gui_url} returned {status}")
            if "前端尚未构建" in body:
                raise SmokeError("GUI served the 'frontend not built' fallback; web/dist was not embedded")
            if "<div id=\"app\"" not in body and "<div id='app'" not in body:
                raise SmokeError("GUI root element missing from index.html")
            return handshake
    except Exception:
        tail = ""
        try:
            tail = stderr_path.read_text("utf-8", "replace")[-4000:].strip()
        except OSError:
            pass
        if tail:
            print(f"--- sidecar stderr ---\n{tail}\n----------------------", file=sys.stderr)
        raise
    finally:
        if process is not None:
            terminate_tree(process)
        shutil.rmtree(workdir, ignore_errors=True)


def main() -> int:
    parser = argparse.ArgumentParser(description="Smoke-test a built MQS sidecar binary")
    parser.add_argument("--binary", type=Path, default=None, help="sidecar executable (autodetected by default)")
    parser.add_argument("--timeout", type=float, default=HANDSHAKE_TIMEOUT)
    options = parser.parse_args()

    try:
        binary = options.binary or default_binary()
        handshake = run_smoke(binary, timeout=options.timeout)
    except SmokeError as exc:
        print(f"smoke test failed: {exc}", file=sys.stderr)
        return 1
    except FileNotFoundError as exc:
        print(f"smoke test failed: {exc}", file=sys.stderr)
        return 1

    print(
        "sidecar ok: "
        f"{binary.name} profile={handshake.get('profile')} "
        f"sim={handshake.get('sim_port')} control={handshake.get('control_port')} "
        f"gui={handshake.get('gui_url', '').split('#')[0]}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
