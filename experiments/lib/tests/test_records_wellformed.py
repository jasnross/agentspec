"""Every committed record and manifest satisfies the contract.

Read-only over the real tree. The extra-key half matters as much as the missing-key half:
six fields were removed during planning, and without a test rejecting them they return one
record at a time.

Each check reproduces the jq expression the bats suite applied, so a value on which that
expression would error or yield false fails here too. Types are therefore tested
explicitly: in jq, `true` is not `1`, and indexing a non-object is an error rather than a
missing key.
"""

import json
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

from probe_harness.contract import Depth, Driver, Provider

from tests.conftest import EXPERIMENTS_DIR, RECORD_FORBIDDEN_KEYS


class _JqError(Exception):
    """Where the bats suite's jq expression would have raised a runtime error."""


def test_every_committed_record_satisfies_the_seven_key_contract(
    record_wellformed: Callable[[Path], None],
) -> None:
    # Deliberately vacuous on an empty tree, which keeps the loop well-defined while a
    # package is being authored. The contract requires every probe package to carry at
    # least one record.
    for record in _records():
        record_wellformed(record)


def test_every_committed_manifest_parses_and_declares_its_required_fields() -> None:
    # Scoped to packages that have a manifest. Every probe package has one; the guard
    # covers a package mid-authoring, before its manifest is written.
    for path in _manifests():
        manifest = _load_manifest(path)

        assert isinstance(manifest, dict), f"manifest missing a required field: {path}"
        question = manifest.get("question")
        assertion = manifest.get("assertion")
        assert (
            isinstance(question, str)
            and len(question) > 0
            and _is_member(manifest.get("provider"), {p.value for p in Provider})
            and isinstance(assertion, dict)
            and "expected" in assertion
            and (("projection" in assertion) != ("options" in assertion))
        ), f"manifest missing a required field: {path}"

        # Their own checks rather than clauses in the conjunction above: a retired depth
        # or driver is a present, well-typed field, so the missing-field diagnostic would
        # send the reader looking for an absent key.
        depth = manifest.get("depth")
        assert depth is None or _is_member(depth, {d.value for d in Depth}), (
            f"manifest declares a depth the contract cannot gather: {path} ({_compact(depth)})"
        )

        driver = manifest.get("driver")
        assert _is_member(driver, {d.value for d in Driver}), (
            f"manifest declares a driver outside the scheduling enum: {path} ({_compact(driver)})"
        )


def test_no_option_declares_a_status_other_than_inconclusive() -> None:
    # A declared status replaces the comparison. Only `inconclusive` is a legitimate use
    # of that; anything else fixes the verdict in advance.
    for path in _manifests():
        manifest = _load_manifest(path)
        try:
            options = _optional_iterate(_index(_index(manifest, "assertion"), "options"))
            holds = all(
                _jq_equal(option["status"], "inconclusive")
                for option in options
                if isinstance(option, dict) and "status" in option
            )
        except _JqError:
            holds = False
        assert holds, f"manifest declares an option status other than inconclusive: {path}"


def test_every_options_manifest_carries_exactly_one_inconclusive_option() -> None:
    # Without a "couldn't tell" option a probe forces a binary, and a tired operator
    # picking the first plausible answer manufactures a pass.
    for path in _options_manifests():
        manifest = _load_manifest(path)
        try:
            inconclusive = [
                option
                for option in _iterate(_index(_index(manifest, "assertion"), "options"))
                if _jq_equal(_index(option, "status"), "inconclusive")
            ]
            holds = len(inconclusive) == 1
        except _JqError:
            holds = False
        assert holds, f"options manifest lacks exactly one inconclusive option: {path}"


def test_every_options_manifest_s_expected_names_one_of_its_own_option_ids() -> None:
    # A typo here would make the probe permanently unpassable, and it would only surface
    # during a live session, the most expensive place to find it.
    #
    # Scoped by assertion shape on purpose: a projection manifest's `expected` is an
    # arbitrary JSON value with no `options` to name.
    for path in _options_manifests():
        manifest = _load_manifest(path)
        try:
            assertion = _index(manifest, "assertion")
            expected = _index(assertion, "expected")
            # `any/2` stops at the first match, so an error after it never surfaces.
            holds = False
            for option in _iterate(_index(assertion, "options")):
                if _jq_equal(_index(option, "id"), expected):
                    holds = True
                    break
        except _JqError:
            holds = False
        assert holds, f"options manifest expected names no existing option id: {path}"


def test_every_options_manifest_declares_a_manual_driver() -> None:
    # The runner reads the assertion shape to decide whether to prompt. This is what makes
    # that read safe on committed data; the recorder gates it at write time.
    for path in _manifests():
        manifest = _load_manifest(path)
        try:
            holds = (
                _jq_equal(_index(manifest, "driver"), "manual")
                if _has(_index(manifest, "assertion"), "options")
                else True
            )
        except _JqError:
            holds = False
        assert holds, f"manifest declares options without a manual driver: {path}"


def test_every_manual_manifest_declares_a_wait_for_filter() -> None:
    # The runner polls against it; without one the run burns its full timeout.
    for path in _manifests():
        manifest = _load_manifest(path)
        if _jq_raw(manifest.get("driver") if isinstance(manifest, dict) else None) != "manual":
            continue

        wait_for = manifest.get("wait_for")
        assert isinstance(wait_for, str) and len(wait_for) > 0, (
            f"manual manifest declares no wait_for: {path}"
        )


def test_every_probe_package_is_a_measurement() -> None:
    # The contract's discovery command is `jq -r '.question' experiments/*/probe.json`, and
    # nothing else enforces it: the run loop and the status report each pass over a
    # manifest-less directory silently, so an unbacked package would be invisible to all
    # three at once. Every other check here is scoped to packages with a manifest; this one
    # says there are no others.
    for package in _packages():
        assert (package / "probe.json").is_file(), (
            f"package has no manifest, so nothing discovers it: {package}"
        )
        assert (package / "probe.py").is_file(), f"package has no runner (probe.py): {package}"
        assert list((package / "results").glob("*.json")), (
            f"package has never recorded a measurement: {package}"
        )


def test_every_package_with_a_manifest_has_an_executable_runner() -> None:
    # A package's runner is its `probe.py` definition, which `experiments/probe run` loads.
    for manifest in _manifests():
        assert (manifest.parent / "probe.py").is_file(), (
            f"package has a manifest but no probe.py: {manifest.parent}"
        )


def test_no_committed_record_carries_a_field_removed_during_planning() -> None:
    for path in _records():
        try:
            record = json.loads(path.read_text(encoding="utf-8"))
        except ValueError:
            record = None
        for key in RECORD_FORBIDDEN_KEYS:
            # `has` on a non-object is a jq error, which never prints `false`.
            assert isinstance(record, dict) and key not in record, (
                f"record {path} carries forbidden key {key}"
            )


def _packages() -> Iterator[Path]:
    """Every package directory: `experiments/*/`, minus `lib` and dot-directories."""
    for entry in sorted(EXPERIMENTS_DIR.iterdir()):
        if entry.name == "lib" or entry.name.startswith(".") or not entry.is_dir():
            continue
        yield entry


def _manifests() -> Iterator[Path]:
    for package in _packages():
        manifest = package / "probe.json"
        if manifest.exists():
            yield manifest


def _options_manifests() -> Iterator[Path]:
    for path in _manifests():
        manifest = _load_manifest(path)
        try:
            if _has(_index(manifest, "assertion"), "options"):
                yield path
        except _JqError:
            continue


def _records() -> Iterator[Path]:
    for package in _packages():
        yield from sorted((package / "results").glob("*.json"))


def _load_manifest(path: Path) -> Any:
    """The manifest's root, failing as `jq -e .` would on invalid JSON or a falsy root."""
    try:
        root = json.loads(path.read_text(encoding="utf-8"))
    except ValueError:
        root = None
    assert root is not None and root is not False, f"manifest is not valid JSON: {path}"
    return root


def _index(value: Any, key: str) -> Any:
    """jq's `.key`: `null` on `null`, the member on an object, an error on anything else."""
    if value is None:
        return None
    if isinstance(value, dict):
        return value.get(key)
    raise _JqError(f"cannot index {type(value).__name__} with {key!r}")


def _has(value: Any, key: str) -> bool:
    """jq's `has(key)`, which errors on anything but an object."""
    if isinstance(value, dict):
        return key in value
    raise _JqError(f"cannot check whether {type(value).__name__} has a key")


def _iterate(value: Any) -> list[Any]:
    """jq's `.[]`: an array's elements, an object's values, and an error otherwise."""
    if isinstance(value, list):
        return value
    if isinstance(value, dict):
        return list(value.values())
    raise _JqError(f"cannot iterate over {type(value).__name__}")


def _optional_iterate(value: Any) -> list[Any]:
    """jq's `.[]?`: as `.[]`, but nothing where `.[]` would error."""
    try:
        return _iterate(value)
    except _JqError:
        return []


def _jq_equal(a: Any, b: Any) -> bool:
    """jq's `==`, where booleans and numbers never compare equal."""
    if isinstance(a, bool) or isinstance(b, bool):
        return type(a) is type(b) and a == b
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(_jq_equal(x, y) for x, y in zip(a, b, strict=True))
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(_jq_equal(a[k], b[k]) for k in a)
    if isinstance(a, int | float) and isinstance(b, int | float):
        return a == b
    return type(a) is type(b) and a == b


def _is_member(value: Any, members: set[str]) -> bool:
    return isinstance(value, str) and value in members


def _jq_raw(value: Any) -> str:
    """What `jq -r` prints for one value."""
    return value if isinstance(value, str) else json.dumps(value)


def _compact(value: Any) -> str:
    return json.dumps(value, separators=(",", ":"))
