"""The source map: the git blob id of every file that affects how a run is evaluated.

A run writes the map its preflight read into `<ws>/source.json`, so a workspace judged
again later can say which code produced its capture. Ids hash the working content (`git
hash-object`) rather than reading `HEAD`, so a run with uncommitted changes names what
actually ran; on a clean tree the two agree.

Covered: the package's `probe.json`, its `probe.py`, and every file under its `fixtures/`,
and the harness itself — every file under `lib/probe_harness/` and `lib/stubs/`, plus
`probe`, `pyproject.toml` and `uv.lock`. Each id is keyed by its path relative to the parent
of its own experiments directory, which in the real tree is the repository-relative path.

A v2 record carries the map, so its ids must name committed content: the clean-tree gate
(`clean_tree_refusal`) refuses a real v2 recording whose map differs from `HEAD`'s, and
`materialize` writes a record's files back out of the object store.
"""

import re
import subprocess
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from probe_harness import json_values
from probe_harness.contract import RecordV2

# `experiments/`, the directory holding the harness.
HARNESS_ROOT = Path(__file__).resolve().parents[2]


class SourceMapError(Exception):
    """A git command the source map needs failed. `lines` name the command and its stderr."""

    def __init__(self, *lines: str) -> None:
        super().__init__("\n".join(lines))
        self.lines = lines


@dataclass(frozen=True)
class _Base:
    """A directory the map keys paths against, and the covered roots under it.

    Each root is a file or a directory whose every file is covered.
    """

    directory: Path
    roots: tuple[Path, ...]

    def keys(self, source: Mapping[str, str]) -> dict[str, str]:
        """The entries of <source> that fall under this base's roots."""
        prefixes = [root.relative_to(self.directory).as_posix() for root in self.roots]
        return {
            key: blob
            for key, blob in source.items()
            if any(key == prefix or key.startswith(f"{prefix}/") for prefix in prefixes)
        }


def source_map(package: Path) -> dict[str, str]:
    """The blob id of every covered file, keyed by path and sorted by key."""
    covered: list[tuple[Path, Path]] = []
    for base in _bases(package):
        files: list[Path] = []
        for root in base.roots:
            files += _files_under(root) if root.is_dir() else [root]
        covered += ((base.directory, path) for path in files if _regular(path))
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


def read_source(ws: Path) -> dict[str, str] | None:
    """The map a run wrote to `<ws>/source.json`; `None` when absent or not such a map."""
    try:
        document = json_values.loads((ws / "source.json").read_text(encoding="utf-8"))
    except ValueError, OSError:
        return None
    if not isinstance(document, dict):
        return None
    source = {key: value for key, value in document.items() if isinstance(value, str)}
    if len(source) != len(document) or not all(map(_BLOB_ID.fullmatch, source.values())):
        return None
    return source


_BLOB_ID = re.compile(r"[0-9a-f]{40}([0-9a-f]{24})?")

# How many differing paths a clean-tree refusal names before summarizing the rest.
_REFUSAL_PATHS = 10


def clean_tree_refusal(source: Mapping[str, str], package: Path) -> tuple[str, ...] | None:
    """Why <source> cannot be recorded: lines naming the paths that differ from `HEAD`.

    Each base is compared against `HEAD` in its own repository, over its covered roots: a
    path added, removed or changed since the last commit refuses. A package outside any git
    work tree has no `HEAD` and records nothing that could be committed, so the gate is
    skipped for it; otherwise a base outside any work tree refuses.
    """
    package = package.resolve()
    if not _inside_work_tree(package):
        return None
    differing: list[str] = []
    for base in _bases(package):
        if not _inside_work_tree(base.directory):
            return (f"{base.directory} is not inside a git work tree, so its files have no commit",)
        committed = _committed(base)
        working = base.keys(source)
        for key in sorted(committed.keys() | working.keys()):
            if key not in committed:
                differing.append(f"  uncommitted: {key}")
            elif key not in working:
                differing.append(f"  deleted: {key}")
            elif committed[key] != working[key]:
                differing.append(f"  modified: {key}")
    if not differing:
        return None
    shown = differing[:_REFUSAL_PATHS]
    rest = len(differing) - len(shown)
    return (
        "the probe's source differs from HEAD, so a record could not name the code that "
        "judged it; commit these files and re-run:",
        *shown,
        *((f"  … and {rest} more",) if rest else ()),
    )


def materialize(record: RecordV2, record_path: Path, out_dir: Path, *, arranged: bool) -> list[str]:
    """Write each file <record> names to `<out_dir>/<path>`, out of the record's repository.

    With <arranged>, the files that arranged the workspace; a record carrying no `arranged`
    was arranged by the code that judged it. Refuses with `FileExistsError` before writing
    anything when a target exists, and with `ValueError` for a path that would leave
    <out_dir>. Returns the paths whose objects are missing, having written every other.
    """
    files = recorded_files(record, arranged=arranged)
    for key in files:
        relative = Path(key)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"{record_path} names a path outside the repository: {key}")
        target = out_dir / relative
        # A dangling symlink does not exist, but writing through it would leave <out_dir>.
        if target.exists() or target.is_symlink():
            raise FileExistsError(target)
    repository = record_path.resolve().parent
    missing: list[str] = []
    for key, blob in files.items():
        content = _object(blob, repository)
        if content is None:
            missing.append(key)
            continue
        target = out_dir / key
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
    return missing


def recorded_files(record: RecordV2, *, arranged: bool) -> dict[str, str]:
    """The files <record> names: those that judged it, or with <arranged> those that arranged
    its workspace, which are the same files when it carries no `arranged`."""
    if arranged and isinstance(record.arranged, dict):
        return record.arranged
    return record.source


def _bases(package: Path) -> tuple[_Base, _Base]:
    """The package's base and the harness's, each with the roots it covers."""
    package = package.resolve()
    lib = HARNESS_ROOT / "lib"
    return (
        _Base(
            package.parent.parent,
            (package / "probe.json", package / "probe.py", package / "fixtures"),
        ),
        _Base(
            HARNESS_ROOT.parent,
            (
                lib / "probe_harness",
                lib / "stubs",
                HARNESS_ROOT / "probe",
                HARNESS_ROOT / "pyproject.toml",
                HARNESS_ROOT / "uv.lock",
            ),
        ),
    )


def _inside_work_tree(directory: Path) -> bool:
    # A directory that does not exist yet is judged by its nearest existing ancestor.
    while not directory.is_dir() and directory != directory.parent:
        directory = directory.parent
    return _git(["rev-parse", "--is-inside-work-tree"], directory, check=False) == "true\n"


def _committed(base: _Base) -> dict[str, str]:
    """`HEAD`'s blob id for every regular file under <base>'s roots, keyed as the map keys it."""
    directory = base.directory
    if not _git(["rev-parse", "--verify", "--quiet", "HEAD"], directory, check=False):
        raise SourceMapError(
            f"the repository holding {directory} has no commit, so a record could not name "
            "the code that judged it; commit the probe and re-run"
        )
    prefix = _git(["rev-parse", "--show-prefix"], directory).rstrip("\n")
    specs = [f"{prefix}{root.relative_to(directory).as_posix()}" for root in base.roots]
    listed = _git(["ls-tree", "-r", "-z", "--full-tree", "HEAD", "--", *specs], directory)
    committed: dict[str, str] = {}
    for entry in listed.split("\0"):
        if not entry:
            continue
        meta, path = entry.split("\t", 1)
        mode, kind, blob = meta.split()
        # The map covers regular files only, so a committed symlink or submodule is not one.
        if kind == "blob" and mode != "120000":
            committed[path.removeprefix(prefix)] = blob
    return committed


def _object(blob: str, repository: Path) -> bytes | None:
    """The content of blob <blob> in the repository holding <repository>; `None` if missing."""
    completed = subprocess.run(
        ["git", "cat-file", "blob", blob],
        cwd=repository,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        check=False,
    )
    return completed.stdout if completed.returncode == 0 else None


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
