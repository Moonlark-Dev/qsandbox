"""Hatch build hook: ship the built GUI in the wheel, but do not demand it.

The frontend lives in ``web/`` and is generated into ``web/dist`` (gitignored),
so a plain ``pip install -e .`` on a fresh checkout has no ``web/dist`` at all.
A static ``force-include`` would make that editable install fail before the
user ever gets a chance to run ``npm run build``; the control GUI serving the
"前端尚未构建" page is the expected state of a source checkout, and a message
that says so is better than a packaging traceback.

Release builds are protected by ``tools/check_dist.py`` (and the wheel smoke
test), which fail when ``web/dist`` did not make it into the wheel.
"""

from __future__ import annotations

from pathlib import Path

from hatchling.builders.hooks.plugin.interface import BuildHookInterface


class CustomBuildHook(BuildHookInterface):
    PLUGIN_NAME = "custom"

    def initialize(self, version: str, build_data: dict) -> None:
        web_dist = Path(self.root) / "web" / "dist"
        if self.target_name != "wheel" or not (web_dist / "index.html").exists():
            return
        build_data["force_include"]["web/dist"] = "mqs/web/dist"
