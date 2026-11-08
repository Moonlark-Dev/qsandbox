"""Install a built wheel into a throwaway venv and prove it actually serves.

This is the PyPI counterpart of ``tools/smoke_sidecar.py``. A wheel built
without ``web/dist`` still installs and still answers the API, so the only
symptom users see is the control GUI quietly turning into the "前端尚未构建"
fallback page. That is exactly the regression this script fails on.

Checks, in order:
  1. the wheel installs into a fresh venv (console script + dependencies);
  2. ``mqs serve`` prints the JSON handshake;
  3. ``/_mqs/health`` answers;
  4. the control GUI serves the built Vue app, not the fallback page.

Usage:
    python tools/smoke_wheel.py                       # newest dist/*.whl
    python tools/smoke_wheel.py --wheel dist/x.whl
    python tools/smoke_wheel.py --skip-install        # wheel already installed
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
import venv
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

from smoke_sidecar import SmokeError, run_smoke  # noqa: E402

SCRIPTS_DIR = "Scripts" if sys.platform == "win32" else "bin"
EXE_NAME = "mqs.exe" if sys.platform == "win32" else "mqs"


class WheelSmokeError(RuntimeError):
    pass


def newest_wheel() -> Path:
    wheels = sorted((ROOT / "dist").glob("*.whl"), key=lambda p: p.stat().st_mtime, reverse=True)
    if not wheels:
        raise WheelSmokeError("no wheel found in dist/; run `python -m build` first")
    return wheels[0]


def install_wheel(wheel: Path, venv_dir: Path) -> Path:
    """Create a venv, install the wheel into it, return the ``mqs`` script."""
    venv.create(venv_dir, with_pip=True)
    python = venv_dir / SCRIPTS_DIR / ("python.exe" if sys.platform == "win32" else "python")
    install = subprocess.run(
        [str(python), "-m", "pip", "install", "--disable-pip-version-check", "-q", str(wheel)],
        capture_output=True,
        text=True,
    )
    if install.returncode != 0:
        raise WheelSmokeError(f"pip install {wheel.name} failed:\n{install.stdout}\n{install.stderr}")
    script = venv_dir / SCRIPTS_DIR / EXE_NAME
    if not script.exists():
        raise WheelSmokeError(f"the wheel did not install an mqs console script ({script})")
    return script


def main() -> int:
    parser = argparse.ArgumentParser(description="Smoke-test a built MQS wheel in a clean venv")
    parser.add_argument("--wheel", type=Path, default=None, help="wheel to install (newest dist/*.whl by default)")
    parser.add_argument("--skip-install", action="store_true", help="reuse the current interpreter instead of a venv")
    parser.add_argument("--timeout", type=float, default=90.0)
    options = parser.parse_args()

    workdir = Path(tempfile.mkdtemp(prefix="mqs-wheel-smoke-"))
    try:
        if options.skip_install:
            script = Path(sys.executable).with_name(EXE_NAME)
            wheel = options.wheel
        else:
            wheel = (options.wheel or newest_wheel()).resolve()
            if not wheel.exists():
                raise WheelSmokeError(f"wheel is missing: {wheel}")
            script = install_wheel(wheel, workdir / "venv")
        handshake = run_smoke(script, options.timeout)
    except (SmokeError, WheelSmokeError) as exc:
        print(f"wheel smoke test failed: {exc}", file=sys.stderr)
        return 1
    except FileNotFoundError as exc:
        print(f"wheel smoke test failed: {exc}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(workdir, ignore_errors=True)

    label = wheel.name if wheel is not None else str(script)
    print(
        "wheel ok: "
        f"{label} profile={handshake.get('profile')} "
        f"sim={handshake.get('sim_port')} control={handshake.get('control_port')} "
        f"version={handshake.get('version')}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
