"""Verify that a built desktop package really contains the compiled sidecar.

The desktop shell only knows how to talk to ``resources/sidecar/mqs``; if the
Nuitka build or the electron-builder ``extraResources`` copy silently misses it,
the shipped installer opens with "QSandbox 启动失败" on the user's machine and
nothing in CI notices. This check closes that gap: it looks at the unpacked app
electron-builder just produced, asserts the sidecar (and the embedded web GUI)
are there, and then runs the packaged binary for real.

Usage:
    python tools/verify_desktop_bundle.py --target linux|win|mac
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

from smoke_sidecar import SmokeError, run_smoke  # noqa: E402

RELEASE = ROOT / "desktop" / "release"
MIN_SIDECAR_BYTES = 5 * 1024 * 1024


def find_unpacked(target: str) -> Path:
    if target == "linux":
        return RELEASE / "linux-unpacked"
    if target == "win":
        return RELEASE / "win-unpacked"
    if target == "mac":
        apps = sorted(RELEASE.glob("mac*/QSandbox.app"))
        if not apps:
            raise SmokeError(f"no mac app bundle under {RELEASE} (looked for mac*/QSandbox.app)")
        return apps[0]
    raise SmokeError(f"unknown target: {target}")


def resources_dir(unpacked: Path, target: str) -> Path:
    if target == "mac":
        return unpacked / "Contents" / "Resources"
    return unpacked / "resources"


def main() -> int:
    parser = argparse.ArgumentParser(description="Check that the built desktop package bundles the sidecar")
    parser.add_argument("--target", required=True, choices=["linux", "win", "mac"])
    parser.add_argument("--skip-smoke", action="store_true", help="only check the files, do not start the binary")
    options = parser.parse_args()

    exe_name = "mqs.exe" if options.target == "win" else "mqs"
    try:
        unpacked = find_unpacked(options.target)
        resources = resources_dir(unpacked, options.target)
        if not unpacked.is_dir():
            raise SmokeError(f"unpacked app missing: {unpacked}")
        if not (resources / "app.asar").exists():
            raise SmokeError(f"app.asar missing under {resources}")

        sidecar_dir = resources / "sidecar"
        binary = sidecar_dir / exe_name
        if not binary.exists():
            listing = sorted(p.name for p in sidecar_dir.iterdir())[:10] if sidecar_dir.is_dir() else []
            raise SmokeError(
                f"compiled sidecar is NOT bundled: {binary} is missing.\n"
                "The installer would start without mqs (the desktop shell would fall back to "
                "MQS_PYTHON and fail). Check `npm run dist:*` runs after "
                "`python tools/build_sidecar.py` and that extraResources points at build-sidecar/mqs.dist.\n"
                f"resources/sidecar contains: {listing or 'nothing'}"
            )
        size = binary.stat().st_size
        if size < MIN_SIDECAR_BYTES:
            raise SmokeError(f"bundled sidecar looks broken: {binary} is only {size / 1024 / 1024:.1f} MiB")
        if options.target != "win" and not os.access(binary, os.X_OK):
            raise SmokeError(f"bundled sidecar is not executable: {binary}")

        gui_index = sidecar_dir / "web" / "dist" / "index.html"
        if not gui_index.exists():
            raise SmokeError(
                f"the sidecar has no embedded GUI: {gui_index} is missing "
                "(build web/dist before running tools/build_sidecar.py)"
            )

        total = sum(p.stat().st_size for p in sidecar_dir.rglob("*") if p.is_file())
        print(
            f"bundle ok: {options.target} -> {binary.relative_to(ROOT)} "
            f"({size / 1024 / 1024:.1f} MiB binary, {total / 1024 / 1024:.1f} MiB sidecar dir)"
        )

        if not options.skip_smoke:
            handshake = run_smoke(binary)
            print(
                "packaged sidecar ok: "
                f"profile={handshake.get('profile')} sim={handshake.get('sim_port')} "
                f"control={handshake.get('control_port')}"
            )
    except SmokeError as exc:
        print(f"bundle check failed: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
