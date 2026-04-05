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
    cargo sort-derives

# Run clippy on all targets
lint:
    cargo clippy --all-targets

# Format + lint + test
check: fmt lint build test

# Reinstall binary (required after schema changes)
install:
    cargo install --path .
