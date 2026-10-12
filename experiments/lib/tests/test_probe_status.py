"""Coverage for `probe status`, driven against fixture package trees rather than the
repository's own records, so the assertions do not shift as real probes are added."""

import json
from pathlib import Path
from typing import Any

from tests.conftest import Cli, Result


def put_record(
    tree: Path, package: str, stamp: str, provider: str, record_status: str, version: str
) -> None:
    """Write a record into <package>/results/<stamp>-<provider>-<version>.json."""
    results = tree / package / "results"
    results.mkdir(parents=True, exist_ok=True)
    record = {
        "schema_version": 1,
        "provider": provider,
        "status": record_status,
        "depth": None,
        "date": stamp.split("T", 1)[0],
        "tool_version": version,
        "assertion": {"projection": ".", "expected": 1, "observed": 1},
    }
    (results / f"{stamp}-{provider}-{version}.json").write_text(json.dumps(record) + "\n")


def put_manifest(tree: Path, package: str, content: str) -> None:
    (tree / package).mkdir(parents=True, exist_ok=True)
    (tree / package / "probe.json").write_text(content)


def write_manifest(tree: Path, package: str, manifest: dict[str, Any]) -> None:
    put_manifest(tree, package, json.dumps(manifest, indent=2) + "\n")


def put_version_stub(tree: Path, package: str, version: str, driver: str = "unattended") -> None:
    """A manifest whose version command is a stub binary printing <version>.

    The command is exec'd as argv rather than through a shell, so a version fixture must be a
    real executable rather than a quoted `printf`.
    """
    (tree / package).mkdir(parents=True, exist_ok=True)
    stub = tree / package / "version-stub"
    stub.write_text(f'#!/usr/bin/env bash\nprintf "%s\\n" "{version}"\n')
    stub.chmod(0o755)
    write_manifest(
        tree,
        package,
        {
            "schema_version": 1,
            "provider": "opencode",
            "driver": driver,
            "depth": None,
            "question": "q",
            "version_source": {"kind": "command", "command": str(stub)},
            "assertion": {"projection": ".", "expected": 1},
        },
    )


def put_billed_manifest(tree: Path, package: str) -> None:
    """A manifest with no records — the state a billed package spends most of its life in, and
    the one the record loop passes over entirely."""
    write_manifest(
        tree,
        package,
        {
            "schema_version": 1,
            "provider": "claude",
            "driver": "billed",
            "depth": "outbound-request",
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {"projection": ".", "expected": 1},
        },
    )


def lines(result: Result) -> list[str]:
    """bats' `lines`: the merged output split on newlines, blank lines dropped."""
    return [line for line in result.output.splitlines() if line]


def test_the_newest_record_for_a_probe_is_the_one_reported(cli: Cli, tree: Path) -> None:
    put_record(tree, "alpha", "2026-01-01T000000", "opencode", "refuted", "1.0.0")
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "2.0.0")

    result = cli("status")
    assert result.returncode == 0
    assert "2026-06-01" in result.output
    assert "2026-01-01" not in result.output
    assert "confirmed" in result.output
    assert "REFUTED" not in result.output


def test_the_provider_set_is_derived_from_the_records_inventing_no_rows(
    cli: Cli, tree: Path
) -> None:
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "2.0.0")

    result = cli("status")
    assert result.returncode == 0
    assert "OPENCODE" in result.output
    assert "CURSOR" not in result.output
    assert "CLAUDE" not in result.output


def test_a_probe_s_reported_name_comes_from_its_directory(cli: Cli, tree: Path) -> None:
    # Two otherwise-identical records under different package names must produce two rows,
    # each under its own name.
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "2.0.0")
    put_record(tree, "beta", "2026-06-01T000000", "opencode", "confirmed", "2.0.0")

    result = cli("status")
    assert result.returncode == 0
    assert "alpha" in result.output
    assert "beta" in result.output
    assert "2 recorded" in result.output


def test_an_inconclusive_record_renders_visibly_distinct_from_a_confirmed_one(
    cli: Cli, tree: Path
) -> None:
    # Reading "couldn't tell" as a pass would defeat the option's purpose.
    put_record(tree, "alpha", "2026-06-01T000000", "cursor", "confirmed", "3.0.0")
    put_record(tree, "beta", "2026-06-01T000000", "cursor", "inconclusive", "3.0.0")

    result = cli("status")
    assert result.returncode == 0
    assert "INCONCLUSIVE" in result.output
    assert "1 inconclusive" in result.output


def test_a_refuted_record_is_surfaced_as_recorded_assertion_drift(cli: Cli, tree: Path) -> None:
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "refuted", "2.0.0")

    result = cli("status")
    assert result.returncode == 0
    assert "REFUTED" in result.output
    assert "1 refuted" in result.output


def test_a_version_source_command_absent_from_path_reports_not_installed(
    cli: Cli, tree: Path
) -> None:
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "2.0.0")
    write_manifest(
        tree,
        "alpha",
        {
            "schema_version": 1,
            "provider": "opencode",
            "driver": "unattended",
            "depth": None,
            "question": "q",
            "version_source": {
                "kind": "command",
                "command": "definitely-not-a-real-tool --version",
            },
            "assertion": {"projection": ".", "expected": 1},
        },
    )

    result = cli("status")
    assert result.returncode == 0
    assert "not installed" in result.output
    # An absent tool is not staleness.
    assert "0 version drift" in result.output


def test_a_capture_sourced_version_computes_no_drift(cli: Cli, tree: Path) -> None:
    put_record(tree, "alpha", "2026-06-01T000000", "cursor", "confirmed", "3.16.17")
    write_manifest(
        tree,
        "alpha",
        {
            "schema_version": 1,
            "provider": "cursor",
            "driver": "manual",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "capture", "jq": "[.[] | .cursor_version] | first"},
            "assertion": {"projection": ".", "expected": 1},
        },
    )

    result = cli("status")
    assert result.returncode == 0
    assert "from capture" in result.output
    assert "0 version drift" in result.output


def test_a_version_string_beginning_with_a_sentinel_word_still_counts_as_drift(
    cli: Cli, tree: Path
) -> None:
    # The display string and the not-comparable signal must not share a namespace, or a real
    # version starting with the sentinel is silently treated as incomparable and genuine
    # staleness goes uncounted.
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "1.0.0")
    put_version_stub(tree, "alpha", "unknown-build-42")

    result = cli("status")
    assert result.returncode == 0
    assert "installed unknown-build-42" in result.output
    assert "1 version drift" in result.output


def test_a_malformed_manifest_is_reported_not_silently_read_as_a_human_check(
    cli: Cli, tree: Path
) -> None:
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "1.0.0")
    put_manifest(tree, "alpha", "BROKEN {{{ not json\n")

    result = cli("status")
    assert result.returncode == 0
    assert "manifest unreadable" in result.output
    assert "human check" not in result.output


def test_a_version_command_reading_stdin_does_not_wedge_the_report(cli: Cli, tree: Path) -> None:
    # stdin is closed for the version command, so a command that reads it returns immediately
    # instead of blocking a build nobody is watching.
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "1.0.0")
    write_manifest(
        tree,
        "alpha",
        {
            "schema_version": 1,
            "provider": "opencode",
            "driver": "unattended",
            "depth": None,
            "question": "q",
            "version_source": {"kind": "command", "command": "cat"},
            "assertion": {"projection": ".", "expected": 1},
        },
    )

    result = cli("status", "--summary")
    assert result.returncode == 0
    assert "recorded" in result.output


def test_an_unrelated_json_file_in_results_is_not_mistaken_for_the_newest_record(
    cli: Cli, tree: Path
) -> None:
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "2.0.0")
    (tree / "alpha" / "results" / "notes.json").write_text('{"note":"scratch"}\n')

    result = cli("status")
    assert result.returncode == 0
    assert "confirmed" in result.output
    assert "2.0.0" in result.output
    assert "1 recorded" in result.output


def test_an_unknown_argument_exits_0_rather_than_failing_the_build(cli: Cli, tree: Path) -> None:
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "2.0.0")

    result = cli("status", "--bogus")
    assert result.returncode == 0


def test_an_installed_version_differing_from_the_recorded_one_counts_as_drift(
    cli: Cli, tree: Path
) -> None:
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "1.0.0")
    put_version_stub(tree, "alpha", "9.9.9")

    result = cli("status")
    assert result.returncode == 0
    assert "installed 9.9.9" in result.output
    assert "1 version drift" in result.output


def test_a_package_with_no_manifest_still_has_its_records_reported(cli: Cli, tree: Path) -> None:
    # Defense-in-depth: the report is derived from records, so it does not depend on a
    # manifest existing. Every probe package has one regardless.
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "2.0.0")

    result = cli("status")
    assert result.returncode == 0
    assert "alpha" in result.output
    assert "1 recorded" in result.output


def test_malformed_json_in_a_records_directory_does_not_break_the_exit_code(
    cli: Cli, tree: Path
) -> None:
    # The property `just check` depends on.
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "2.0.0")
    broken = tree / "broken" / "results"
    broken.mkdir(parents=True)
    (broken / "2026-06-01T000000-opencode-1.json").write_text("not json at all {{{\n")

    result = cli("status")
    assert result.returncode == 0
    assert "alpha" in result.output
    # The broken record is skipped rather than counted.
    assert "1 recorded" in result.output


def test_an_empty_tree_exits_0_and_says_so(cli: Cli, tree: Path) -> None:
    result = cli("status")
    assert result.returncode == 0
    assert "No probe records" in result.output


def test_summary_emits_exactly_one_line_with_counts_matching_the_tree(cli: Cli, tree: Path) -> None:
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "2.0.0")
    put_record(tree, "beta", "2026-06-01T000000", "cursor", "refuted", "3.0.0")
    put_record(tree, "gamma", "2026-06-01T000000", "cursor", "inconclusive", "3.0.0")

    result = cli("status", "--summary")
    assert result.returncode == 0
    assert len(lines(result)) == 1
    assert "3 recorded" in result.output
    assert "1 refuted" in result.output
    assert "1 inconclusive" in result.output


def test_summary_exits_0_on_an_empty_tree(cli: Cli, tree: Path) -> None:
    result = cli("status", "--summary")
    assert result.returncode == 0
    assert len(lines(result)) == 1
    assert "0 recorded" in result.output


def test_a_billed_package_s_version_drift_is_excluded_from_the_count_and_named_as_billed(
    cli: Cli, tree: Path
) -> None:
    # A batch run never refreshes a billed package, so its drift is permanent. Counting it
    # would make the total structurally unable to reach zero, which hides the genuine drift of
    # a package a free run could clear.
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "1.0.0")
    put_version_stub(tree, "alpha", "9.9.9", "billed")

    result = cli("status")
    assert result.returncode == 0
    assert "installed 9.9.9" in result.output
    assert "just probe-run --billed" in result.output
    assert "0 version drift" in result.output
    assert "1 billed (drift not tracked)" in result.output


def test_a_billed_package_s_bucket_does_not_suppress_an_unattended_package_s_drift(
    cli: Cli, tree: Path
) -> None:
    # The failure the separate bucket exists to prevent: a permanent-drift package must not
    # swallow the signal from one a batch run can clear.
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "1.0.0")
    put_version_stub(tree, "alpha", "9.9.9", "billed")
    put_record(tree, "beta", "2026-06-01T000000", "opencode", "confirmed", "1.0.0")
    put_version_stub(tree, "beta", "8.8.8", "unattended")

    result = cli("status")
    assert result.returncode == 0
    assert "1 version drift" in result.output
    assert "1 billed" in result.output


def test_a_billed_package_with_no_results_directory_still_renders_a_row(
    cli: Cli, tree: Path
) -> None:
    # The record loop skips a package with no results/, which is exactly the state a billed
    # package is most likely to be in — and no batch run will ever move it out of that state.
    put_billed_manifest(tree, "alpha")

    result = cli("status")
    assert result.returncode == 0
    assert "alpha" in result.output
    assert "not yet run" in result.output
    assert "outbound-request" in result.output
    assert "just probe-run --billed" in result.output
    assert "No probe records" not in result.output
    assert "0 recorded" in result.output
    assert "1 billed" in result.output


def test_a_billed_package_that_has_recorded_is_not_also_given_a_not_yet_run_row(
    cli: Cli, tree: Path
) -> None:
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "1.0.0")
    put_version_stub(tree, "alpha", "1.0.0", "billed")

    result = cli("status")
    assert result.returncode == 0
    assert "not yet run" not in result.output
    assert "1 recorded" in result.output
    assert "1 billed" in result.output


def test_a_tree_with_no_billed_packages_prints_the_summary_line_without_a_billed_segment(
    cli: Cli, tree: Path
) -> None:
    # The segment is omitted rather than printed as zero, so a tree that has never declared a
    # billed package keeps the line it had before.
    put_record(tree, "alpha", "2026-06-01T000000", "opencode", "confirmed", "2.0.0")

    result = cli("status")
    assert result.returncode == 0
    assert "1 recorded" in result.output
    assert "billed" not in result.output


def test_summary_works_while_a_billed_package_is_present(cli: Cli, tree: Path) -> None:
    # The billed manifest scan feeds the billed count the `--summary` early exit reads; a
    # refactor hoisting that exit above the scan would lose it, and the four tests above all
    # take the full-report path — so nothing else pins the ordering.
    put_billed_manifest(tree, "alpha")

    result = cli("status", "--summary")
    assert result.returncode == 0
    assert len(lines(result)) == 1
    assert "1 billed (drift not tracked)" in result.output


def test_a_billed_package_holding_an_unreadable_record_is_not_labelled_not_yet_run(
    cli: Cli, tree: Path
) -> None:
    # It has been run. Claiming otherwise would be a false statement about the package, rather
    # than the invisibility an unparseable record earns every other package in the report.
    put_billed_manifest(tree, "alpha")
    results = tree / "alpha" / "results"
    results.mkdir(parents=True)
    (results / "2026-06-01T000000-claude-1.json").write_text("not json at all {{{\n")

    result = cli("status")
    assert result.returncode == 0
    assert "not yet run" not in result.output
    assert "1 billed" in result.output
