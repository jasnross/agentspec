# List available recipes
default:
    @just --list

# Build the project
build:
    cargo build

# Run graphify
graphify:
  graphify update .

# Automatically run graphify during development
graphify-watch:
  @watchexec -r -e rs 'cargo check && just graphify'

# Run tests
test:
    cargo test

# Format source files
fmt:
    prettier -w .
    cargo fix --allow-dirty
    cargo +nightly fmt
    cargo sort-derives
    uv run --locked --directory experiments ruff format .

# Run clippy on all targets
lint:
    cargo clippy --all-targets -- -D warnings

# Check dependency licenses
licenses:
    cargo deny check licenses

# Lint the scripts a provider runs (scoped to probe fixtures; see experiments/README.md)
shellcheck:
    find experiments -path 'experiments/.*' -prune -o -path '*/fixtures/*' -name '*.sh' -print0 | xargs -0 shellcheck

# Run the probe harness test suite
probe-test:
    uv run --locked --directory experiments pytest -q

# Lint, format-check, and type-check the probe harness; each probe.py is its own mypy run
probe-lint:
    #!/usr/bin/env bash
    set -euo pipefail
    cd experiments
    uv run --locked ruff check .
    uv run --locked ruff format --check .
    uv run --locked mypy
    shopt -s nullglob
    for definition in */probe.py; do uv run --locked mypy "$definition"; done

# Run the probes this invocation authorizes. Free ones always; --billed and
# --manual each add their driver, --all adds every one, and the flags stack.
# --stale narrows to the probes owed a run: never recorded, or version drift.
probe-run *args:
    experiments/probe run {{args}}

# Report on committed probe records; invokes no probe
probe-status:
    experiments/probe status

# One line of probe status; can never fail the build
probe-summary:
    -@experiments/probe status --summary

# lint, cargo check, probe-harness gates, test, licenses, format, then the probe summary
check: lint && shellcheck probe-lint probe-test test licenses fmt probe-summary
  cargo check

# Install binary locally
install:
    cargo install --path .
