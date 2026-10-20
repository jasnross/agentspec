"""Every committed record and manifest satisfies the contract.

Read-only over the real tree. The extra-key half matters as much as the missing-key half:
six fields were removed during planning, and without a test rejecting them they return one
record at a time. Manifests are read through `parse_manifest`, whose gates
`test_contract.py` runs over every committed manifest, so the checks here are the ones
those gates do not make.
"""

import json
from collections.abc import Callable, Iterator
from pathlib import Path

from probe_harness import json_values
from probe_harness.contract import (
    Driver,
    Manifest,
    OptionsAssertion,
    parse_manifest,
    parse_record,
)

from tests.conftest import (
    EXPERIMENTS_DIR,
    RECORD_FORBIDDEN_KEYS,
)


def test_every_committed_record_satisfies_the_seven_key_contract(
    record_wellformed: Callable[[Path], None],
) -> None:
    # Deliberately vacuous on an empty tree, which keeps the loop well-defined while a
    # package is being authored. The contract requires every probe package to carry at
    # least one record.
    for record in _records():
        record_wellformed(record)


def test_every_committed_record_is_written_as_the_recorder_writes_it() -> None:
    for path in _records():
        text = path.read_text(encoding="utf-8")
        assert json_values.dumps(json_values.loads(text)) + "\n" == text, (
            f"record {path} is not in the recorder's format"
        )


def test_every_committed_record_parses_as_a_record() -> None:
    # The byte comparison is what pins the model's field order to the committed format.
    for path in _records():
        parsed = parse_record(path)
        assert parsed is not None, f"record {path} does not satisfy the record model"
        assert path.read_text(encoding="utf-8") == (
            json_values.dumps(parsed.model_dump(mode="json")) + "\n"
        ), f"record {path} is not what the record model writes"


def test_every_options_manifest_carries_exactly_one_inconclusive_option() -> None:
    # Without a "couldn't tell" option a probe forces a binary, and a tired operator
    # picking the first plausible answer manufactures a pass.
    for path, assertion in _options_manifests():
        inconclusive = [o for o in assertion.options if o.status == "inconclusive"]
        assert len(inconclusive) == 1, (
            f"options manifest lacks exactly one inconclusive option: {path}"
        )


def test_every_options_manifest_s_expected_names_one_of_its_own_option_ids() -> None:
    # A typo here would make the probe permanently unpassable, and it would only surface
    # during a live session, the most expensive place to find it.
    #
    # Scoped by assertion shape on purpose: a projection manifest's `expected` is an
    # arbitrary JSON value with no `options` to name.
    for path, assertion in _options_manifests():
        assert any(json_values.equal(o.id, assertion.expected) for o in assertion.options), (
            f"options manifest expected names no existing option id: {path}"
        )


def test_every_manual_manifest_declares_a_wait_for_filter() -> None:
    # The runner polls against it; without one the run burns its full timeout.
    for path in _manifests():
        manifest = parse_manifest(path)
        if manifest.driver == Driver.MANUAL:
            assert manifest.wait_for is not None, f"manual manifest declares no wait_for: {path}"


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
        record = json.loads(path.read_text(encoding="utf-8"))
        assert isinstance(record, dict), f"record {path} is not a JSON object"
        for key in RECORD_FORBIDDEN_KEYS:
            assert key not in record, f"record {path} carries forbidden key {key}"


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


def _options_manifests() -> Iterator[tuple[Path, OptionsAssertion]]:
    for path in _manifests():
        manifest: Manifest = parse_manifest(path)
        if isinstance(manifest.assertion, OptionsAssertion):
            yield path, manifest.assertion


def _records() -> Iterator[Path]:
    for package in _packages():
        yield from sorted((package / "results").glob("*.json"))
