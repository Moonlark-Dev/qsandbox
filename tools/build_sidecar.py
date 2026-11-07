"""Build the MQS server sidecar with Nuitka (cross-platform).

Usage: python tools/build_sidecar.py
Output: build-sidecar/mqs (or mqs.exe on Windows)

The web GUI is embedded, so build it first:

    cd web && npm ci && npm run build
"""

from __future__ import annotations

import importlib.util
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "build-sidecar"
WEB_DIST = ROOT / "web" / "dist"

# Packages imported dynamically (importlib / plugin discovery) or through
# optional uvicorn extras. Missing ones are skipped so the same script works on
# all platforms (e.g. uvloop does not exist on Windows).
CANDIDATE_PACKAGES = [
    "mqs",
    "uvicorn",
    "fastapi",
    "starlette",
    "pydantic",
    "pydantic_core",
    "mcp",
    "aiosqlite",
    "anyio",
    "typer",
    "yaml",
    "httpx",
    "websockets",
    "httptools",
    "uvloop",
    "python_multipart",
    "multipart",
    "dotenv",
]


def include_packages() -> list[str]:
    packages: list[str] = []
    for name in CANDIDATE_PACKAGES:
        if name == "uvloop" and sys.platform == "win32":
            continue
        try:
            if importlib.util.find_spec(name) is None:
                continue
        except (ImportError, ModuleNotFoundError, ValueError):
            continue
        packages.append(name)
    return packages


def build_args() -> list[str]:
    args = [
        sys.executable,
        "-m",
        "nuitka",
        "--onefile",
        "--assume-yes-for-downloads",
        "--remove-output",
        f"--output-dir={OUT}",
        "--output-filename=mqs",
        # The GUI is served from web/dist next to the extracted executable.
        f"--include-data-dir={WEB_DIST}=web/dist",
    ]
    # SQL migrations are read via Path(__file__).with_name(...) at runtime.
    for schema in sorted((ROOT / "src" / "mqs").glob("schema*.sql")):
        args.append(f"--include-data-files={schema}=mqs/{schema.name}")
    for package in include_packages():
        args.append(f"--include-package={package}")
    # Optional SDK shims must not leak into the sandbox binary, otherwise the
    # build silently depends on qq-botpy being installed in the build env.
    args += [
        "--nofollow-import-to=botpy",
        "--nofollow-import-to=aiohttp",
    ]
    args.append(str(ROOT / "tools" / "mqs_entry.py"))
    return args


def main() -> int:
    if not (WEB_DIST / "index.html").exists():
        print("web/dist is missing; run: cd web && npm ci && npm run build", file=sys.stderr)
        return 1

    try:
        import nuitka  # noqa: F401
    except ImportError:
        print("Nuitka is required: pip install '.[build]'", file=sys.stderr)
        return 1

    shutil.rmtree(OUT, ignore_errors=True)
    OUT.mkdir(parents=True, exist_ok=True)

    subprocess.run(build_args(), cwd=ROOT, check=True)

    binary = OUT / ("mqs.exe" if sys.platform == "win32" else "mqs")
    if not binary.exists():
        print(f"build failed: {binary} missing", file=sys.stderr)
        return 1
    print(f"built {binary} ({binary.stat().st_size / 1024 / 1024:.1f} MiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
