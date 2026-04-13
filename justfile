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
    cargo fix --allow-dirty
    cargo +nightly fmt
    cargo sort-derives

# Run clippy on all targets
lint:
    cargo clippy --all-targets

# Check dependency licenses
licenses:
    cargo deny check licenses

# Format + lint + test + licenses
check: fmt lint build test licenses

# Install binary locally
install:
    cargo install --path .
