"""The source map: the git blob id of every file that affects how a run is evaluated.

A run writes the map its preflight read into `<ws>/source.json`, so a workspace judged
again later can say which code produced its capture. Ids hash the working content (`git
hash-object`) rather than reading `HEAD`, so a run with uncommitted changes names what
actually ran; on a clean tree the two agree.

Covered: the package's `probe.json`, its `probe.py`, and every file under its `fixtures/`,
and the harness itself — every file under `lib/probe_harness/` and `lib/stubs/`, plus
`probe`, `pyproject.toml` and `uv.lock`. Each id is keyed by its path relative to the parent
of its own experiments directory, which in the real tree is the repository-relative path.
"""

import subprocess
from collections.abc import Iterator, Mapping, Sequence
from pathlib import Path

from probe_harness import json_values

# `experiments/`, the directory holding the harness.
HARNESS_ROOT = Path(__file__).resolve().parents[2]


class SourceMapError(Exception):
    """A git command the source map needs failed. `lines` name the command and its stderr."""

    def __init__(self, *lines: str) -> None:
        super().__init__("\n".join(lines))
        self.lines = lines


def source_map(package: Path) -> dict[str, str]:
    """The blob id of every covered file, keyed by path and sorted by key."""
    package = package.resolve()
    covered: list[tuple[Path, Path]] = []
    for base, files in (
        (
            package.parent.parent,
            [package / "probe.json", package / "probe.py", *_files_under(package / "fixtures")],
        ),
        (
            HARNESS_ROOT.parent,
            [
                *_files_under(HARNESS_ROOT / "lib" / "probe_harness"),
                *_files_under(HARNESS_ROOT / "lib" / "stubs"),
                HARNESS_ROOT / "probe",
                HARNESS_ROOT / "pyproject.toml",
                HARNESS_ROOT / "uv.lock",
            ],
        ),
    ):
        covered += ((base, path) for path in files if _regular(path))
    ids = _hash_objects([path for _, path in covered])
    if len(ids) != len(covered):
        raise SourceMapError(
            f"could not compute the source map: git hash-object returned {len(ids)} ids "
            f"for {len(covered)} files"
        )
    return dict(
        sorted(
            (path.relative_to(base).as_posix(), blob)
            for (base, path), blob in zip(covered, ids, strict=True)
        )
    )


def write_source(ws: Path, source: Mapping[str, str]) -> None:
    """Write <source> to `<ws>/source.json`."""
    (ws / "source.json").write_text(json_values.dumps(dict(source)) + "\n", encoding="utf-8")


def _files_under(directory: Path) -> list[Path]:
    """Every file under <directory>: git's view of it inside a work tree, a walk outside one.

    Inside a work tree, tracked files and untracked files git does not ignore count, so a
    fixture not yet committed is covered and `__pycache__` is not. A file git ignores is
    not covered even though a run may copy it.
    """
    if not directory.is_dir():
        return []
    if _git(["rev-parse", "--is-inside-work-tree"], directory, check=False) == "true\n":
        listed = _git(
            ["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "."],
            directory,
        )
        return sorted(directory / name for name in listed.split("\0") if name)
    return sorted(_walk(directory))


def _walk(directory: Path) -> Iterator[Path]:
    for path in directory.rglob("*"):
        if "__pycache__" in path.relative_to(directory).parts:
            continue
        yield path


def _regular(path: Path) -> bool:
    return path.is_file() and not path.is_symlink()


def _hash_objects(paths: Sequence[Path]) -> list[str]:
    if not paths:
        return []
    out = _git(
        ["hash-object", "--stdin-paths"], HARNESS_ROOT, stdin="".join(f"{p}\n" for p in paths)
    )
    return out.split()


def _git(args: list[str], cwd: Path, *, check: bool = True, stdin: str | None = None) -> str:
    """git's stdout; with <check>, raise `SourceMapError` when it exits nonzero."""
    command = ["git", *args]
    completed = subprocess.run(
        command,
        cwd=cwd,
        input=stdin,
        stdin=subprocess.DEVNULL if stdin is None else None,
        capture_output=True,
        text=True,
        # A name that is not UTF-8 then fails `hash-object` as a `SourceMapError`.
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if check and completed.returncode != 0:
        raise SourceMapError(
            f"could not compute the source map: {' '.join(command)} exited "
            f"{completed.returncode} in {cwd}",
            *completed.stderr.rstrip("\n").splitlines(),
        )
    return completed.stdout if completed.returncode == 0 else ""
