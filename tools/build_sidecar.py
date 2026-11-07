"""Build the MQS server sidecar with PyInstaller (cross-platform).

Usage: python tools/build_sidecar.py
Output: build-sidecar/mqs (or mqs.exe on Windows)
"""

from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "build-sidecar"
WORK = ROOT / "build" / "pyinstaller"
SPEC = ROOT / "build"


def main() -> int:
    try:
        import PyInstaller.__main__ as pyinstaller
    except ImportError:
        print("PyInstaller is required: pip install '.[build]'", file=sys.stderr)
        return 1

    shutil.rmtree(OUT, ignore_errors=True)
    sep = os.pathsep
    args = [
        "--noconfirm",
        "--onefile",
        "--name",
        "mqs",
        "--paths",
        str(ROOT / "src"),
        *[
            item
            for schema in sorted((ROOT / "src" / "mqs").glob("schema*.sql"))
            for item in ("--add-data", f"{schema}{sep}mqs")
        ],
        "--add-data",
        f"{ROOT / 'web' / 'dist'}{sep}web/dist",
        "--collect-submodules",
        "uvicorn",
        "--collect-submodules",
        "fastapi",
        "--collect-submodules",
        "pydantic",
        "--collect-submodules",
        "mcp",
        "--collect-submodules",
        "aiosqlite",
        "--collect-submodules",
        "anyio",
        "--distpath",
        str(OUT),
        "--workpath",
        str(WORK),
        "--specpath",
        str(SPEC),
        str(ROOT / "tools" / "mqs_entry.py"),
    ]
    pyinstaller.run(args)
    binary = OUT / ("mqs.exe" if sys.platform == "win32" else "mqs")
    if not binary.exists():
        print(f"build failed: {binary} missing", file=sys.stderr)
        return 1
    print(f"built {binary} ({binary.stat().st_size / 1024 / 1024:.1f} MiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
