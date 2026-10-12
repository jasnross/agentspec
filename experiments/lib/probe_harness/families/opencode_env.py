"""The isolated layout and environment OpenCode runs under, for any family that isolates it.

The `XDG_*` redirect keeps the operator's global config, plugins, and cache out of the run.
`HOME` is redirected too, because OpenCode also reads `~/.opencode/` as a config
directory, located from the home directory rather than from any `XDG_*` variable. Neither
redirect blocks the four config variables cleared here, each of which would inject config
(or tool rules directly) from the operator's shell, nor `OPENCODE_DISABLE_PROJECT_CONFIG`,
which would discard the fixture's own config and fail the run with no obvious cause.
"""

import os
from dataclasses import dataclass
from pathlib import Path

# Each would inject or discard config from the operator's shell.
CLEARED = (
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_PERMISSION",
    "OPENCODE_DISABLE_PROJECT_CONFIG",
)


@dataclass(frozen=True)
class IsolatedLayout:
    root: Path

    @property
    def project(self) -> Path:
        return self.root / "project"

    @property
    def home(self) -> Path:
        return self.root / "home"

    def xdg(self, kind: str) -> Path:
        return self.root / "xdg" / kind


@dataclass(frozen=True)
class OpenCodeIsolation:
    """Run OpenCode with its own home and XDG directories under the workspace."""

    def layout(self, directory: Path) -> IsolatedLayout:
        layout = IsolatedLayout(directory)
        for path in (layout.project, layout.home, *map(layout.xdg, _XDG_KINDS)):
            path.mkdir(parents=True, exist_ok=True)
        return layout

    def env(self, layout: IsolatedLayout) -> dict[str, str]:
        env = {k: v for k, v in os.environ.items() if k not in CLEARED}
        env["HOME"] = str(layout.home)
        for kind in _XDG_KINDS:
            env[f"XDG_{kind.upper()}_HOME"] = str(layout.xdg(kind))
        env["OPENCODE_DISABLE_AUTOUPDATE"] = "1"
        env["OPENCODE_DISABLE_MODELS_FETCH"] = "1"
        return env


_XDG_KINDS = ("config", "cache", "data", "state")
