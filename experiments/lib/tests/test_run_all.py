"""Coverage for `probe run`'s batch loop, driven with stub `probe.py` files rather than real
providers. The Testing Strategy exempts per-probe scripts from unit tests; this loop is the
exception, because it is the only branching logic the batch path adds.

The two axes the loop composes — selection (`--stale`) and authorization (`--billed`,
`--manual`, `--all`) — are exercised independently and together, because their independence
is the property the design rests on.
"""

import json
from collections.abc import Callable
from pathlib import Path

from tests.conftest import (
    Cli,
)

type PutPackage = Callable[..., Path]


def put_version_stub(tree: Path, name: str, installed: str) -> None:
    """Give an existing package a `command` version source resolving to <installed>.

    The command is exec'd as argv rather than through a shell, so it has to be a real
    executable.
    """
    stub = tree / name / "version-stub"
    stub.write_text(f"#!/bin/sh\nprintf '%s\\n' '{installed}'\n")
    stub.chmod(0o755)
    manifest = tree / name / "probe.json"
    data = json.loads(manifest.read_text())
    data["version_source"] = {"kind": "command", "command": str(stub)}
    manifest.write_text(json.dumps(data, indent=2) + "\n")


def put_record(tree: Path, name: str, version: str) -> None:
    """A record pinning <version>, so freshness has something to compare against."""
    results = tree / name / "results"
    results.mkdir(parents=True, exist_ok=True)
    record = {
        "schema_version": 1,
        "provider": "opencode",
        "status": "confirmed",
        "depth": None,
        "date": "2026-01-01",
        "tool_version": version,
        "assertion": {"projection": ".", "expected": 1, "observed": 1},
    }
    path = results / f"2026-01-01T000000-opencode-{version}.json"
    path.write_text(json.dumps(record, indent=2) + "\n")


def test_an_unattended_package_runs(cli: Cli, put_package: PutPackage) -> None:
    put_package("alpha", "unattended")

    result = cli("run")
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "1 ran" in result.output


def test_manual_packages_are_listed_as_skipped_with_a_readme_pointer(
    cli: Cli, put_package: PutPackage
) -> None:
    put_package("alpha", "manual")
    put_package("beta", "manual")

    result = cli("run")
    assert result.returncode == 0
    assert "skipped" in result.output
    assert "alpha/README.md" in result.output
    assert "beta/README.md" in result.output
    assert "2 skipped" in result.output
    # Their runners must not have been invoked.
    assert "ran alpha" not in result.output
    assert "ran beta" not in result.output


def test_a_billed_package_is_withheld_by_default_naming_the_flag_that_frees_it(
    cli: Cli, put_package: PutPackage
) -> None:
    # The default has to withhold: a batch run that costs money on every invocation is a
    # batch run people stop invoking.
    put_package("alpha", "billed")

    result = cli("run")
    assert result.returncode == 0
    assert "driver billed" in result.output
    assert "authorize with --billed" in result.output
    assert "1 skipped" in result.output
    assert "1 need --billed" in result.output
    assert "0 need --manual" in result.output
    # The money-spending part must not have been invoked.
    assert "ran alpha" not in result.output


def test_a_billed_package_runs_under_billed(cli: Cli, put_package: PutPackage) -> None:
    put_package("alpha", "billed")

    result = cli("run", "--billed")
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "1 ran" in result.output
    assert "0 need --billed" in result.output


def test_a_manual_package_is_withheld_by_default_naming_the_flag_that_frees_it(
    cli: Cli, put_package: PutPackage
) -> None:
    put_package("alpha", "manual")

    result = cli("run")
    assert result.returncode == 0
    assert "driver manual" in result.output
    assert "authorize with --manual" in result.output
    assert "1 need --manual" in result.output
    assert "ran alpha" not in result.output


def test_a_manual_package_runs_under_manual(cli: Cli, put_package: PutPackage) -> None:
    # Every manual probe blocks through a human flow in production; the stub stands in for
    # that, so what is pinned here is the authorization decision rather than the human flow.
    put_package("alpha", "manual")

    result = cli("run", "--manual")
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "1 ran" in result.output


def test_billed_does_not_authorize_manual_and_manual_does_not_authorize_billed(
    cli: Cli, put_package: PutPackage
) -> None:
    # The whole point of a set over a single escalating level: authorizing one cost must
    # not silently authorize a different one.
    put_package("alpha", "billed")
    put_package("beta", "manual")

    result = cli("run", "--billed")
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "ran beta" not in result.output

    result = cli("run", "--manual")
    assert result.returncode == 0
    assert "ran alpha" not in result.output
    assert "ran beta" in result.output


def test_authorization_flags_stack(cli: Cli, put_package: PutPackage) -> None:
    put_package("alpha", "billed")
    put_package("beta", "manual")
    put_package("gamma", "unattended")

    result = cli("run", "--billed", "--manual")
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "ran beta" in result.output
    assert "ran gamma" in result.output
    assert "3 ran" in result.output
    assert "0 skipped" in result.output


def test_authorization_flags_are_order_independent(cli: Cli, put_package: PutPackage) -> None:
    # Each flag adds to a set rather than setting a level, so no ordering can make one flag
    # overwrite another.
    put_package("alpha", "billed")
    put_package("beta", "manual")

    result = cli("run", "--manual", "--billed")
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "ran beta" in result.output
    assert "2 ran" in result.output


def test_a_repeated_flag_authorizes_the_same_driver_once(cli: Cli, put_package: PutPackage) -> None:
    put_package("alpha", "billed")

    result = cli("run", "--billed", "--billed")
    assert result.returncode == 0
    assert "1 ran" in result.output


def test_all_authorizes_every_driver(cli: Cli, put_package: PutPackage) -> None:
    put_package("alpha", "unattended")
    put_package("beta", "billed")
    put_package("gamma", "manual")

    result = cli("run", "--all")
    assert result.returncode == 0
    assert "3 ran" in result.output
    assert "0 skipped" in result.output


def test_probe_authorize_drivers_seeds_the_same_set_the_flags_add_to(
    cli: Cli, put_package: PutPackage
) -> None:
    # The environment form exists so a caller that cannot pass arguments — anything
    # invoking the script rather than the recipe — can still opt in.
    put_package("alpha", "billed")
    put_package("beta", "manual")

    result = cli("run", env={"PROBE_AUTHORIZE_DRIVERS": "billed manual"})
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "ran beta" in result.output
    assert "2 ran" in result.output


def test_probe_authorize_drivers_composes_with_a_flag_rather_than_replacing_it(
    cli: Cli, put_package: PutPackage
) -> None:
    put_package("alpha", "billed")
    put_package("beta", "manual")

    result = cli("run", "--manual", env={"PROBE_AUTHORIZE_DRIVERS": "billed"})
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "ran beta" in result.output
    assert "2 ran" in result.output


def test_probe_authorize_drivers_naming_an_unknown_driver_exits_2(
    cli: Cli, put_package: PutPackage
) -> None:
    # A typo'd driver name must not silently authorize nothing and look like a run that
    # simply found nothing to do.
    put_package("alpha", "billed")

    result = cli("run", env={"PROBE_AUTHORIZE_DRIVERS": "biled"})
    assert result.returncode == 2
    assert "unknown driver" in result.output
    assert "ran alpha" not in result.output


def test_the_summary_breakdown_reports_each_withheld_driver_separately(
    cli: Cli, put_package: PutPackage
) -> None:
    # A single count would let the parenthetical claim every skip had one cause, and the
    # reader could not tell which flag to reach for.
    put_package("alpha", "manual")
    put_package("beta", "billed")
    put_package("gamma", "billed")

    result = cli("run")
    assert result.returncode == 0
    assert "0 ran" in result.output
    assert "3 skipped (2 need --billed · 1 need --manual)" in result.output


def test_an_unknown_argument_exits_2_without_running_any_package(
    cli: Cli, put_package: PutPackage
) -> None:
    put_package("alpha", "unattended")

    result = cli("run", "--bogus")
    assert result.returncode == 2
    assert "unknown argument" in result.output
    assert "--billed --manual --all --stale" in result.output
    assert "ran alpha" not in result.output


def test_stale_runs_a_package_that_has_never_recorded(cli: Cli, put_package: PutPackage) -> None:
    put_package("alpha", "unattended")

    result = cli("run", "--stale")
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "1 ran" in result.output
    assert "0 fresh" in result.output


def test_stale_runs_a_package_whose_installed_version_has_moved(
    cli: Cli, tree: Path, put_package: PutPackage
) -> None:
    put_package("alpha", "unattended")
    put_version_stub(tree, "alpha", "2.0.0")
    put_record(tree, "alpha", "1.0.0")

    result = cli("run", "--stale")
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "1 ran" in result.output


def test_stale_passes_over_a_package_recorded_at_the_installed_version(
    cli: Cli, tree: Path, put_package: PutPackage
) -> None:
    put_package("alpha", "unattended")
    put_version_stub(tree, "alpha", "1.0.0")
    put_record(tree, "alpha", "1.0.0")

    result = cli("run", "--stale")
    assert result.returncode == 0
    assert "ran alpha" not in result.output
    assert "fresh" in result.output
    assert "1.0.0 unchanged" in result.output
    assert "0 ran" in result.output
    assert "1 skipped (0 need --billed · 0 need --manual · 1 fresh)" in result.output


def test_stale_treats_an_incomparable_version_as_fresh_rather_than_as_owed(
    cli: Cli, tree: Path, put_package: PutPackage
) -> None:
    # `version_source.kind: "none"` — what every put_package manifest declares. Unknowable
    # is not the same as stale: counting it as owed would put every capture-sourced package
    # permanently in the run set, filtering nothing.
    put_package("alpha", "unattended")
    put_record(tree, "alpha", "1.0.0")

    result = cli("run", "--stale")
    assert result.returncode == 0
    assert "ran alpha" not in result.output
    assert "fresh" in result.output
    assert "1 fresh" in result.output


def test_stale_treats_an_unreadable_newest_record_as_never_having_recorded(
    cli: Cli, tree: Path, put_package: PutPackage
) -> None:
    put_package("alpha", "unattended")
    results = tree / "alpha" / "results"
    results.mkdir(parents=True)
    (results / "2026-01-01T000000-opencode-1.0.0.json").write_text("not json\n")

    result = cli("run", "--stale")
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "1 ran" in result.output


def test_stale_skips_a_fresh_manual_package_instead_of_pointing_at_its_readme(
    cli: Cli, tree: Path, put_package: PutPackage
) -> None:
    # The manual skip line exists to tell you to go drive the probe by hand. For a package
    # whose answer cannot have changed, that is a false errand.
    put_package("alpha", "manual")
    put_version_stub(tree, "alpha", "1.0.0")
    put_record(tree, "alpha", "1.0.0")

    result = cli("run", "--stale")
    assert result.returncode == 0
    assert "authorize with --manual" not in result.output
    assert "1 fresh" in result.output
    assert "0 need --manual" in result.output


def test_stale_still_names_a_drifted_manual_package_as_needing_a_hand_run(
    cli: Cli, tree: Path, put_package: PutPackage
) -> None:
    put_package("alpha", "manual")
    put_version_stub(tree, "alpha", "2.0.0")
    put_record(tree, "alpha", "1.0.0")

    result = cli("run", "--stale")
    assert result.returncode == 0
    assert "authorize with --manual" in result.output
    assert "1 need --manual" in result.output
    assert "0 fresh" in result.output


def test_stale_does_not_on_its_own_authorize_spending_on_a_drifted_billed_package(
    cli: Cli, tree: Path, put_package: PutPackage
) -> None:
    # Selection and authorization are independent: being owed a run is not permission to
    # pay for one.
    put_package("alpha", "billed")
    put_version_stub(tree, "alpha", "2.0.0")
    put_record(tree, "alpha", "1.0.0")

    result = cli("run", "--stale")
    assert result.returncode == 0
    assert "ran alpha" not in result.output
    assert "authorize with --billed" in result.output
    assert "1 need --billed" in result.output


def test_stale_composes_with_billed_to_reach_a_drifted_billed_package(
    cli: Cli, tree: Path, put_package: PutPackage
) -> None:
    put_package("alpha", "billed")
    put_version_stub(tree, "alpha", "2.0.0")
    put_record(tree, "alpha", "1.0.0")
    put_package("beta", "billed")
    put_version_stub(tree, "beta", "1.0.0")
    put_record(tree, "beta", "1.0.0")

    result = cli("run", "--stale", "--billed")
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "ran beta" not in result.output
    assert "1 ran" in result.output
    assert "1 fresh" in result.output


def test_a_fresh_package_is_passed_over_before_its_cost_is_ever_considered(
    cli: Cli, tree: Path, put_package: PutPackage
) -> None:
    # Selection runs first, so a fresh billed package is reported as fresh rather than as
    # needing authorization it would never be asked to spend.
    put_package("alpha", "billed")
    put_version_stub(tree, "alpha", "1.0.0")
    put_record(tree, "alpha", "1.0.0")

    result = cli("run", "--stale")
    assert result.returncode == 0
    assert "fresh" in result.output
    assert "1 fresh" in result.output
    assert "0 need --billed" in result.output


def test_a_default_run_reports_no_fresh_bucket_at_all(
    cli: Cli, tree: Path, put_package: PutPackage
) -> None:
    # Not `0 fresh`: on a default run freshness is never evaluated, and a zero would read as
    # having been evaluated and found nothing.
    put_package("alpha", "unattended")
    put_version_stub(tree, "alpha", "1.0.0")
    put_record(tree, "alpha", "1.0.0")

    result = cli("run")
    assert result.returncode == 0
    assert "ran alpha" in result.output
    assert "fresh" not in result.output


def test_a_package_with_no_manifest_is_passed_over_silently(
    cli: Cli, tree: Path, put_package: PutPackage
) -> None:
    # Defense-in-depth, not a sanctioned state: the contract says every probe package has a
    # manifest. This pins that a half-authored one cannot break a batch run.
    (tree / "fixtures-only").mkdir()
    (tree / "fixtures-only" / "README.md").write_text(
        "A directory mid-authoring: a README, no manifest yet.\n"
    )
    put_package("alpha", "unattended")

    result = cli("run")
    assert result.returncode == 0
    assert "fixtures-only" not in result.output
    assert "1 ran" in result.output
    assert "0 skipped" in result.output


def test_a_failing_probe_does_not_abort_the_loop(cli: Cli, put_package: PutPackage) -> None:
    # The partial-failure contract: records written by probes that passed stay valid, and
    # the run names what broke. The phase success criterion expects success, so it never
    # exercises this.
    put_package("alpha", "unattended", 1)
    put_package("beta", "unattended", 0)

    result = cli("run")
    assert result.returncode != 0
    assert "ran alpha" in result.output
    assert "ran beta" in result.output
    assert "alpha FAILED" in result.output
    assert "failed: alpha" in result.output
    assert "1 ran" in result.output


def test_an_unattended_package_with_no_executable_runner_is_reported_as_failed(
    cli: Cli, put_package: PutPackage
) -> None:
    # A package's runner is its `probe.py`; a manifest without one has nothing to run.
    put_package("alpha", "unattended").joinpath("probe.py").unlink()

    result = cli("run")
    assert result.returncode != 0
    assert "alpha FAILED" in result.output
    assert "failed: alpha" in result.output


def test_an_unknown_driver_is_reported_as_failed_rather_than_silently_skipped(
    cli: Cli, put_package: PutPackage
) -> None:
    # `script` is a retired value, so it exercises the arm a stale manifest would land in
    # rather than a value nobody ever wrote.
    put_package("alpha", "script")

    result = cli("run")
    assert result.returncode != 0
    assert "alpha: manifest driver must be unattended, billed, or manual" in result.stderr
    assert "failed: alpha" in result.stderr
    assert "ran alpha" not in result.output


def test_an_empty_tree_exits_0(cli: Cli, tree: Path) -> None:
    result = cli("run")
    assert result.returncode == 0
    assert "0 ran" in result.output
