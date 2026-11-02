"""The isolated layout and environment OpenCode runs under, for any family that isolates it.

The `XDG_*` redirect keeps the operator's global config, plugins, and cache out of the run.
`HOME` is redirected too, because OpenCode also reads `~/.opencode/` as a config
directory, located from the home directory rather than from any `XDG_*` variable. Neither
redirect blocks the four config variables cleared here, each of which would inject config
(or tool rules directly) from the operator's shell, nor `OPENCODE_DISABLE_PROJECT_CONFIG`,
which would discard the fixture's own config and fail the run with no obvious cause.

Neither redirect stops OpenCode's external skill scan, which walks up from the project
directory for `.claude/skills` and `.agents/skills`. A workspace under the operator's home
reaches `~/.claude/skills` that way, so the scan is switched off: every skill a probe
arranges sits in the project's `.opencode/`, outside the scan. OpenCode's skills
documentation says the same walk reads an `.opencode/` directory above the project too, and
switching the scan off does not stop that, so an operator's `~/.opencode/skills/` would reach
a workspace under their home. Each OpenCode family therefore refuses to run while
`ancestor_config_paths` finds an `.opencode/` directory above the workspace root. An operator
who keeps one can move it, or set `XDG_STATE_HOME` to a directory outside their home;
agentspec TODO #68 tracks keeping it out without refusing the run.

Nor does either redirect stop OpenCode's instruction-file search. OpenCode 1.18.34 walks up
from the project to its git worktree, which is `/` for a non-git workspace, for
`INSTRUCTION_FILES`, and loads every match of the first name it finds (measured by
`opencode-ancestor-instructions`). An instruction file anywhere above the workspace would
therefore load into every run, so each OpenCode family refuses to run while
`ancestor_config_paths` finds one. The list is OpenCode 1.18.34's, read from the
filesystem rather than from OpenCode's own view, so a name a later OpenCode adds is missed
until the list changes.
"""

import os
from dataclasses import dataclass
from pathlib import Path

from probe_harness.gates import GateFailure

# The instruction files OpenCode 1.18.34 searches for above a project.
INSTRUCTION_FILES = ("AGENTS.md", "CLAUDE.md", "CONTEXT.md")

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
        env["OPENCODE_DISABLE_EXTERNAL_SKILLS"] = "1"
        return env


def ancestor_config_paths(root: Path) -> list[Path]:
    """Each of `INSTRUCTION_FILES`, and each `.opencode` directory, present in <root> or any
    of its ancestors, up to `/`.

    <root> need not exist yet, so the check can run before a workspace is made under it. A
    relative <root> is made absolute first, so its real ancestors are searched.
    """
    root = root.absolute()
    found: list[Path] = []
    for directory in (root, *root.parents):
        found += [directory / name for name in INSTRUCTION_FILES if (directory / name).is_file()]
        if (directory / _CONFIG_DIR).is_dir():
            found.append(directory / _CONFIG_DIR)
    return found


def ancestor_config_refusal(root: Path) -> GateFailure | None:
    """The refusal a run under <root> gets when `ancestor_config_paths` finds any."""
    found = ancestor_config_paths(root)
    if not found:
        return None
    return GateFailure(
        "an instruction file or .opencode directory sits above the workspace root, and "
        "OpenCode would load it into the run:",
        *(f"  {path}" for path in found),
        "move it, or set XDG_STATE_HOME to a directory with none above it. No workspace made.",
    )


_XDG_KINDS = ("config", "cache", "data", "state")

# The config directory OpenCode reads in the project and, for skills, above it.
_CONFIG_DIR = ".opencode"
