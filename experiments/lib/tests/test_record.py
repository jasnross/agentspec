"""Coverage for `probe record`: comparison, freshness refusal, selection handling, and record
well-formedness. No provider CLI is involved: every oracle output here is hand-written.
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


@pytest.fixture
def pkg(tmp_path: Path) -> Path:
    package = tmp_path / "pkg"
    package.mkdir()
    return package


@pytest.fixture
def manifest(pkg: Path) -> Path:
    return pkg / "probe.json"


@pytest.fixture
def view(tmp_path: Path) -> Path:
    return tmp_path / "view.json"


def write_manifest(path: Path, manifest: dict[str, Any]) -> None:
    path.write_text(json.dumps(manifest, indent=2) + "\n")


def write_view_text(path: Path, text: str) -> None:
    path.write_text(text + "\n")


def fresh_capture(tmp_path: Path) -> Path:
    """A workspace shaped the way the harness shapes one, carrying a Cursor hook payload."""
    ws = tmp_path / "ws"
    (ws / "capture").mkdir(parents=True, exist_ok=True)
    (ws / "capture" / "payloads.jsonl").write_text(
        '{"cursor_version":"3.16.17","model":"m","variant":"high"}\n'
    )
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
    """`probe record --dry-run <args…>`: stdout is the would-be record, stderr the diagnostic."""
    return cli("record", "--dry-run", *args)


def test_a_matching_observed_value_records_confirmed(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = cli("record", "--manifest", manifest, "--view", view)
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "confirmed"


def test_a_mismatching_observed_value_records_refuted(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"low"}')

    result = cli("record", "--manifest", manifest, "--view", view)
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

    result = cli("record", "--manifest", manifest, "--view", view)
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "confirmed"


def test_a_manual_projection_manifest_without_capture_is_refused(
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
            "assertion": {"projection": ".", "expected": {}},
        },
    )
    write_view_text(view, "{}")

    result = cli("record", "--manifest", manifest, "--view", view)
    assert result.returncode != 0
    assert "requires --capture" in result.output


def test_a_manual_options_manifest_without_capture_is_refused(cli: Cli, manifest: Path) -> None:
    write_manifest(manifest, JUDGE_MANIFEST)

    result = cli("record", "--manifest", manifest, "--selection", "neither")
    assert result.returncode != 0
    assert "requires --capture" in result.output


def test_a_selection_matching_expected_records_confirmed(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    write_manifest(manifest, JUDGE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "--manifest", manifest, "--selection", "neither", "--capture", ws)
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "confirmed"
    assert the_record(pkg)["assertion"]["observed"] == "neither"


def test_a_selection_not_matching_expected_records_refuted(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    write_manifest(manifest, JUDGE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "--manifest", manifest, "--selection", "both-markers", "--capture", ws)
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "refuted"
    assert the_record(pkg)["assertion"]["observed"] == "both-markers"


def test_the_couldnt_tell_option_records_inconclusive(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    # Without this an operator who could not tell would produce a false pass.
    write_manifest(manifest, JUDGE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "--manifest", manifest, "--selection", "couldnt-tell", "--capture", ws)
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

    result = cli("record", "--manifest", manifest, "--selection", "yes", "--capture", ws)
    assert result.returncode != 0
    assert 'only status "inconclusive"' in result.output


def test_several_inconclusive_options_do_not_stop_the_comparison(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    write_manifest(manifest, MULTI_INCONCLUSIVE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "--manifest", manifest, "--selection", "neither", "--capture", ws)
    assert result.returncode == 0
    assert the_record(pkg)["status"] == "confirmed"


def test_an_inconclusive_option_yields_inconclusive_wherever_it_sits_in_the_set(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    # `not-reached` is the second option declaring the status, so a gate reading only the
    # first would miss it.
    write_manifest(manifest, MULTI_INCONCLUSIVE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "--manifest", manifest, "--selection", "not-reached", "--capture", ws)
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

    result = cli("record", "--manifest", manifest, "--view", view)
    assert result.returncode != 0
    assert "depth must be" in result.output


def test_a_selection_absent_from_the_option_set_is_refused(
    cli: Cli, tmp_path: Path, manifest: Path
) -> None:
    write_manifest(manifest, JUDGE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "--manifest", manifest, "--selection", "typo", "--capture", ws)
    assert result.returncode != 0
    assert "not one of the manifest's option ids" in result.output


def test_supplying_both_view_and_selection_is_refused(cli: Cli, manifest: Path, view: Path) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, "{}")

    result = cli("record", "--manifest", manifest, "--view", view, "--selection", "neither")
    assert result.returncode != 0
    assert "mutually exclusive" in result.output


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

    result = cli("record", "--manifest", manifest, "--selection", "neither", "--capture", ws)
    assert result.returncode == 0

    record_file = first_record_file(pkg)
    assert json.loads(record_file.read_text())["depth"] is None
    record_wellformed(record_file)


def test_a_capture_with_no_payloads_file_is_refused(
    cli: Cli, tmp_path: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')
    ws = tmp_path / "nopayloads"
    (ws / "capture").mkdir(parents=True)

    result = cli("record", "--manifest", manifest, "--view", view, "--capture", ws)
    assert result.returncode != 0
    assert "absent or empty" in result.output


def test_a_capture_with_an_empty_payloads_file_is_refused(
    cli: Cli, tmp_path: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')
    ws = tmp_path / "emptypayloads"
    (ws / "capture").mkdir(parents=True)
    (ws / "capture" / "payloads.jsonl").write_text("")

    result = cli("record", "--manifest", manifest, "--view", view, "--capture", ws)
    assert result.returncode != 0
    assert "absent or empty" in result.output


def test_a_fresh_capture_records_successfully_and_stores_nothing_about_the_capture(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path, view: Path
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
    payloads = (ws / "capture" / "payloads.jsonl").read_text().splitlines()
    view.write_text(json.dumps([json.loads(line) for line in payloads if line], indent=2) + "\n")

    result = cli("record", "--manifest", manifest, "--view", view, "--capture", ws)
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

    result = cli("record", "--manifest", manifest, "--view", view)
    assert result.returncode == 0

    record_wellformed(first_record_file(pkg))


def test_date_is_stamped_in_utc(cli: Cli, pkg: Path, manifest: Path, view: Path) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    # Run under a timezone 14 hours ahead of UTC, so for most of the day a local-clock stamp
    # would produce tomorrow's date and fail this assertion. The clock is read either side of
    # the run so a genuine UTC-midnight crossing tolerates the boundary instead of flaking.
    before = datetime.datetime.now(datetime.UTC).date().isoformat()
    result = cli("record", "--manifest", manifest, "--view", view, env={"TZ": "Pacific/Kiritimati"})
    after = datetime.datetime.now(datetime.UTC).date().isoformat()
    assert result.returncode == 0

    assert the_record(pkg)["date"] in (before, after)


def test_a_record_carries_no_key_outside_the_seven_key_contract(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = cli("record", "--manifest", manifest, "--view", view)
    assert result.returncode == 0

    record = json.loads(first_record_file(pkg).read_text())
    for key in ("probe", "kind", "driver", "capture", "finding", "blocked_reason"):
        assert key not in record


def test_a_path_hostile_version_string_is_sanitized_in_the_record_filename(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path, view: Path
) -> None:
    # The version command is exec'd as argv, not through a shell, so the hostile string comes
    # from a stub binary rather than shell quoting.
    stub = tmp_path / "hostile-version"
    stub.write_text('#!/usr/bin/env bash\nprintf "1.0/../etc x\\n"\n')
    stub.chmod(0o755)

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

    result = cli("record", "--manifest", manifest, "--view", view)
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

    result = cli("record", "--manifest", manifest, "--view", view)
    assert result.returncode != 0
    assert "declares no expected value" in result.output
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

    result = cli("record", "--manifest", manifest, "--view", view)
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

        result = cli("record", "--manifest", manifest, "--selection", "yes", "--capture", ws)
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

    result = cli("record", "--manifest", manifest, "--view", view)
    assert result.returncode != 0
    assert "exactly one of projection or options" in result.output


def test_an_options_manifest_invoked_with_view_is_refused_not_recorded_as_refuted(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    # A wiring error must not manufacture the strong signal.
    write_manifest(manifest, JUDGE_MANIFEST)
    write_view_text(view, "{}")

    result = cli("record", "--manifest", manifest, "--view", view)
    assert result.returncode != 0
    assert "use --selection" in result.output
    assert not (pkg / "results").is_dir()


def test_a_projection_manifest_invoked_with_selection_is_refused(
    cli: Cli, tmp_path: Path, manifest: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli("record", "--manifest", manifest, "--selection", "anything", "--capture", ws)
    assert result.returncode != 0
    assert "use --view" in result.output


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

    result = cli("record", "--manifest", manifest, "--view", view)
    assert result.returncode != 0
    assert "provider must be" in result.output


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

    result = cli("record", "--manifest", manifest, "--view", view)
    assert result.returncode != 0
    assert "schema_version must be 1" in result.output


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

    result = cli("record", "--manifest", manifest, "--view", view)
    assert result.returncode != 0
    assert "driver must be unattended, billed, or manual" in result.output
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

    result = cli("record", "--manifest", manifest, "--view", view)
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

    result = cli("record", "--manifest", manifest, "--selection", "dup", "--capture", ws)
    assert result.returncode != 0
    assert "2 times" in result.output


def test_two_runs_on_the_same_day_at_the_same_version_produce_two_records(
    cli: Cli, pkg: Path, manifest: Path, view: Path
) -> None:
    # The append-only property the filename's time component exists to guarantee: re-running
    # a refuted probe to confirm it must not overwrite the evidence.
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    cli("record", "--manifest", manifest, "--view", view)
    time.sleep(1)
    cli("record", "--manifest", manifest, "--view", view)

    assert len(record_files(pkg)) == 2


def test_dry_run_writes_no_record(cli: Cli, pkg: Path, manifest: Path, view: Path) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')

    result = dry_run(cli, "--manifest", manifest, "--view", view)
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

    result = dry_run(cli, "--manifest", manifest, "--view", view)
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

    result = dry_run(cli, "--manifest", manifest, "--view", view)
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

    result = dry_run(cli, "--manifest", manifest, "--view", view)
    assert result.returncode != 0
    assert "depth must be" in result.stderr
    # A refused dry run emits no would-be record, so a scripted `--dry-run >file` cannot leave
    # a partial artifact behind.
    assert result.stdout == ""


def test_dry_run_still_enforces_the_capture_non_empty_check(
    cli: Cli, tmp_path: Path, manifest: Path, view: Path
) -> None:
    write_manifest(manifest, SCRIPT_MANIFEST)
    write_view_text(view, '{"model":"m","variant":"high"}')
    ws = tmp_path / "emptypayloads"
    (ws / "capture").mkdir(parents=True)
    (ws / "capture" / "payloads.jsonl").write_text("")

    result = dry_run(cli, "--manifest", manifest, "--view", view, "--capture", ws)
    assert result.returncode != 0
    assert "absent or empty" in result.stderr
    assert result.stdout == ""


def test_dry_run_records_nothing_on_the_selection_path_either(
    cli: Cli, tmp_path: Path, pkg: Path, manifest: Path
) -> None:
    # The options path is the expensive one to get wrong: a human-judged probe discovers a
    # wiring error only during a live session. The flag is passed last here on purpose: both
    # usage lines show it trailing, and the helper above always passes it first, so nothing
    # else exercises that position.
    write_manifest(manifest, JUDGE_MANIFEST)
    ws = fresh_capture(tmp_path)

    result = cli(
        "record", "--manifest", manifest, "--selection", "neither", "--capture", ws, "--dry-run"
    )
    assert result.returncode == 0
    assert not (pkg / "results").is_dir()
    assert "no file written" in result.output


def test_a_non_object_version_source_fails_with_jq_s_diagnostic(cli: Cli, tmp_path: Path) -> None:
    package = tmp_path / "pkg"
    package.mkdir()
    manifest = package / "probe.json"
    manifest.write_text(json.dumps(unattended_manifest(version_source="command")), encoding="utf-8")
    view = tmp_path / "view.json"
    view.write_text("1\n", encoding="utf-8")
    result = cli("record", "--manifest", manifest, "--view", view, "--dry-run")
    assert result.returncode == 5
    assert "Cannot index string" in result.stderr
    assert result.stdout == ""


def test_a_relative_manifest_path_records_under_the_logical_working_directory(
    cli: Cli, tmp_path: Path
) -> None:
    real = tmp_path / "real"
    (real / "pkg").mkdir(parents=True)
    (real / "pkg" / "probe.json").write_text(json.dumps(unattended_manifest()), encoding="utf-8")
    (real / "view.json").write_text("1\n", encoding="utf-8")
    link = tmp_path / "link"
    link.symlink_to(real)
    result = cli(
        "record",
        "--manifest",
        "pkg/probe.json",
        "--view",
        "view.json",
        cwd=link,
        env={"PWD": str(link)},
    )
    assert result.returncode == 0
    assert result.stdout.startswith(f"{link}/pkg/results/")
