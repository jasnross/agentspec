"""The discrimination rule, enforced over every committed sample.

Each package's samples must declare exactly the expressions its definition carries, each
declared result must be what the current evaluator yields over the sample's input, and
across a package's samples every expression must take both outcomes. So an assertion,
`wait_for` or custom check made constant by a later edit, to its own code or to a model it
reads, fails here. Every sample must also pass the leak scan.
"""

from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from probe_harness import json_values
from probe_harness.contract import (
    ExpectedAssertion,
    ManifestV2,
    ProjectionAssertionV1,
    Sample,
    parse_sample_file,
)
from probe_harness.families import ProbeDefinition, load_package, parse_sample
from probe_harness.samples import (
    Identity,
    evaluate,
    expression_names,
    leaks,
    sample_paths,
)

from tests.conftest import EXPERIMENTS_DIR


def _packages() -> Iterator[Path]:
    for entry in sorted(EXPERIMENTS_DIR.iterdir()):
        if entry.name != "lib" and not entry.name.startswith(".") and entry.is_dir():
            yield entry


SAMPLED = [p for p in _packages() if sample_paths(p)]
SAMPLE_FILES = [path for package in SAMPLED for path in sample_paths(package)]


def _loaded(package: Path) -> tuple[ProbeDefinition, Any, list[tuple[Path, Sample]]]:
    definition, manifest = load_package(package)
    return definition, manifest, [(path, parse_sample_file(path)) for path in sample_paths(package)]


@pytest.mark.parametrize("package", SAMPLED, ids=lambda p: p.name)
def test_each_sample_declares_exactly_the_definition_s_expressions(package: Path) -> None:
    definition, manifest, samples = _loaded(package)
    want = expression_names(definition, manifest)
    for path, sample in samples:
        assert set(sample.results.model_fields_set) == want, path.name


@pytest.mark.parametrize("package", SAMPLED, ids=lambda p: p.name)
def test_each_declared_result_is_reproduced_by_the_current_evaluator(package: Path) -> None:
    definition, manifest, samples = _loaded(package)
    for path, sample in samples:
        value = parse_sample(definition, sample.input, str(path))
        produced = evaluate(definition, manifest, value, sample.input).model_dump(mode="json")
        declared = sample.results.model_dump(mode="json")
        assert produced.keys() == declared.keys(), path.name
        for key in declared:
            assert json_values.equal(produced[key], declared[key]), (
                f"{path.name}: {key} declares {json_values.to_text(declared[key])} but the "
                f"evaluator yields {json_values.to_text(produced[key])}"
            )


@pytest.mark.parametrize("package", SAMPLED, ids=lambda p: p.name)
def test_every_expression_takes_both_outcomes_across_the_package_s_samples(package: Path) -> None:
    definition, manifest, samples = _loaded(package)
    results = [sample.results.model_dump(mode="json") for _, sample in samples]
    names = expression_names(definition, manifest)
    if "assertion" in names and isinstance(
        manifest.assertion, ProjectionAssertionV1 | ExpectedAssertion
    ):
        expected = manifest.assertion.expected
        outcomes = {json_values.equal(r["assertion"], expected) for r in results}
        assert outcomes == {True, False}, (
            "the assertion needs a sample equal to expected and one not"
        )
    if "wait_for" in names:
        assert {r["wait_for"] for r in results} == {True, False}, "wait_for needs both outcomes"
    if "capture_version" in names:
        assert any(r["capture_version"] is not None for r in results), (
            "capture_version needs a sample yielding a version"
        )
    if "custom_checks" in names:
        for check in results[0]["custom_checks"]:
            seen = {r["custom_checks"][check] for r in results}
            assert seen == {True, False}, f"custom check {check} needs both outcomes"


@pytest.mark.parametrize("package", list(_packages()), ids=lambda p: p.name)
def test_every_schema_2_package_has_samples(package: Path) -> None:
    manifest = package / "probe.json"
    if not manifest.is_file():
        pytest.skip("no manifest")
    _, parsed = load_package(package)
    if isinstance(parsed, ManifestV2):
        assert sample_paths(package), f"{package.name} is schema 2 but has no samples/"


@pytest.mark.parametrize("path", SAMPLE_FILES, ids=lambda p: f"{p.parent.parent.name}/{p.name}")
def test_no_sample_carries_personal_data(path: Path) -> None:
    assert leaks(path.read_text(encoding="utf-8"), Identity.current()) == []
