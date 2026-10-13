"""The manifest gates in `contract.parse_manifest`, each refused with its exact message."""

import json
from pathlib import Path
from typing import Any

import pytest
from probe_harness.contract import (
    CommandSource,
    ManifestError,
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


def _refusal(tmp_path: Path, document: Any) -> str:
    path = _write(tmp_path, document)
    with pytest.raises(ManifestError) as exc:
        parse_manifest(path)
    message = str(exc.value)
    assert message.endswith(f": {path}")
    return message.removesuffix(f": {path}")


def _options(*options: Any) -> dict[str, Any]:
    return {**OPTIONS, "assertion": {"options": list(options), "expected": "a"}}


@pytest.mark.parametrize("root", [[], 5, None, False, "text"], ids=repr)
def test_a_root_that_is_not_an_object_is_refused(tmp_path: Path, root: Any) -> None:
    assert _refusal(tmp_path, root) == "manifest must be a JSON object"


@pytest.mark.parametrize("version", [1.0, True, "1", 2], ids=repr)
def test_schema_version_must_be_the_integer_1(tmp_path: Path, version: Any) -> None:
    document = {**PROJECTION, "schema_version": version}
    assert _refusal(tmp_path, document) == "manifest schema_version must be 1"


@pytest.mark.parametrize("options", [5, [], {"id": "a"}, None], ids=repr)
def test_options_must_be_a_non_empty_list(tmp_path: Path, options: Any) -> None:
    document = {**OPTIONS, "assertion": {"options": options, "expected": "a"}}
    assert _refusal(tmp_path, document) == "manifest assertion options must be a non-empty list"


def test_a_misspelled_driver_is_reported_before_a_malformed_option_set(tmp_path: Path) -> None:
    document = {**OPTIONS, "driver": "script", "assertion": {"options": 5, "expected": "a"}}
    assert _refusal(tmp_path, document) == "manifest driver must be unattended, billed, or manual"


@pytest.mark.parametrize("projection", [5, "", None, ["."]], ids=repr)
def test_a_projection_must_be_a_non_empty_string(tmp_path: Path, projection: Any) -> None:
    document = {**PROJECTION, "assertion": {"projection": projection, "expected": 1}}
    assert (
        _refusal(tmp_path, document) == "manifest assertion projection must be a non-empty string"
    )


@pytest.mark.parametrize(
    "option",
    [
        "a",
        {"id": "b", "text": "t", "label": "extra"},
        {"id": 2, "text": "t"},
        {"id": "", "text": "t"},
        {"text": "t"},
        {"id": "b", "text": None},
        {"id": "b"},
    ],
    ids=repr,
)
def test_an_option_must_be_an_object_with_a_string_id_and_text(tmp_path: Path, option: Any) -> None:
    document = _options({"id": "a", "text": "first"}, option)
    assert _refusal(tmp_path, document) == (
        "manifest option 2 must be an object with a string id and text, and no other key but status"
    )


def test_option_ids_are_unique(tmp_path: Path) -> None:
    document = _options({"id": "a", "text": "one"}, {"id": "a", "text": "two"})
    assert _refusal(tmp_path, document) == 'manifest declares option id "a" more than once'


@pytest.mark.parametrize("source", ["command", 5, ["command"]], ids=repr)
def test_version_source_must_be_an_object(tmp_path: Path, source: Any) -> None:
    document = {**PROJECTION, "version_source": source}
    assert _refusal(tmp_path, document) == "manifest version_source must be an object"


@pytest.mark.parametrize("kind", ["script", "", False, 1], ids=repr)
def test_version_source_kind_must_be_command_capture_or_none(tmp_path: Path, kind: Any) -> None:
    document = {**PROJECTION, "version_source": {"kind": kind}}
    assert _refusal(tmp_path, document) == (
        "manifest version_source.kind must be command, capture, or none"
    )


@pytest.mark.parametrize(
    "command", ["opencode\n--version", "", "   ", None, ["opencode"]], ids=repr
)
def test_a_command_source_needs_a_non_empty_single_line_command(
    tmp_path: Path, command: Any
) -> None:
    document = {**PROJECTION, "version_source": {"kind": "command", "command": command}}
    assert _refusal(tmp_path, document) == (
        "manifest version_source.command must be a non-empty, single-line string"
    )


def test_a_command_source_splits_on_whitespace(tmp_path: Path) -> None:
    document = {**PROJECTION, "version_source": {"kind": "command", "command": "a \t b  c"}}
    assert parse_manifest(_write(tmp_path, document)).version_source == CommandSource(
        ("a", "b", "c")
    )


@pytest.mark.parametrize("expr", ["", None, 5], ids=repr)
def test_a_capture_source_needs_a_non_empty_jq_string(tmp_path: Path, expr: Any) -> None:
    document = {**OPTIONS, "version_source": {"kind": "capture", "jq": expr}}
    assert _refusal(tmp_path, document) == "manifest version_source.jq must be a non-empty string"


@pytest.mark.parametrize("wait_for", ["", False, 5], ids=repr)
def test_wait_for_must_be_a_non_empty_string(tmp_path: Path, wait_for: Any) -> None:
    document = {**OPTIONS, "wait_for": wait_for}
    assert _refusal(tmp_path, document) == "manifest wait_for must be a non-empty string"


def test_an_options_manifest_parses_into_typed_options(tmp_path: Path) -> None:
    manifest = parse_manifest(_write(tmp_path, OPTIONS))
    assert manifest.assertion == OptionsAssertion(
        (Option("a", "first", None), Option("couldnt-tell", "Could not determine", "inconclusive")),
        "a",
    )
    assert manifest.wait_for == "length > 0"


@pytest.mark.parametrize(
    "path",
    sorted(EXPERIMENTS_DIR.glob("*/probe.json")),
    ids=lambda p: p.parent.name,
)
def test_every_committed_manifest_parses(path: Path) -> None:
    parse_manifest(path)
