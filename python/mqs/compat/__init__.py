"""SDK compatibility shims (Python-only, not part of the Rust binary)."""

from .botpy import patch, patch_from_runtime

__all__ = ["patch", "patch_from_runtime"]
