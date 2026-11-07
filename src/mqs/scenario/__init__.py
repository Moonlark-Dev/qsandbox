"""Scenario engine package."""

from __future__ import annotations

from pathlib import Path

import yaml

from ..domain.world import World
from .engine import ScenarioResult, ScenarioRunner, StepFailed, run_scenario_payload
from .templates import BUILTINS

REPO_ROOT = Path(__file__).resolve().parents[3]


def builtin_scenarios() -> list[dict]:
    items = []
    for name, text in BUILTINS.items():
        try:
            doc = yaml.safe_load(text) or {}
        except yaml.YAMLError as exc:  # pragma: no cover
            doc = {"description": str(exc)}
        items.append(
            {
                "name": name,
                "source": "builtin",
                "description": doc.get("description", ""),
                "steps": len(doc.get("steps") or []),
            }
        )
    return items


def list_scenario_files(world: World | None = None) -> list[dict]:
    roots = [REPO_ROOT / "scenarios"]
    if world is not None:
        roots.append(world.paths.directory / "scenarios")
    items = []
    for root in roots:
        if not root.exists():
            continue
        for path in sorted(root.glob("*.y*ml")):
            try:
                doc = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
            except (yaml.YAMLError, OSError):
                doc = {}
            items.append(
                {
                    "name": path.stem,
                    "source": "file",
                    "path": str(path),
                    "description": doc.get("description", ""),
                    "steps": len(doc.get("steps") or []),
                }
            )
    return items


def get_builtin(name: str) -> str | None:
    return BUILTINS.get(name)


__all__ = [
    "BUILTINS",
    "ScenarioResult",
    "ScenarioRunner",
    "StepFailed",
    "builtin_scenarios",
    "get_builtin",
    "list_scenario_files",
    "run_scenario_payload",
]
