"""`contract.parse_manifest`: each invalid manifest is refused, naming the invalid field's path."""

import json
from pathlib import Path
from typing import Any

import pytest
from probe_harness.contract import (
    CommandSource,
    ManifestError,
    NoSource,
    Option,
    OptionsAssertion,
    parse_manifest,
)

from tests.conftest import EXPERIMENTS_DIR

PROJECTION: dict[str, Any] = {
    "schema_version": 1,
    "provider": "opencode",
    "driver": "unattended",
    "depth": None,
    "question": "q",
    "version_source": {"kind": "none"},
    "assertion": {"projection": ".", "expected": 1},
}

OPTIONS: dict[str, Any] = {
    **PROJECTION,
    "provider": "cursor",
    "driver": "manual",
    "wait_for": "length > 0",
    "assertion": {
        "options": [
            {"id": "a", "text": "first"},
            {"id": "couldnt-tell", "text": "Could not determine", "status": "inconclusive"},
        ],
        "expected": "a",
    },
}


def _write(tmp_path: Path, document: Any) -> Path:
    path = tmp_path / "probe.json"
    path.write_text(json.dumps(document), encoding="utf-8")
    return path


def _refusal(tmp_path: Path, document: Any) -> list[str]:
    """The field lines of <document>'s refusal, after the line naming the manifest."""
    path = _write(tmp_path, document)
    with pytest.raises(ManifestError) as exc:
        parse_manifest(path)
    first, *fields = exc.value.lines
    assert first == f"manifest is invalid: {path}"
    assert str(exc.value) == "\n".join(exc.value.lines)
    assert all(line.startswith("  ") for line in fields)
    return [line.removeprefix("  ") for line in fields]


def _names(lines: list[str], field: str) -> bool:
    """Whether some refusal line is about <field>, written as its dotted path."""
    return any(line.startswith(f"{field}: ") for line in lines)


def _options(*options: Any) -> dict[str, Any]:
    return {**OPTIONS, "assertion": {"options": list(options), "expected": "a"}}


def test_a_missing_manifest_is_refused(tmp_path: Path) -> None:
    path = tmp_path / "probe.json"
    with pytest.raises(ManifestError) as exc:
        parse_manifest(path)
    assert exc.value.lines == (f"manifest not found: {path}",)


@pytest.mark.parametrize("text", ["{", '{"expected": NaN}', '{"expected": 1e400}'], ids=repr)
def test_a_manifest_that_is_not_json_is_refused(tmp_path: Path, text: str) -> None:
    path = tmp_path / "probe.json"
    path.write_text(text, encoding="utf-8")
    with pytest.raises(ManifestError) as exc:
        parse_manifest(path)
    assert exc.value.lines == (f"manifest is not valid JSON: {path}",)


def test_nan_in_expected_is_refused(tmp_path: Path) -> None:
    path = tmp_path / "probe.json"
    document = {**PROJECTION, "assertion": {"projection": ".", "expected": float("nan")}}
    path.write_text(json.dumps(document), encoding="utf-8")
    with pytest.raises(ManifestError) as exc:
        parse_manifest(path)
    assert exc.value.lines == (f"manifest is not valid JSON: {path}",)


@pytest.mark.parametrize("root", [[], 5, None, False, "text"], ids=repr)
def test_a_root_that_is_not_an_object_is_refused(tmp_path: Path, root: Any) -> None:
    path = _write(tmp_path, root)
    with pytest.raises(ManifestError) as exc:
        parse_manifest(path)
    assert exc.value.lines == (f"manifest must be a JSON object: {path}",)


@pytest.mark.parametrize("version", [1.0, True, "1", 2], ids=repr)
def test_schema_version_must_be_the_integer_1(tmp_path: Path, version: Any) -> None:
    document = {**PROJECTION, "schema_version": version}
    assert _names(_refusal(tmp_path, document), "schema_version")


@pytest.mark.parametrize("options", [5, [], {"id": "a"}, None], ids=repr)
def test_options_must_be_a_non_empty_list(tmp_path: Path, options: Any) -> None:
    document = {**OPTIONS, "assertion": {"options": options, "expected": "a"}}
    assert _names(_refusal(tmp_path, document), "assertion.options")


def test_an_empty_option_list_is_refused_in_the_author_s_words(tmp_path: Path) -> None:
    document = {**OPTIONS, "assertion": {"options": [], "expected": "a"}}
    assert _refusal(tmp_path, document) == ["assertion.options: must be a non-empty list"]


def test_a_misspelled_driver_is_reported_beside_a_malformed_option_set(tmp_path: Path) -> None:
    # Every invalid field is reported at once, so a retired driver is never mistaken for a
    # valid one that merely mismatches the option set.
    document = {**OPTIONS, "driver": "script", "assertion": {"options": 5, "expected": "a"}}
    lines = _refusal(tmp_path, document)
    assert _names(lines, "driver")
    assert _names(lines, "assertion.options")
    assert not any("requires a manual driver" in line for line in lines)


@pytest.mark.parametrize("projection", [5, "", None, ["."]], ids=repr)
def test_a_projection_must_be_a_non_empty_string(tmp_path: Path, projection: Any) -> None:
    document = {**PROJECTION, "assertion": {"projection": projection, "expected": 1}}
    assert _names(_refusal(tmp_path, document), "assertion.projection")


@pytest.mark.parametrize(
    ("option", "field"),
    [
        ("a", "assertion.options.1"),
        ({"id": "b", "text": "t", "label": "extra"}, "assertion.options.1.label"),
        ({"id": 2, "text": "t"}, "assertion.options.1.id"),
        ({"id": "", "text": "t"}, "assertion.options.1.id"),
        ({"text": "t"}, "assertion.options.1.id"),
        ({"id": "b", "text": None}, "assertion.options.1.text"),
        ({"id": "b"}, "assertion.options.1.text"),
    ],
    ids=repr,
)
def test_an_option_must_be_an_object_with_a_string_id_and_text(
    tmp_path: Path, option: Any, field: str
) -> None:
    document = _options({"id": "a", "text": "first"}, option)
    assert _names(_refusal(tmp_path, document), field)


def test_an_unknown_key_inside_an_option_is_refused_by_its_path(tmp_path: Path) -> None:
    document = _options({"id": "a", "text": "first", "colour": "red"})
    assert _refusal(tmp_path, document) == [
        "assertion.options.0.colour: Extra inputs are not permitted"
    ]


def test_an_option_may_declare_only_status_inconclusive(tmp_path: Path) -> None:
    document = _options({"id": "a", "text": "first", "status": "confirmed"})
    assert _refusal(tmp_path, document) == [
        "assertion.options.0.status: Input should be 'inconclusive'"
    ]


def test_option_ids_are_unique(tmp_path: Path) -> None:
    document = _options({"id": "a", "text": "one"}, {"id": "a", "text": "two"})
    assert _refusal(tmp_path, document) == ['manifest declares option id "a" more than once']


def test_an_options_assertion_requires_a_manual_driver(tmp_path: Path) -> None:
    document = {**OPTIONS, "driver": "unattended"}
    assert _refusal(tmp_path, document) == ["an options assertion requires a manual driver"]


def test_an_unknown_top_level_key_is_refused(tmp_path: Path) -> None:
    document = {**PROJECTION, "finding": "x"}
    assert _refusal(tmp_path, document) == ["finding: Extra inputs are not permitted"]


def test_an_absent_expected_is_refused(tmp_path: Path) -> None:
    document = {**PROJECTION, "assertion": {"projection": "."}}
    assert _refusal(tmp_path, document) == ["assertion.expected: Field required"]


def test_an_assertion_declaring_both_shapes_is_refused(tmp_path: Path) -> None:
    document = {**PROJECTION, "assertion": {"projection": ".", "options": [], "expected": 1}}
    assert _refusal(tmp_path, document) == [
        "assertion: must be an object declaring exactly one of projection or options"
    ]


@pytest.mark.parametrize("assertion", ["x", 5, None], ids=repr)
def test_an_assertion_that_is_not_an_object_is_refused(tmp_path: Path, assertion: Any) -> None:
    document = {**PROJECTION, "assertion": assertion}
    assert _refusal(tmp_path, document) == [
        "assertion: must be an object declaring exactly one of projection or options"
    ]


def test_an_unknown_key_inside_an_assertion_is_refused_by_its_path(tmp_path: Path) -> None:
    document = {**PROJECTION, "assertion": {"projection": ".", "expected": 1, "status": "x"}}
    assert _refusal(tmp_path, document) == ["assertion.status: Extra inputs are not permitted"]


def test_an_absent_depth_is_refused(tmp_path: Path) -> None:
    document = {k: v for k, v in PROJECTION.items() if k != "depth"}
    assert _refusal(tmp_path, document) == ["depth: Field required"]


@pytest.mark.parametrize("source", ["command", 5, ["command"], None], ids=repr)
def test_version_source_must_be_an_object(tmp_path: Path, source: Any) -> None:
    document = {**PROJECTION, "version_source": source}
    assert _refusal(tmp_path, document) == [
        "version_source: must be an object whose kind is command, capture, or none"
    ]


@pytest.mark.parametrize("kind", ["script", "", False, 1], ids=repr)
def test_version_source_kind_must_be_command_capture_or_none(tmp_path: Path, kind: Any) -> None:
    document = {**PROJECTION, "version_source": {"kind": kind}}
    assert _refusal(tmp_path, document) == [
        "version_source: must be an object whose kind is command, capture, or none"
    ]


def test_a_null_kind_is_refused_rather_than_read_as_none(tmp_path: Path) -> None:
    document = {**PROJECTION, "version_source": {"kind": None}}
    assert _refusal(tmp_path, document) == [
        "version_source: must be an object whose kind is command, capture, or none"
    ]


def test_an_unknown_key_inside_a_version_source_is_refused_by_its_path(tmp_path: Path) -> None:
    document = {**PROJECTION, "version_source": {"kind": "none", "command": "x"}}
    assert _refusal(tmp_path, document) == [
        "version_source.command: Extra inputs are not permitted"
    ]


@pytest.mark.parametrize(
    "command", ["opencode\n--version", "", "   ", None, ["opencode"]], ids=repr
)
def test_a_command_source_needs_a_non_empty_single_line_command(
    tmp_path: Path, command: Any
) -> None:
    document = {**PROJECTION, "version_source": {"kind": "command", "command": command}}
    assert _names(_refusal(tmp_path, document), "version_source.command")


def test_a_command_source_splits_on_whitespace(tmp_path: Path) -> None:
    document = {**PROJECTION, "version_source": {"kind": "command", "command": "a \t b  c"}}
    source = parse_manifest(_write(tmp_path, document)).version_source
    assert isinstance(source, CommandSource)
    assert source.argv == ("a", "b", "c")


@pytest.mark.parametrize("version_source", [{}, None], ids=["no kind", "absent"])
def test_a_missing_kind_resolves_to_no_source(tmp_path: Path, version_source: Any) -> None:
    document = {**PROJECTION, "version_source": version_source}
    if version_source is None:
        del document["version_source"]
    assert parse_manifest(_write(tmp_path, document)).version_source == NoSource()


@pytest.mark.parametrize("expr", ["", None, 5], ids=repr)
def test_a_capture_source_needs_a_non_empty_jq_string(tmp_path: Path, expr: Any) -> None:
    document = {**OPTIONS, "version_source": {"kind": "capture", "jq": expr}}
    assert _names(_refusal(tmp_path, document), "version_source.jq")


@pytest.mark.parametrize("wait_for", ["", False, 5], ids=repr)
def test_wait_for_must_be_a_non_empty_string(tmp_path: Path, wait_for: Any) -> None:
    document = {**OPTIONS, "wait_for": wait_for}
    assert _names(_refusal(tmp_path, document), "wait_for")


def test_an_options_manifest_parses_into_typed_options(tmp_path: Path) -> None:
    manifest = parse_manifest(_write(tmp_path, OPTIONS))
    assert manifest.assertion == OptionsAssertion(
        options=(
            Option(id="a", text="first"),
            Option(id="couldnt-tell", text="Could not determine", status="inconclusive"),
        ),
        expected="a",
    )
    assert manifest.wait_for == "length > 0"
    assert manifest.path == tmp_path / "probe.json"


@pytest.mark.parametrize(
    "path",
    sorted(EXPERIMENTS_DIR.glob("*/probe.json")),
    ids=lambda p: p.parent.name,
)
def test_every_committed_manifest_parses(path: Path) -> None:
    parse_manifest(path)
