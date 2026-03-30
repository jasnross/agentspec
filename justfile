# List available recipes
default:
    @just --list

# Build the project
build:
    cargo build

# Run tests
test:
    cargo test

# Format source files
fmt:
    cargo fix
    cargo +nightly fmt

# Check formatting without writing (CI)
fmt-check:
    cargo +nightly fmt --check

# Run clippy on all targets
lint:
    cargo clippy --all-targets

# Format + lint (fmt writes, then lint checks)
check:
    cargo build
    cargo +nightly fmt
    cargo clippy --all-targets

# CI check: fmt + lint without writing
check-ci:
    cargo +nightly fmt --check
    cargo clippy --all-targets

# Reinstall binary (required after schema changes)
install:
    cargo install --path .
