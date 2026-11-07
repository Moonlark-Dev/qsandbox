"""Build the MQS server sidecar with Nuitka (cross-platform).

Recommended (and what the desktop installer ships): a **standalone** build.

    python tools/build_sidecar.py              # -> build-sidecar/mqs.dist/{mqs,mqs.exe}
    python tools/build_sidecar.py --onefile    # -> build-sidecar/mqs (legacy single file)

Why standalone by default: a Nuitka ``--onefile`` binary unpacks ~200 MiB into
``TMPDIR`` on *every* launch. When the temp filesystem is small or full (a 2 GiB
tmpfs is common) the sidecar dies with "couldn't unpack file to target path.: No
space left on device" and exits with code 1, which the desktop shell can only
report as "QSandbox server exited with code 1". A standalone build is a plain
directory shipped next to the Electron resources: no unpacking, no temp-space
requirement, and the app opens immediately.

The web GUI is embedded, so build it first:

    cd web && npm ci && npm run build
"""

from __future__ import annotations

import argparse
import importlib.util
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "build-sidecar"
WEB_DIST = ROOT / "web" / "dist"
DIST_DIR_NAME = "mqs.dist"

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
    # Windows has no system zoneinfo database; mqs.clock needs this at import.
    "tzdata",
]

EXE_NAME = "mqs.exe" if sys.platform == "win32" else "mqs"


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


def build_args(onefile: bool, reuse_build_dir: bool) -> list[str]:
    args = [
        sys.executable,
        "-m",
        "nuitka",
        "--assume-yes-for-downloads",
        f"--output-dir={OUT}",
        "--output-filename=mqs",
        # The GUI is served from web/dist next to the executable.
        f"--include-data-dir={WEB_DIST}=web/dist",
    ]
    if not reuse_build_dir:
        args.append("--remove-output")
    args.append("--onefile" if onefile else "--standalone")
    # SQL migrations are read via mqs.db.schema_path() at runtime. They must not
    # live in a directory called `mqs`: in a standalone build that name is taken
    # by the executable, and Nuitka refuses ("data file ... conflicts with
    # executable 'mqs'").
    for schema in sorted((ROOT / "src" / "mqs").glob("schema*.sql")):
        args.append(f"--include-data-files={schema}=mqs_data/{schema.name}")
    # `--include-package=tzdata` carries the packaged zoneinfo database along
    # with the modules (Nuitka 4 keeps package data), which is the only way a
    # Windows build can resolve ZoneInfo("Asia/Shanghai").
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


def locate_dist_dir() -> Path:
    """Return the standalone output directory, renamed to ``mqs.dist``.

    Nuitka names the folder after the entry point (``mqs_entry.dist``) on some
    versions and after ``--output-filename`` on others; both are accepted and
    normalised so the desktop shell and CI always look at one path.
    """
    dist = OUT / DIST_DIR_NAME
    if dist.is_dir():
        return dist
    candidates = sorted(p for p in OUT.glob("*.dist") if p.is_dir())
    if len(candidates) != 1:
        raise SystemExit(f"build failed: expected one *.dist directory under {OUT}, found {candidates}")
    candidates[0].rename(dist)
    return dist


def directory_size(path: Path) -> int:
    return sum(p.stat().st_size for p in path.rglob("*") if p.is_file())


def main() -> int:
    parser = argparse.ArgumentParser(description="Build the MQS sidecar with Nuitka")
    parser.add_argument(
        "--onefile",
        action="store_true",
        help="single-file build (extracts ~200 MiB into TMPDIR at every launch; not used by the installer)",
    )
    parser.add_argument(
        "--reuse-build-dir",
        action="store_true",
        help="keep Nuitka's intermediate build directory so a local rebuild only recompiles changed modules",
    )
    options = parser.parse_args()

    if not (WEB_DIST / "index.html").exists():
        print("web/dist is missing; run: cd web && npm ci && npm run build", file=sys.stderr)
        return 1

    try:
        import nuitka  # noqa: F401
    except ImportError:
        print("Nuitka is required: pip install '.[build]'", file=sys.stderr)
        return 1

    if not options.reuse_build_dir:
        shutil.rmtree(OUT, ignore_errors=True)
    OUT.mkdir(parents=True, exist_ok=True)
    for stale in OUT.glob("*.dist"):
        shutil.rmtree(stale, ignore_errors=True)

    subprocess.run(build_args(options.onefile, options.reuse_build_dir), cwd=ROOT, check=True)

    if options.onefile:
        binary = OUT / EXE_NAME
        if not binary.exists():
            print(f"build failed: {binary} missing", file=sys.stderr)
            return 1
        print(f"built {binary} ({binary.stat().st_size / 1024 / 1024:.1f} MiB)")
        return 0

    dist = locate_dist_dir()
    binary = dist / EXE_NAME
    if not binary.exists():
        print(f"build failed: {binary} missing", file=sys.stderr)
        return 1
    # The intermediate compilation directory is hundreds of MiB and is useless
    # once the dist folder is complete.
    for leftover in OUT.glob("*.build"):
        shutil.rmtree(leftover, ignore_errors=True)
    print(f"built {dist} ({directory_size(dist) / 1024 / 1024:.1f} MiB, executable {binary.name})")
    if sys.platform != "win32" and not binary.stat().st_mode & 0o111:
        print(f"build failed: {binary} is not executable", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
