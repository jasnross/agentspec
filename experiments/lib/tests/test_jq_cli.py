"""The jq boundary's own behavior: comparison, record assembly, and compile checks."""

from pathlib import Path

from probe_harness import jq_cli
from probe_harness.workspace import load_json_values

# A manifest whose `expected` carries a trailing-zero literal and a nested object, so a
# record that re-serialized numbers through Python, or reordered keys, would differ.
GOLDEN_MANIFEST = """\
{
  "schema_version": 1,
  "provider": "opencode",
  "driver": "unattended",
  "depth": "resolved-config",
  "question": "Does the golden record match?",
  "version_source": {"kind": "none"},
  "assertion": {
    "projection": ".x",
    "expected": {"n": 1.10, "nested": {"b": [1, 2.50], "a": "s"}}
  }
}
"""

# The compact observed value `record.sh` projected from the golden view,
# `{"x": {"nested": {"a": "s", "b": [1, 2.50]}, "n": 1.10}}`, with `.x`.
GOLDEN_OBSERVED = '{"nested":{"a":"s","b":[1,2.50]},"n":1.10}'

GOLDEN_DATE = "2026-10-12"

# `record.sh --manifest <GOLDEN_MANIFEST> --view <view> --dry-run`, run from the reference
# bash checkout under jq 1.8.2, with its trailing newline removed. Its `date` is the day
# it ran, which is why `GOLDEN_DATE` is passed in rather than read from a clock.
GOLDEN_RECORD = """\
{
  "schema_version": 1,
  "provider": "opencode",
  "status": "confirmed",
  "depth": "resolved-config",
  "date": "2026-10-12",
  "tool_version": null,
  "assertion": {
    "projection": ".x",
    "expected": {
      "n": 1.10,
      "nested": {
        "b": [
          1,
          2.50
        ],
        "a": "s"
      }
    },
    "observed": {
      "nested": {
        "a": "s",
        "b": [
          1,
          2.50
        ]
      },
      "n": 1.10
    }
  }
}"""


def test_equal_ignores_key_order() -> None:
    assert jq_cli.equal('{"a":1,"b":2}', '{"b":2,"a":1}')


def test_equal_compares_numbers_as_jq_does() -> None:
    # `jq -n '1.10 == 1.1'` prints `true` under jq 1.8.2: equality is numeric, even though
    # jq preserves the `1.10` literal when it prints the value unchanged.
    assert jq_cli.equal("1.10", "1.1")


def test_assemble_record_matches_record_sh(tmp_path: Path) -> None:
    manifest = tmp_path / "probe.json"
    manifest.write_text(GOLDEN_MANIFEST, encoding="utf-8")

    record = jq_cli.assemble_record(
        manifest, "projection", GOLDEN_OBSERVED, "confirmed", GOLDEN_DATE, ""
    )

    assert record == GOLDEN_RECORD


def test_compile_error_returns_jq_diagnostic_for_a_broken_expression() -> None:
    error = jq_cli.compile_error(".[")
    assert isinstance(error, str)
    assert error


def test_compile_error_returns_none_for_a_valid_expression() -> None:
    assert jq_cli.compile_error(".") is None


def test_load_json_values_reads_concatenated_objects_without_whitespace(tmp_path: Path) -> None:
    path = tmp_path / "payloads.jsonl"
    path.write_text('{"a":1}{"b":2}', encoding="utf-8")

    assert load_json_values(path) == [{"a": 1}, {"b": 2}]


def test_load_json_values_reads_concatenated_objects_with_whitespace(tmp_path: Path) -> None:
    path = tmp_path / "payloads.jsonl"
    path.write_text('{"a":1}\n  {"b":2}\n', encoding="utf-8")

    assert load_json_values(path) == [{"a": 1}, {"b": 2}]
