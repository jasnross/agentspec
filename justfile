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
    prettier -w ./**/*.md
    cargo fix --allow-dirty
    cargo +nightly fmt
    cargo sort-derives

# Run clippy on all targets
lint:
    cargo clippy --all-targets

# Check dependency licenses
licenses:
    cargo deny check licenses

# Format + lint + cargo check + test + licenses
check: fmt lint && test licenses
  cargo check

# Install binary locally
install:
    cargo install --path .
