"""Check the built sdist/wheel before they leave CI for PyPI.

A wheel that was built before ``npm run build`` installs fine and answers the
whole API — the only broken part is the control GUI, which silently falls back
to the "前端尚未构建" page. This gate makes that failure loud, and also checks
the license file and the version, because none of those can be fixed after a
release is uploaded.

Usage:
    python tools/check_dist.py                 # newest dist/*.whl + *.tar.gz
    python tools/check_dist.py dist/*.whl
"""

from __future__ import annotations

import argparse
import sys
import tarfile
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class DistError(RuntimeError):
    pass


def project_version() -> str:
    with open(ROOT / "pyproject.toml", "rb") as handle:
        return tomllib.load(handle)["project"]["version"]


def check_wheel(path: Path, version: str) -> str:
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
        metadata_name = next((n for n in names if n.endswith(".dist-info/METADATA")), None)
        if metadata_name is None:
            raise DistError(f"{path.name} has no dist-info/METADATA")
        metadata = archive.read(metadata_name).decode("utf-8", "replace")

    if "mqs/web/dist/index.html" not in names:
        raise DistError(f"{path.name} does not contain the built GUI (mqs/web/dist/index.html)")
    if not any(n.endswith(".dist-info/licenses/LICENSE") for n in names):
        raise DistError(f"{path.name} does not contain LICENSE")
    if "License-Expression: AGPL-3.0-or-later" not in metadata:
        raise DistError(f"{path.name} is missing the SPDX license expression")
    if f"Version: {version}" not in metadata:
        raise DistError(f"{path.name} is not version {version}")
    return f"wheel ok: {path.name} ({len(names)} files, GUI + LICENSE + {version})"


def check_sdist(path: Path, version: str) -> str:
    with tarfile.open(path) as archive:
        names = archive.getnames()
    prefix = f"{path.name.removesuffix('.tar.gz')}/"
    if not any(n == prefix + "web/dist/index.html" for n in names):
        raise DistError(f"{path.name} does not contain web/dist/index.html")
    if not any(n.startswith(prefix + "src/mqs/") for n in names):
        raise DistError(f"{path.name} does not contain the source package")
    if not any(n == prefix + "PKG-INFO" for n in names):
        raise DistError(f"{path.name} has no PKG-INFO")
    return f"sdist ok: {path.name} ({len(names)} files)"


def main() -> int:
    parser = argparse.ArgumentParser(description="Verify the built MQS sdist/wheel")
    parser.add_argument("artifacts", nargs="*", type=Path, help="dist artifacts (default: newest in dist/)")
    options = parser.parse_args()

    try:
        version = project_version()
        if options.artifacts:
            wheel = next((p for p in options.artifacts if p.suffix == ".whl"), None)
            sdist = next((p for p in options.artifacts if p.name.endswith(".tar.gz")), None)
        else:
            wheels = sorted((ROOT / "dist").glob("*.whl"), key=lambda p: p.stat().st_mtime, reverse=True)
            sdists = sorted((ROOT / "dist").glob("*.tar.gz"), key=lambda p: p.stat().st_mtime, reverse=True)
            wheel = wheels[0] if wheels else None
            sdist = sdists[0] if sdists else None
        if wheel is None or sdist is None:
            raise DistError("need both a wheel and an sdist in dist/ (run `python -m build`)")
        print(check_wheel(wheel, version))
        print(check_sdist(sdist, version))
    except (DistError, OSError, KeyError) as exc:
        print(f"dist check failed: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
