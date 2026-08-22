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

# Run clippy on all targets
lint:
    cargo clippy --all-targets

# Check dependency licenses
licenses:
    cargo deny check licenses

# Lint the probe shell (scoped to experiments/; see experiments/README.md)
shellcheck:
    find experiments -name '*.sh' -exec shellcheck {} +

# Run the probe harness test suite
bats-test:
    bats experiments/lib/tests

# Run the probes this invocation authorizes. Free ones always; --billed and
# --manual each add their driver, --all adds every one, and the flags stack.
# --stale narrows to the probes owed a run: never recorded, or version drift.
probe-run *args:
    experiments/lib/probe-run.sh {{args}}

# Report on committed probe records; invokes no probe
probe-status:
    experiments/lib/probe-status.sh

# One line of probe status; can never fail the build
probe-summary:
    -@experiments/lib/probe-status.sh --summary

# lint, cargo check, shell gates, test, licenses, format, then the probe summary
check: lint && shellcheck bats-test test licenses fmt probe-summary
  cargo check

# Install binary locally
install:
    cargo install --path .
