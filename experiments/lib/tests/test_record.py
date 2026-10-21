"""Coverage for `probe record <package> <workspace>`: comparison, capture refusal, selection
handling, and record well-formedness. No provider CLI is involved: every workspace here is
hand-written, in its family's layout.
"""

import datetime
import json
import os
import shutil
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest
from probe_harness import contract, json_values, record
from probe_harness.record import JudgeResult

from tests.conftest import (
    Cli,
    Result,
    unattended_manifest,
)

type RecordWellformed = Callable[[Path], None]


SCRIPT_MANIFEST: dict[str, Any] = {
    "schema_version": 1,
    "provider": "opencode",
    "driver": "unattended",
    "depth": "resolved-config",
    "question": "Test manifest.",
    "version_source": {"kind": "none"},
    "assertion": {
        "projection": "{model, variant}",
        "expected": {"model": "m", "variant": "high"},
    },
}


JUDGE_MANIFEST: dict[str, Any] = {
    "schema_version": 1,
    "provider": "cursor",
    "driver": "manual",
    "depth": None,
    "question": "Which markers appeared?",
    "version_source": {"kind": "none"},
    "assertion": {
        "options": [
            {"id": "both-markers", "text": "Both markers were visible"},
            {"id": "neither", "text": "Neither marker was visible"},
            {"id": "couldnt-tell", "text": "Could not determine", "status": "inconclusive"},
        ],
        "expected": "neither",
    },
}


# A manifest whose option set declares `inconclusive` more than once. The narrowing rejects
# a *value*, not the presence of a status, so this must be accepted: a gate written as "at
# most one option declares a status" would refuse it, and both tests using it would fail at
# the manifest check.
MULTI_INCONCLUSIVE_MANIFEST: dict[str, Any] = {
    "schema_version": 1,
    "provider": "cursor",
    "driver": "manual",
    "depth": None,
    "question": "q",
    "version_source": {"kind": "none"},
    "assertion": {
        "options": [
            {"id": "neither", "text": "Neither marker was visible"},
            {"id": "couldnt-tell", "text": "Could not determine", "status": "inconclusive"},
            {"id": "not-reached", "text": "The command never ran", "status": "inconclusive"},
        ],
        "expected": "neither",
    },
}


# A custom probe whose judge reads the view a test wrote into the workspace.
VIEW_PROBE_PY = """\
from pathlib import Path
from probe_harness.families.base import CustomProbe, JudgeResult

PROBE = CustomProbe(required_tools=(), arrange=lambda ws, variant: None, act=lambda ws: None,
                    judge=lambda ws: JudgeResult(view=ws / "view.json", selection=None))
"""

MANUAL_PROBE_PY = """\
from probe_harness.families.manual import ManualProbe

PROBE = ManualProbe(instructions="")
"""

# The filter every capture here satisfies.
WAIT_FOR = "length > 0"


@pytest.fixture
def pkg(tree: Path) -> Path:
    """Package `pkg`, in the tree `probe record` reads through `PROBE_EXPERIMENTS_DIR`."""
    package = tree / "pkg"
    package.mkdir()
    return package


@pytest.fixture
def manifest(pkg: Path) -> Path:
    return pkg / "probe.json"


def stamp_run(ws: Path, package: str = "pkg", variant: str = "assertion") -> None:
    """Record in <ws> the package and variant a run arranges it for, as a run does."""
    (ws / "run.json").write_text(json.dumps({"package": package, "variant": variant}) + "\n")


@pytest.fixture
def ws(tmp_path: Path) -> Path:
    """A workspace for a projection probe outside the manual family."""
    workspace = tmp_path / "ws"
    workspace.mkdir()
    stamp_run(workspace)
    return workspace


@pytest.fixture
def view(ws: Path) -> Path:
    return ws / "view.json"


def write_manifest(path: Path, manifest: dict[str, Any]) -> None:
    """Write <manifest> beside a definition of its family.

    A `manual` manifest gets a manual probe, and, unless it declares one, the `wait_for`
    every capture here satisfies; any other gets a custom probe whose judge reads the
    workspace's `view.json`.
    """
    manual = manifest.get("driver") == "manual"
    if manual:
        manifest = {"wait_for": WAIT_FOR, **manifest}
    path.write_text(json.dumps(manifest, indent=2) + "\n")
    (path.parent / "probe.py").write_text(MANUAL_PROBE_PY if manual else VIEW_PROBE_PY)


def write_view_text(path: Path, text: str) -> None:
    path.write_text(text + "\n")


def fresh_capture(tmp_path: Path) -> Path:
    """A workspace shaped the way the harness shapes one, carrying a Cursor hook payload."""
    ws = tmp_path / "ws"
    (ws / "capture").mkdir(parents=True, exist_ok=True)
    (ws / "capture" / "payloads.jsonl").write_text(
        '{"cursor_version":"3.16.17","model":"m","variant":"high"}\n'
    )
    stamp_run(ws)
    return ws


def record_files(pkg: Path) -> list[Path]:
    return sorted((pkg / "results").glob("*.json"))


def the_record(pkg: Path) -> Any:
    """The newest record. Filenames sort lexicographically in run order, which is what the
    filename's time component is for; the first would be the oldest.
    """
    return json.loads(record_files(pkg)[-1].read_text())


def first_record_file(pkg: Path) -> Path:
    return record_files(pkg)[0]


def dry_run(cli: Cli, *args: str | os.PathLike[str]) -> Result:
    """`probe record <args…> --dry-run`: stdout is the would-be record, stderr the diagnostic."""
    return cli("record", *args, "--dry-run")


def test_a_matching_observed_value_records_confirmed(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = cli("record", "pkg", view.parent)
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "confirmed"


def test_a_mismatching_observed_value_records_refuted(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"low"}')

    result = cli("record", "pkg", view.parent)
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "refuted"
    assert the_record(pkg)["assertion"]["observed"]["variant"] == "low"


def test_comparison_is_structural_so_key_order_does_not_matter(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    # This is the property JSON was chosen for and the one a string comparison would have
    # broken.
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"variant":"high","model":"m"}')

    result = cli("record", "pkg", view.parent)
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "confirmed"


def test_a_manual_projection_workspace_without_a_capture_is_refused(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(
        manifest,
        {
            "schema_version": 1,
            "provider": "cursor",
            "driver": "manual",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {"projection": ".", "expected": {}},
        },
    )
    write_view_text(view, "{}")

    result = cli("record", "pkg", view.parent)
    assert result.returncode == 1
    assert f"probe record: {view.parent} is not a probe workspace (no capture/ directory)\n" in (
        result.stderr
    )
    assert not (pkg / "results").is_dir()


def test_a_manual_options_workspace_without_a_capture_is_refused(
    cli: Cli, pkg: Path, manifest: Path, ws: Path
) -> None:
    write_manifest(manifest, JUDGE_MANIFEST)

    result = cli("record", "pkg", ws, "--selection", "neither")
    assert result.returncode == 1
    assert "is not a probe workspace (no capture/ directory)" in result.stderr
    assert not (pkg / "results").is_dir()


def test_a_selection_matching_expected_records_confirmed(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    write_manifest(manifest, JUDGE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "pkg", ws, "--selection", "neither")
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "confirmed"
    assert the_record(pkg)["assertion"]["observed"] == "neither"


def test_a_selection_not_matching_expected_records_refuted(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    write_manifest(manifest, JUDGE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "pkg", ws, "--selection", "both-markers")
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "refuted"
    assert the_record(pkg)["assertion"]["observed"] == "both-markers"


def test_the_couldnt_tell_option_records_inconclusive(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    # Without this an operator who could not tell would produce a false pass.
    write_manifest(manifest, JUDGE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "pkg", ws, "--selection", "couldnt-tell")
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "inconclusive"


def test_an_option_declaring_a_status_other_than_inconclusive_is_refused(
    cli: Cli, tmp_path: Path, manifest: Path
) -> None:
    # A declared status replaces the comparison, so any value but `inconclusive` is the
    # caller-supplied verdict the no-`--status` rule exists to prevent, smuggled in through
    # the manifest instead of the command line.
    write_manifest(
        manifest,
        {
            "schema_version": 1,
            "provider": "cursor",
            "driver": "manual",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {
                "options": [
                    {"id": "yes", "text": "yes", "status": "confirmed"},
                    {"id": "couldnt-tell", "text": "Could not determine", "status": "inconclusive"},
                ],
                "expected": "yes",
            },
        },
    )
    ws = fresh_capture(tmp_path)

    result = cli("record", "pkg", ws, "--selection", "yes")
    assert result.returncode != 0
    assert "  assertion.options.0.status: Input should be 'inconclusive'" in result.output


def test_several_inconclusive_options_do_not_stop_the_comparison(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    write_manifest(manifest, MULTI_INCONCLUSIVE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "pkg", ws, "--selection", "neither")
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "confirmed"


def test_an_inconclusive_option_yields_inconclusive_wherever_it_sits_in_the_set(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    # `not-reached` is the second option declaring the status, so a gate reading only the
    # first would miss it.
    write_manifest(manifest, MULTI_INCONCLUSIVE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "pkg", ws, "--selection", "not-reached")
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "inconclusive"


def test_a_manifest_declaring_a_retired_depth_is_refused(
    cli: Cli, manifest: Path, view: Path
) -> None:
    # `provider-parses` would be an assertion on the absence of an error, which the contract
    # forbids. A record copies `depth` from its manifest, so a value gathered nowhere would
    # become a claim on a record.
    write_manifest(manifest, {**SCRIPT_MANIFEST, "depth": "provider-parses"})
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = cli("record", "pkg", view.parent)
    assert result.returncode != 0
    assert "  depth: Input should be" in result.output


def test_a_selection_absent_from_the_option_set_is_refused(
    cli: Cli, tmp_path: Path, manifest: Path
) -> None:
    write_manifest(manifest, JUDGE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "pkg", ws, "--selection", "typo")
    assert result.returncode != 0
    assert "not one of the manifest's option ids" in result.output


@pytest.mark.parametrize(
    ("args", "message"),
    [
        ((), "the following arguments are required: package, workspace"),
        (("pkg",), "the following arguments are required: workspace"),
        (("pkg", "ws", "--selection"), "argument --selection: expected one argument"),
        (("pkg", "ws", "--bogus"), "unrecognized arguments: --bogus"),
    ],
)
def test_an_argument_error_exits_2(cli: Cli, args: tuple[str, ...], message: str) -> None:
    result = cli("record", *args)
    assert result.returncode == 2
    assert f"probe record: error: {message}" in result.stderr


def test_a_record_with_depth_null_is_well_formed(
    cli: Cli,
    tmp_path: Path,
    pkg: Path,
    manifest: Path,
    record_wellformed: RecordWellformed,
) -> None:
    # An off-chain finding must be representable.
    write_manifest(manifest, JUDGE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "pkg", ws, "--selection", "neither")
    assert result.returncode == 0

    record_file = first_record_file(pkg)
    assert json.loads(record_file.read_text())["depth"] is None
    record_wellformed(record_file)


MANUAL_PROJECTION_MANIFEST: dict[str, Any] = {
    "schema_version": 1,
    "provider": "cursor",
    "driver": "manual",
    "depth": None,
    "question": "q",
    "version_source": {"kind": "none"},
    "assertion": {"projection": "[.[] | .variant] | first", "expected": "high"},
}


@pytest.mark.parametrize("payloads", [None, ""], ids=["absent", "empty"])
def test_a_capture_with_no_payloads_is_refused(
    cli: Cli, pkg: Path, manifest: Path, ws: Path, payloads: str | None
) -> None:
    write_manifest(manifest, MANUAL_PROJECTION_MANIFEST)
    (ws / "capture").mkdir()
    if payloads is not None:
        (ws / "capture" / "payloads.jsonl").write_text(payloads)

    result = cli("record", "pkg", ws)
    assert result.returncode == 1
    assert f"probe record: no captured payloads at {ws / 'capture' / 'payloads.jsonl'}\n" in (
        result.stderr
    )
    assert not (pkg / "results").is_dir()


def test_a_fresh_capture_records_successfully_and_stores_nothing_about_the_capture(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    write_manifest(
        manifest,
        {
            "schema_version": 1,
            "provider": "cursor",
            "driver": "manual",
            "depth": None,
            "question": "q",
            "version_source": {
                "kind": "capture",
                "jq": "[.[] | .cursor_version] | map(select(. != null)) | first",
            },
            "assertion": {"projection": "[.[] | .variant] | first", "expected": "high"},
        },
    )
    ws = fresh_capture(tmp_path)

    result = cli("record", "pkg", ws)
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "confirmed"
    # The version came out of the capture, slurped.
    assert the_record(pkg)["tool_version"] == "3.16.17"
    assert "capture" not in the_record(pkg)


def test_the_emitted_record_parses_and_carries_the_full_key_set(
    cli: Cli,
    pkg: Path,
    manifest: Path,
    view: Path,
    record_wellformed: RecordWellformed,
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = cli("record", "pkg", view.parent)
    assert result.returncode == 0

    record_wellformed(first_record_file(pkg))


def test_date_is_stamped_in_utc(cli: Cli, pkg: Path, manifest: Path, view: Path) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    # Run under a timezone 14 hours ahead of UTC, so for most of the day a local-clock stamp
    # would produce tomorrow's date and fail this assertion. The clock is read either side of
    # the run so a genuine UTC-midnight crossing tolerates the boundary instead of flaking.
    before = datetime.datetime.now(datetime.UTC).date().isoformat()
    result = cli("record", "pkg", view.parent, env={"TZ": "Pacific/Kiritimati"})
    after = datetime.datetime.now(datetime.UTC).date().isoformat()
    assert result.returncode == 0

    assert the_record(pkg)["date"] in (before, after)


def test_a_record_carries_no_key_outside_the_seven_key_contract(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = cli("record", "pkg", view.parent)
    assert result.returncode == 0

    record = json.loads(first_record_file(pkg).read_text())
    for key in ("probe", "kind", "driver", "capture", "finding", "blocked_reason"):
        assert key not in record


def test_a_path_hostile_version_string_is_sanitized_in_the_record_filename(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path, view: Path
) -> None:
    # The run recorded the hostile string as the command's output; the judge reads it from
    # the workspace and never runs the command.
    stub = tmp_path / "hostile-version"
    (view.parent / "tool_version").write_text("1.0/../etc x\n")

    write_manifest(
        manifest,
        {
            "schema_version": 1,
            "provider": "opencode",
            "driver": "unattended",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "command", "command": str(stub)},
            "assertion": {"projection": ".", "expected": {}},
        },
    )
    write_view_text(view, "{}")

    result = cli("record", "pkg", view.parent)
    assert result.returncode == 0

    name = first_record_file(pkg).name
    assert "/../" not in name
    assert " " not in name
    assert name.endswith("1.0_.._etc_x.json")


def test_a_manifest_declaring_no_expected_value_is_refused(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    # Otherwise the expected value reads as null, a misspelled projection also yields null,
    # and `null == null` records a vacuous confirmed: the exact failure the contract claims
    # to prevent.
    write_manifest(
        manifest,
        {
            "schema_version": 1,
            "provider": "opencode",
            "driver": "unattended",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {"projection": ".no.such.path"},
        },
    )
    write_view_text(view, '{"real":"data"}')

    result = cli("record", "pkg", view.parent)
    assert result.returncode != 0
    assert "  assertion.expected: Field required" in result.output
    assert not (pkg / "results").is_dir()


def test_a_manifest_declaring_both_projection_and_options_is_refused(
    cli: Cli, manifest: Path, view: Path
) -> None:
    write_manifest(
        manifest,
        {
            "schema_version": 1,
            "provider": "cursor",
            "driver": "manual",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {
                "projection": ".",
                "options": [{"id": "a", "text": "a"}],
                "expected": "a",
            },
        },
    )
    write_view_text(view, "{}")

    result = cli("record", "pkg", view.parent)
    assert result.returncode != 0
    assert "exactly one of projection or options" in result.output


def test_a_manifest_declaring_options_under_any_other_driver_is_refused(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    # An options assertion is a person choosing from a list, so running the probe requires a
    # person present. Both other members of the enum are covered rather than just one,
    # because `billed` is scriptable and so is the value a future edit is most likely to
    # loosen the gate for.
    for driver in ("unattended", "billed"):
        shutil.rmtree(pkg / "results", ignore_errors=True)
        write_manifest(
            manifest,
            {
                "schema_version": 1,
                "provider": "cursor",
                "driver": driver,
                "depth": None,
                "question": "q",
                "version_source": {"kind": "none"},
                "assertion": {
                    "options": [
                        {"id": "yes", "text": "yes"},
                        {
                            "id": "couldnt-tell",
                            "text": "Could not determine",
                            "status": "inconclusive",
                        },
                    ],
                    "expected": "yes",
                },
            },
        )
        ws = fresh_capture(tmp_path)

        result = cli("record", "pkg", ws, "--selection", "yes")
        assert result.returncode != 0, driver
        assert "requires a manual driver" in result.output, driver
        assert not (pkg / "results").is_dir(), driver


def test_a_manifest_declaring_neither_projection_nor_options_is_refused(
    cli: Cli, manifest: Path, view: Path
) -> None:
    write_manifest(
        manifest,
        {
            "schema_version": 1,
            "provider": "cursor",
            "driver": "unattended",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {"expected": "a"},
        },
    )
    write_view_text(view, "{}")

    result = cli("record", "pkg", view.parent)
    assert result.returncode != 0
    assert "exactly one of projection or options" in result.output


def test_an_options_manifest_recorded_without_a_selection_is_a_usage_error(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    # With no terminal to prompt on, the selection has to be passed, and nothing records.
    write_manifest(manifest, JUDGE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "pkg", ws)
    assert result.returncode == 2
    assert "probe record: pkg declares options; pass --selection <id>\n" in result.stderr
    assert not (pkg / "results").is_dir()


def test_a_projection_manifest_recorded_with_a_selection_is_a_usage_error(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = cli("record", "pkg", view.parent, "--selection", "anything")
    assert result.returncode == 2
    assert "probe record: pkg declares a projection; --selection does not apply\n" in (
        result.stderr
    )
    assert not (pkg / "results").is_dir()


def test_a_manifest_with_an_unknown_provider_is_refused(
    cli: Cli, manifest: Path, view: Path
) -> None:
    # Also keeps `provider` out of the record path, which is built from it.
    write_manifest(
        manifest,
        {
            "schema_version": 1,
            "provider": "../../etc",
            "driver": "unattended",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {"projection": ".", "expected": {}},
        },
    )
    write_view_text(view, "{}")

    result = cli("record", "pkg", view.parent)
    assert result.returncode != 0
    assert "  provider: Input should be" in result.output


def test_a_manifest_with_an_unknown_schema_version_is_refused(
    cli: Cli, manifest: Path, view: Path
) -> None:
    write_manifest(
        manifest,
        {
            "schema_version": 99,
            "provider": "opencode",
            "driver": "unattended",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {"projection": ".", "expected": {}},
        },
    )
    write_view_text(view, "{}")

    result = cli("record", "pkg", view.parent)
    assert result.returncode != 0
    assert "  schema_version: Input should be" in result.output


def test_a_manifest_with_an_unknown_driver_is_refused(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    # `script` rather than an invented string: a retired value is what a stale manifest
    # actually carries, and it is the one the enum has to reject.
    write_manifest(
        manifest,
        {
            "schema_version": 1,
            "provider": "opencode",
            "driver": "script",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {"projection": ".", "expected": {}},
        },
    )
    write_view_text(view, "{}")

    result = cli("record", "pkg", view.parent)
    assert result.returncode != 0
    assert "  driver: Input should be 'unattended', 'billed' or 'manual'" in result.output
    assert not (pkg / "results").is_dir()


def test_a_projection_yielding_several_values_is_refused_with_a_clear_diagnostic(
    cli: Cli, manifest: Path, view: Path
) -> None:
    write_manifest(
        manifest,
        {
            "schema_version": 1,
            "provider": "opencode",
            "driver": "unattended",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {"projection": ".[]", "expected": 1},
        },
    )
    write_view_text(view, "[1,2,3]")

    result = cli("record", "pkg", view.parent)
    assert result.returncode != 0
    assert "exactly one JSON value" in result.output


def test_a_duplicated_option_id_is_refused(cli: Cli, tmp_path: Path, manifest: Path) -> None:
    write_manifest(
        manifest,
        {
            "schema_version": 1,
            "provider": "cursor",
            "driver": "manual",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {
                "options": [
                    {"id": "dup", "text": "one"},
                    {"id": "dup", "text": "two", "status": "inconclusive"},
                ],
                "expected": "dup",
            },
        },
    )
    ws = fresh_capture(tmp_path)

    result = cli("record", "pkg", ws, "--selection", "dup")
    assert result.returncode == 1
    assert f"probe record: manifest is invalid: {manifest}\n" in result.stderr
    assert 'probe record:   manifest declares option id "dup" more than once' in result.stderr


def test_two_runs_on_the_same_day_at_the_same_version_produce_two_records(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    # The append-only property the filename's time component exists to guarantee: re-running
    # a refuted probe to confirm it must not overwrite the evidence.
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    cli("record", "pkg", view.parent)
    time.sleep(1)
    cli("record", "pkg", view.parent)

    assert len(record_files(pkg)) == 2


def test_dry_run_writes_no_record(cli: Cli, pkg: Path, manifest: Path, view: Path) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = dry_run(cli, "pkg", view.parent)
    assert result.returncode == 0
    assert "no file written" in result.stderr
    assert not (pkg / "results").is_dir()


def test_dry_run_prints_a_well_formed_record_to_stdout(
    cli: Cli,
    tmp_path: Path,
    manifest: Path,
    view: Path,
    record_wellformed: RecordWellformed,
) -> None:
    # The point of the flag: what it prints is what a record would contain, so a projection
    # validated here is the one the real run will evaluate.
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = dry_run(cli, "pkg", view.parent)
    assert result.returncode == 0

    dry = tmp_path / "dry.json"
    dry.write_text(result.stdout)
    record_wellformed(dry)


def test_dry_run_exits_0_when_the_comparison_computes_refuted(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    # Validating that an assertion discriminates means running it against an input it is
    # supposed to refute, so a nonzero exit here would fail the flag's primary use case every
    # time.
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"low"}')

    result = dry_run(cli, "pkg", view.parent)
    assert result.returncode == 0
    assert json.loads(result.stdout)["status"] == "refuted"
    assert not (pkg / "results").is_dir()


def test_dry_run_still_refuses_a_manifest_failing_a_gate(
    cli: Cli, manifest: Path, view: Path
) -> None:
    # Every gate is part of the wiring being verified, so dry-run enforces all of them; only
    # the write is skipped.
    write_manifest(
        manifest,
        {
            "schema_version": 1,
            "provider": "opencode",
            "driver": "unattended",
            "depth": "provider-parses",
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {"projection": ".", "expected": {}},
        },
    )
    write_view_text(view, "{}")

    result = dry_run(cli, "pkg", view.parent)
    assert result.returncode != 0
    assert "  depth: Input should be" in result.stderr
    # A refused dry run emits no would-be record, so a scripted `--dry-run >file` cannot leave
    # a partial artifact behind.
    assert result.stdout == ""


def test_dry_run_still_enforces_the_capture_non_empty_check(
    cli: Cli, manifest: Path, ws: Path
) -> None:
    write_manifest(manifest, MANUAL_PROJECTION_MANIFEST)
    (ws / "capture").mkdir()
    (ws / "capture" / "payloads.jsonl").write_text("")

    result = dry_run(cli, "pkg", ws)
    assert result.returncode == 1
    assert "no captured payloads at" in result.stderr
    assert result.stdout == ""


def test_dry_run_records_nothing_on_the_selection_path_either(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    # The options path is the expensive one to get wrong: a human-judged probe discovers a
    # wiring error only during a live session. The flag is passed first here on purpose: the
    # usage line shows it trailing, and the helper above always passes it last, so nothing
    # else exercises that position.
    write_manifest(manifest, JUDGE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "--dry-run", "pkg", ws, "--selection", "neither")
    assert result.returncode == 0
    assert not (pkg / "results").is_dir()
    assert "no file written" in result.output


def _parsed(tmp_path: Path, document: dict[str, Any]) -> contract.Manifest:
    path = tmp_path / "pkg" / "probe.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document), encoding="utf-8")
    return contract.parse_manifest(path)


def _view_file(tmp_path: Path, text: str) -> Path:
    path = tmp_path / "view.json"
    path.write_text(text + "\n", encoding="utf-8")
    return path


def _compute(
    manifest: contract.Manifest,
    ws: Path,
    *,
    view: str | None = None,
    selection: str | None = None,
    dry_run: bool = False,
) -> record.ComputedRecord:
    """`record.compute` over <ws>, judged as the view text <view> or the option <selection>."""
    result = JudgeResult(
        view=_view_file(ws, view) if view is not None else None, selection=selection
    )
    return record.compute(manifest, ws, result, dry_run=dry_run)


def test_compute_confirms_a_matching_projection(tmp_path: Path) -> None:
    manifest = _parsed(tmp_path, unattended_manifest())
    computed = _compute(manifest, tmp_path, view="1")
    assert computed.status == contract.Status.CONFIRMED
    assert computed.observed == 1
    assert computed.tool_version is None


def test_compute_refutes_a_mismatching_projection(tmp_path: Path) -> None:
    manifest = _parsed(tmp_path, unattended_manifest())
    computed = _compute(manifest, tmp_path, view="2")
    assert computed.status == contract.Status.REFUTED
    assert computed.observed == 2


def test_compute_takes_an_inconclusive_option_s_status(tmp_path: Path) -> None:
    manifest = _parsed(tmp_path, JUDGE_MANIFEST)
    computed = _compute(manifest, fresh_capture(tmp_path), selection="couldnt-tell")
    assert computed.status == contract.Status.INCONCLUSIVE
    assert computed.observed == "couldnt-tell"


def test_compute_refuses_a_projection_yielding_several_values(tmp_path: Path) -> None:
    manifest = _parsed(
        tmp_path, unattended_manifest(assertion={"projection": ".[]", "expected": 1})
    )
    with pytest.raises(record.RecordRefused) as exc:
        _compute(manifest, tmp_path, view="[1, 2]")
    assert exc.value.lines == (
        "projection must yield exactly one JSON value, but yielded several: .[]",
    )


def test_compute_writes_no_file_and_prints_nothing(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    manifest = _parsed(tmp_path, unattended_manifest())
    _compute(manifest, tmp_path, view="1")
    assert not (manifest.path.parent / "results").exists()
    captured = capsys.readouterr()
    assert captured.out == captured.err == ""


def test_write_puts_the_record_under_the_manifest_s_results_with_its_stamped_name(
    tmp_path: Path,
) -> None:
    manifest = _parsed(tmp_path, unattended_manifest())
    computed = _compute(manifest, tmp_path, view="1")
    written = record.write(computed)
    assert written == manifest.path.parent / "results" / f"{computed.stamp}-opencode-unknown.json"
    assert written.read_text(encoding="utf-8") == computed.text() + "\n"
    assert json.loads(computed.text())["date"] == computed.stamp.split("T", 1)[0]


def test_a_relative_workspace_path_is_read_from_the_working_directory(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, unattended_manifest())
    write_view_text(view, "1")

    result = cli("record", "pkg", view.parent.relative_to(tmp_path), cwd=tmp_path)
    assert result.returncode == 0, result.output
    (record_file,) = record_files(pkg)
    assert result.stdout == f"{record_file}\n"


def test_an_expected_true_refutes_an_observed_1(tmp_path: Path) -> None:
    manifest = _parsed(
        tmp_path, unattended_manifest(assertion={"projection": ".", "expected": True})
    )
    computed = _compute(manifest, tmp_path, view="1")
    assert computed.status == contract.Status.REFUTED


def test_a_projection_yielding_no_value_observes_null(tmp_path: Path) -> None:
    manifest = _parsed(
        tmp_path, unattended_manifest(assertion={"projection": "empty", "expected": None})
    )
    computed = _compute(manifest, tmp_path, view="1")
    assert computed.observed is None
    assert computed.status == contract.Status.CONFIRMED


def test_a_projection_record_s_document_is_the_seven_keys_in_order(tmp_path: Path) -> None:
    manifest = _parsed(
        tmp_path,
        unattended_manifest(
            depth="resolved-config",
            assertion={"projection": ".x", "expected": {"n": 1, "s": "é"}},
        ),
    )
    computed = _compute(manifest, tmp_path, view='{"x": {"s": "é", "n": 1}}')
    assert computed.text() == (
        "{\n"
        '  "schema_version": 1,\n'
        '  "provider": "opencode",\n'
        '  "status": "confirmed",\n'
        '  "depth": "resolved-config",\n'
        f'  "date": "{computed.record.date}",\n'
        '  "tool_version": null,\n'
        '  "assertion": {\n'
        '    "projection": ".x",\n'
        '    "expected": {\n'
        '      "n": 1,\n'
        '      "s": "é"\n'
        "    },\n"
        '    "observed": {\n'
        '      "s": "é",\n'
        '      "n": 1\n'
        "    }\n"
        "  }\n"
        "}"
    )


def test_an_observed_value_too_deep_for_the_record_model_is_refused(tmp_path: Path) -> None:
    manifest = _parsed(tmp_path, unattended_manifest())
    with pytest.raises(record.RecordRefused) as exc:
        _compute(manifest, tmp_path, view="[" * 300 + "1" + "]" * 300)
    assert exc.value.lines == ("the observed value cannot be written as a record",)


def _command_sourced(tmp_path: Path) -> contract.Manifest:
    # A command that cannot run: a judge must never run it.
    command = {"kind": "command", "command": str(tmp_path / "never-run")}
    return _parsed(tmp_path, unattended_manifest(version_source=command))


def test_a_command_source_s_version_is_read_from_the_workspace(tmp_path: Path) -> None:
    (tmp_path / "tool_version").write_text("6.6.6\n", encoding="utf-8")
    assert _compute(_command_sourced(tmp_path), tmp_path, view="1").tool_version == "6.6.6"


def test_a_workspace_without_tool_version_is_refused_on_a_real_record(tmp_path: Path) -> None:
    with pytest.raises(record.RecordRefused) as exc:
        _compute(_command_sourced(tmp_path), tmp_path, view="1")
    assert exc.value.lines == (
        f"no recorded tool version in {tmp_path}",
        "re-run the probe; a later judge never runs the version command.",
    )


def test_a_workspace_without_tool_version_reports_null_on_a_dry_run(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    computed = _compute(_command_sourced(tmp_path), tmp_path, view="1", dry_run=True)
    assert computed.tool_version is None
    assert f"no recorded tool version in {tmp_path}; the record would carry null" in (
        capsys.readouterr().err
    )


def test_a_written_record_is_its_parsed_record_s_dump(tmp_path: Path) -> None:
    manifest = _parsed(tmp_path, JUDGE_MANIFEST)
    computed = _compute(manifest, fresh_capture(tmp_path), selection="neither")
    written = record.write(computed)
    parsed = contract.parse_record(written)
    assert parsed is not None
    assert written.read_text(encoding="utf-8") == (
        json_values.dumps(parsed.model_dump(mode="json")) + "\n"
    )


def test_an_options_record_s_document_carries_each_option_s_declared_keys(tmp_path: Path) -> None:
    manifest = _parsed(tmp_path, JUDGE_MANIFEST)
    computed = _compute(manifest, fresh_capture(tmp_path), selection="neither")
    assert computed.record.model_dump(mode="json") == {
        "schema_version": 1,
        "provider": "cursor",
        "status": "confirmed",
        "depth": None,
        "date": computed.record.date,
        "tool_version": None,
        "assertion": {
            "options": [
                {"id": "both-markers", "text": "Both markers were visible"},
                {"id": "neither", "text": "Neither marker was visible"},
                {"id": "couldnt-tell", "text": "Could not determine", "status": "inconclusive"},
            ],
            "expected": "neither",
            "observed": "neither",
        },
    }
    assert list(computed.record.model_dump(mode="json")) == [
        "schema_version",
        "provider",
        "status",
        "depth",
        "date",
        "tool_version",
        "assertion",
    ]


# A custom probe whose definition-level judge function always refuses.
REFUSING_PROBE_PY = """\
from pathlib import Path
from probe_harness.families.base import CustomProbe, GateFailure, JudgeResult, ProbeFunctions

def _refuse(ws: Path) -> None:
    raise GateFailure("the apparatus check refused.")

PROBE = CustomProbe(required_tools=(), arrange=lambda ws, variant: None, act=lambda ws: None,
                    judge=lambda ws: JudgeResult(view=ws / "view.json", selection=None),
                    functions=ProbeFunctions(judge=_refuse))
"""


def test_a_workspace_its_gates_refuse_is_never_recorded(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    (pkg / "probe.py").write_text(REFUSING_PROBE_PY)
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = cli("record", "pkg", view.parent)
    assert result.returncode == 1
    assert "probe record: the apparatus check refused.\n" in result.stderr
    assert result.stderr.count("the workspace has been kept for inspection") == 1
    assert not (pkg / "results").is_dir()


def test_a_manual_workspace_whose_wait_for_does_not_hold_is_refused(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    write_manifest(manifest, {**MANUAL_PROJECTION_MANIFEST, "wait_for": "any(.[]; .never)"})
    ws = fresh_capture(tmp_path)

    result = cli("record", "pkg", ws)
    assert result.returncode == 1
    assert (
        "probe record: the capture does not satisfy the manifest's wait_for: any(.[]; .never)\n"
        in result.stderr
    )
    assert not (pkg / "results").is_dir()


def test_a_command_source_without_tool_version_is_refused_on_a_real_record(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    command = {"kind": "command", "command": "never-run --version"}
    write_manifest(manifest, {**SCRIPT_MANIFEST, "version_source": command})
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = cli("record", "pkg", view.parent)
    assert result.returncode == 1
    assert f"probe record: no recorded tool version in {view.parent}\n" in result.stderr
    assert not (pkg / "results").is_dir()

    dry = dry_run(cli, "pkg", view.parent)
    assert dry.returncode == 0, dry.stderr
    assert json.loads(dry.stdout)["tool_version"] is None
    assert f"no recorded tool version in {view.parent}; the record would carry null" in dry.stderr


def test_judging_one_workspace_twice_yields_records_equal_apart_from_the_date(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    command = {"kind": "command", "command": "never-run --version"}
    write_manifest(manifest, {**SCRIPT_MANIFEST, "version_source": command})
    write_view_text(view, '{"model":"m","variant":"high"}')
    (view.parent / "tool_version").write_text("1.2.3\n")

    assert cli("record", "pkg", view.parent).returncode == 0
    time.sleep(1)
    assert cli("record", "pkg", view.parent).returncode == 0

    first, second = (json.loads(path.read_text()) for path in record_files(pkg))
    del first["date"], second["date"]
    assert first == second
    assert first["tool_version"] == "1.2.3"


def test_a_discriminator_workspace_is_never_recorded(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')
    stamp_run(view.parent, variant="discriminator")

    result = cli("record", "pkg", view.parent)
    assert result.returncode == 1
    assert (
        f"probe record: {view.parent} ran the discriminator fixture, which is dry-run only; "
        "pass --dry-run\n" in result.stderr
    )
    assert not (pkg / "results").is_dir()
    assert dry_run(cli, "pkg", view.parent).returncode == 0


def test_another_package_s_workspace_is_refused_even_on_a_dry_run(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')
    stamp_run(view.parent, package="other")

    result = dry_run(cli, "pkg", view.parent)
    assert result.returncode == 1
    assert f"probe record: {view.parent} is a workspace of other, not pkg\n" in result.stderr


def test_a_workspace_without_run_json_can_only_be_judged_dry(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    # A workspace made before runs recorded what they arranged.
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')
    (view.parent / "run.json").unlink()

    result = cli("record", "pkg", view.parent)
    assert result.returncode == 1
    assert "records no package or variant (run.json)" in result.stderr
    assert not (pkg / "results").is_dir()
    assert dry_run(cli, "pkg", view.parent).returncode == 0


def test_probe_dry_run_makes_probe_record_write_nothing(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = cli("record", "pkg", view.parent, env={"PROBE_DRY_RUN": "1"})
    assert result.returncode == 0, result.stderr
    assert "no file written" in result.stderr
    assert not (pkg / "results").is_dir()

    malformed = cli("record", "pkg", view.parent, env={"PROBE_DRY_RUN": "yes"})
    assert malformed.returncode == 2
    assert "probe record: PROBE_DRY_RUN must be 0 or 1 (got: yes)\n" in malformed.stderr
