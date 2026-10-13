"""Preflight: every check that can fail before a workspace exists or an operator acts.

A manual probe's capture costs a live session, so nothing that could be known before the
session may first surface at record time. The version guard and the imports run in
`experiments/probe`; this module runs the rest, in order:

    1. the manifest gates
    2. the family's tools, then a jq compile check of every manifest expression
    3. `PROBE_DRY_RUN`, `PROBE_FIXTURE` and `PROBE_TIMEOUT_SECONDS`

The definition's own preflight function and any family-specific check run next, in the
family runner, still before a workspace exists.
"""

import os
from dataclasses import dataclass
from pathlib import Path

from probe_harness import jq_cli
from probe_harness.contract import (
    CaptureSource,
    Manifest,
    ManifestError,
    ProjectionAssertion,
    parse_manifest,
)
from probe_harness.workspace import DEFAULT_TIMEOUT_SECONDS, require_tools

ASSERTION_VARIANT = "assertion"


class PreflightError(Exception):
    """A preflight refusal: <lines> go to stderr and the run exits with <code>."""

    def __init__(self, code: int, *lines: str) -> None:
        super().__init__("\n".join(lines))
        self.code = code
        self.lines = lines


@dataclass(frozen=True)
class RunOptions:
    dry_run: bool
    variant: str
    timeout: int


def check_manifest(package: Path) -> Manifest:
    """Step 1 and the jq half of step 2, shared with `probe replay`."""
    manifest = _parse(package)
    check_expressions(manifest)
    return manifest


def check_expressions(manifest: Manifest) -> None:
    expressions: list[tuple[str, str]] = []
    if isinstance(manifest.assertion, ProjectionAssertion):
        expressions.append(("projection", manifest.assertion.projection))
    if manifest.wait_for is not None:
        expressions.append(("wait_for", manifest.wait_for))
    if isinstance(manifest.version_source, CaptureSource):
        expressions.append(("version_source.jq", manifest.version_source.jq))
    for name, expr in expressions:
        error = jq_cli.compile_error(expr)
        if error is not None:
            raise PreflightError(
                1, f"probe: the manifest's {name} does not compile: {expr}", error.rstrip("\n")
            )


def run(
    package: Path, variants: tuple[str, ...], family_tools: tuple[str, ...]
) -> tuple[Manifest, RunOptions]:
    """Run the preflight sequence, raising `PreflightError` at the first refusal."""
    manifest = _parse(package)
    try:
        require_tools(*family_tools)
    except SystemExit as e:
        raise PreflightError(int(e.code or 1)) from None
    check_expressions(manifest)
    return manifest, run_options(variants)


def run_options(variants: tuple[str, ...]) -> RunOptions:
    dry_run = os.environ.get("PROBE_DRY_RUN", "")
    if dry_run not in ("", "0", "1"):
        raise PreflightError(2, f"probe: PROBE_DRY_RUN must be 0 or 1 (got: {dry_run})")

    variant = os.environ.get("PROBE_FIXTURE") or ASSERTION_VARIANT
    if variant not in variants:
        raise PreflightError(
            2, f"probe: PROBE_FIXTURE must be one of: {', '.join(variants)} (got: {variant})"
        )
    if variant != ASSERTION_VARIANT and dry_run != "1":
        # A fixture built to disagree would commit a false `refuted` if it recorded.
        raise PreflightError(
            2,
            f"probe: the {variant} fixture is dry-run only; set PROBE_DRY_RUN=1",
            "probe: recording its run would commit a refuted record for a fixture built "
            "to disagree.",
        )

    # Empty is unset, as the shell's `${PROBE_TIMEOUT_SECONDS:-…}` read it.
    timeout_text = os.environ.get("PROBE_TIMEOUT_SECONDS", "")
    timeout = DEFAULT_TIMEOUT_SECONDS
    if timeout_text:
        if not (timeout_text.isascii() and timeout_text.isdecimal() and int(timeout_text) > 0):
            raise PreflightError(
                2, f"probe: PROBE_TIMEOUT_SECONDS must be a positive integer (got: {timeout_text})"
            )
        timeout = int(timeout_text)

    return RunOptions(dry_run=dry_run == "1", variant=variant, timeout=timeout)


def _parse(package: Path) -> Manifest:
    try:
        return parse_manifest(package / "probe.json")
    except ManifestError as e:
        raise PreflightError(1, f"record: {e}") from None
