"""`contract.parse_manifest`: each invalid manifest is refused, naming the invalid field's path."""

import json
from pathlib import Path
from typing import Any

import pytest
from probe_harness import json_values
from probe_harness.contract import (
    CaptureSource,
    CommandSource,
    ExpectedAssertion,
    Manifest,
    ManifestError,
    NoSource,
    Option,
    OptionsAssertion,
    RecordAssertionV2,
    RecordV1,
    RecordV2,
    Sample,
    parse_manifest,
    parse_record,
)
from pydantic import ValidationError

from tests.conftest import EXPERIMENTS_DIR

MANIFEST: dict[str, Any] = {
    "schema_version": 2,
    "provider": "opencode",
    "driver": "unattended",
    "depth": None,
    "question": "q",
    "version_source": {"kind": "none"},
    "assertion": {"expected": 1},
}

OPTIONS: dict[str, Any] = {
    **MANIFEST,
    "provider": "cursor",
    "driver": "manual",
    "assertion": {
        "options": [
            {"id": "a", "text": "first"},
            {"id": "couldnt-tell", "text": "Could not determine", "status": "inconclusive"},
        ],
        "expected": "a",
    },
}

BLOB = "0123456789abcdef0123456789abcdef01234567"


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
    document = {**MANIFEST, "assertion": {"expected": float("nan")}}
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


@pytest.mark.parametrize("version", [1, 1.0, True, 2.0, "2", 3, None], ids=repr)
def test_a_manifest_schema_version_must_be_the_integer_2(tmp_path: Path, version: Any) -> None:
    document = {**MANIFEST, "schema_version": version}
    assert _refusal(tmp_path, document) == ["schema_version must be 2"]


def test_schema_version_2_parses_as_a_manifest(tmp_path: Path) -> None:
    manifest = parse_manifest(_write(tmp_path, MANIFEST))
    assert isinstance(manifest, Manifest)
    assert manifest.assertion == ExpectedAssertion(expected=1)
    assert manifest.path == tmp_path / "probe.json"


def test_a_manifest_refuses_a_projection_by_its_path(tmp_path: Path) -> None:
    document = {**MANIFEST, "assertion": {"projection": ".", "expected": 1}}
    assert _refusal(tmp_path, document) == ["assertion.projection: Extra inputs are not permitted"]


def test_a_manifest_refuses_wait_for(tmp_path: Path) -> None:
    document = {**OPTIONS, "wait_for": "length > 0"}
    assert _refusal(tmp_path, document) == ["wait_for: Extra inputs are not permitted"]


def test_a_manifest_refuses_a_capture_source_s_jq(tmp_path: Path) -> None:
    document = {**OPTIONS, "version_source": {"kind": "capture", "jq": ".[0]"}}
    assert _refusal(tmp_path, document) == ["version_source.jq: Extra inputs are not permitted"]


def test_a_manifest_accepts_a_capture_source_with_no_expression(tmp_path: Path) -> None:
    document = {**OPTIONS, "version_source": {"kind": "capture"}}
    manifest = parse_manifest(_write(tmp_path, document))
    assert manifest.version_source == CaptureSource(kind="capture")
    assert isinstance(manifest.assertion, OptionsAssertion)


def test_an_options_assertion_requires_a_manual_driver(tmp_path: Path) -> None:
    document = {**OPTIONS, "driver": "unattended"}
    assert _refusal(tmp_path, document) == ["an options assertion requires a manual driver"]


@pytest.mark.parametrize("assertion", ["x", None], ids=repr)
def test_an_assertion_that_is_not_an_object_is_refused(tmp_path: Path, assertion: Any) -> None:
    assert _refusal(tmp_path, {**MANIFEST, "assertion": assertion}) == [
        "assertion: must be an object declaring expected, and options for an option set"
    ]


def _record_v2(**overrides: Any) -> dict[str, Any]:
    document: dict[str, Any] = {
        "schema_version": 2,
        "provider": "opencode",
        "status": "confirmed",
        "depth": None,
        "date": "2026-10-30",
        "tool_version": "1.0",
        "source": {"experiments/pkg/probe.py": BLOB},
        "assertion": {"expected": 1, "observed": 1},
    }
    document.update(overrides)
    return document


@pytest.mark.parametrize("blob", ["", "abc", BLOB.upper(), BLOB + "0", BLOB[:-1] + "g"], ids=repr)
def test_a_v2_record_refuses_a_malformed_blob_id(blob: str) -> None:
    with pytest.raises(ValidationError):
        RecordV2.model_validate(_record_v2(source={"experiments/pkg/probe.py": blob}))


def test_a_v2_record_accepts_a_sha256_blob_id() -> None:
    sha256 = BLOB + "0" * 24
    record = RecordV2.model_validate(_record_v2(source={"experiments/pkg/probe.py": sha256}))
    assert record.source == {"experiments/pkg/probe.py": sha256}


def test_a_v2_record_dumps_in_the_documented_key_order_without_arranged() -> None:
    record = RecordV2.model_validate(_record_v2())
    assert list(record.model_dump(mode="json")) == [
        "schema_version",
        "provider",
        "status",
        "depth",
        "date",
        "tool_version",
        "source",
        "assertion",
    ]


def test_a_v2_record_dumps_in_the_documented_key_order_with_arranged() -> None:
    record = RecordV2.model_validate(_record_v2(arranged={"experiments/pkg/probe.py": BLOB}))
    dumped = record.model_dump(mode="json")
    assert list(dumped) == [
        "schema_version",
        "provider",
        "status",
        "depth",
        "date",
        "tool_version",
        "source",
        "arranged",
        "assertion",
    ]
    assert list(dumped["assertion"]) == ["expected", "observed"]


def test_a_v2_record_dumps_an_option_set_s_options_first() -> None:
    options = [{"id": "a", "text": "first"}]
    record = RecordV2.model_validate(
        _record_v2(assertion={"options": options, "expected": "a", "observed": "a"})
    )
    assert isinstance(record.assertion, RecordAssertionV2)
    assert list(record.model_dump(mode="json")["assertion"]) == ["options", "expected", "observed"]


def test_parse_record_reads_either_version(tmp_path: Path) -> None:
    v2 = tmp_path / "v2.json"
    v2.write_text(json_values.dumps(_record_v2()), encoding="utf-8")
    v1 = tmp_path / "v1.json"
    v1_document = {
        **{k: v for k, v in _record_v2().items() if k != "source"},
        "schema_version": 1,
        "assertion": {"projection": ".", "expected": 1, "observed": 1},
    }
    v1.write_text(json_values.dumps(v1_document), encoding="utf-8")
    assert isinstance(parse_record(v2), RecordV2)
    assert isinstance(parse_record(v1), RecordV1)


def test_parse_record_refuses_a_v2_record_without_a_source(tmp_path: Path) -> None:
    path = tmp_path / "r.json"
    document = {k: v for k, v in _record_v2().items() if k != "source"}
    path.write_text(json_values.dumps(document), encoding="utf-8")
    assert parse_record(path) is None


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


def test_an_unknown_top_level_key_is_refused(tmp_path: Path) -> None:
    document = {**MANIFEST, "finding": "x"}
    assert _refusal(tmp_path, document) == ["finding: Extra inputs are not permitted"]


def test_an_absent_expected_is_refused(tmp_path: Path) -> None:
    document = {**MANIFEST, "assertion": {}}
    assert _refusal(tmp_path, document) == ["assertion.expected: Field required"]


def test_an_unknown_key_inside_an_assertion_is_refused_by_its_path(tmp_path: Path) -> None:
    document = {**MANIFEST, "assertion": {"expected": 1, "status": "x"}}
    assert _refusal(tmp_path, document) == ["assertion.status: Extra inputs are not permitted"]


def test_an_absent_depth_is_refused(tmp_path: Path) -> None:
    document = {k: v for k, v in MANIFEST.items() if k != "depth"}
    assert _refusal(tmp_path, document) == ["depth: Field required"]


@pytest.mark.parametrize("source", ["command", 5, ["command"], None], ids=repr)
def test_version_source_must_be_an_object(tmp_path: Path, source: Any) -> None:
    document = {**MANIFEST, "version_source": source}
    assert _refusal(tmp_path, document) == [
        "version_source: must be an object whose kind is command, capture, or none"
    ]


@pytest.mark.parametrize("kind", ["script", "", False, 1], ids=repr)
def test_version_source_kind_must_be_command_capture_or_none(tmp_path: Path, kind: Any) -> None:
    document = {**MANIFEST, "version_source": {"kind": kind}}
    assert _refusal(tmp_path, document) == [
        "version_source: must be an object whose kind is command, capture, or none"
    ]


def test_a_null_kind_is_refused_rather_than_read_as_none(tmp_path: Path) -> None:
    document = {**MANIFEST, "version_source": {"kind": None}}
    assert _refusal(tmp_path, document) == [
        "version_source: must be an object whose kind is command, capture, or none"
    ]


def test_an_unknown_key_inside_a_version_source_is_refused_by_its_path(tmp_path: Path) -> None:
    document = {**MANIFEST, "version_source": {"kind": "none", "command": "x"}}
    assert _refusal(tmp_path, document) == [
        "version_source.command: Extra inputs are not permitted"
    ]


@pytest.mark.parametrize(
    "command", ["opencode\n--version", "", "   ", None, ["opencode"]], ids=repr
)
def test_a_command_source_needs_a_non_empty_single_line_command(
    tmp_path: Path, command: Any
) -> None:
    document = {**MANIFEST, "version_source": {"kind": "command", "command": command}}
    assert _names(_refusal(tmp_path, document), "version_source.command")


def test_a_command_source_splits_on_whitespace(tmp_path: Path) -> None:
    document = {**MANIFEST, "version_source": {"kind": "command", "command": "a \t b  c"}}
    source = parse_manifest(_write(tmp_path, document)).version_source
    assert isinstance(source, CommandSource)
    assert source.argv == ("a", "b", "c")


@pytest.mark.parametrize("version_source", [{}, None], ids=["no kind", "absent"])
def test_a_missing_kind_resolves_to_no_source(tmp_path: Path, version_source: Any) -> None:
    document = {**MANIFEST, "version_source": version_source}
    if version_source is None:
        del document["version_source"]
    assert parse_manifest(_write(tmp_path, document)).version_source == NoSource()


def test_an_options_manifest_parses_into_typed_options(tmp_path: Path) -> None:
    manifest = parse_manifest(_write(tmp_path, OPTIONS))
    assert manifest.assertion == OptionsAssertion(
        options=(
            Option(id="a", text="first"),
            Option(id="couldnt-tell", text="Could not determine", status="inconclusive"),
        ),
        expected="a",
    )
    assert isinstance(manifest, Manifest)
    assert not hasattr(manifest, "wait_for")
    assert manifest.path == tmp_path / "probe.json"


@pytest.mark.parametrize(
    "path",
    sorted(EXPERIMENTS_DIR.glob("*/probe.json")),
    ids=lambda p: p.parent.name,
)
def test_every_committed_manifest_parses(path: Path) -> None:
    parse_manifest(path)


EDITED_SAMPLE: dict[str, Any] = {
    "origin": "edited",
    "note": "the reply replaced by an error",
    "authored": ["MCP error: the fx server is unavailable"],
    "input": {"reply": "MCP error: the fx server is unavailable"},
    "results": {"assertion": "error"},
}


def test_an_edited_sample_carries_an_authored_list() -> None:
    assert Sample.model_validate(EDITED_SAMPLE).authored == (
        "MCP error: the fx server is unavailable",
    )


def test_an_authored_list_is_refused_on_a_capture_sample() -> None:
    document = {k: v for k, v in EDITED_SAMPLE.items() if k != "note"}
    with pytest.raises(ValidationError, match="an authored list belongs only on an edited"):
        Sample.model_validate({**document, "origin": "capture"})


def test_an_authored_entry_absent_from_the_input_is_refused() -> None:
    with pytest.raises(ValidationError, match="'not typed anywhere'"):
        Sample.model_validate({**EDITED_SAMPLE, "authored": ["not typed anywhere"]})


def test_an_authored_entry_inside_a_longer_line_is_refused() -> None:
    with pytest.raises(ValidationError, match="'the fx server is unavailable'"):
        Sample.model_validate({**EDITED_SAMPLE, "authored": ["the fx server is unavailable"]})


def test_an_authored_entry_may_be_one_line_of_a_multi_line_string() -> None:
    document = {
        **EDITED_SAMPLE,
        "input": {"reply": "first\nMCP error: the fx server is unavailable"},
    }
    assert Sample.model_validate(document).authored == ("MCP error: the fx server is unavailable",)
