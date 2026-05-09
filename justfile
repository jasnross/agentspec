# List available recipes
default:
    @just --list

# Build the project
build:
    cargo build

<<<<<<< Updated upstream
=======
# Run graphify
graphify:
  graphify update .

# Automatically run graphify during development
graphify-watch:
  @watchexec -r -e rs 'cargo check && just graphify'

>>>>>>> Stashed changes
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

<<<<<<< Updated upstream
# Format + lint + test + licenses
check: fmt lint build test licenses
=======
# Format + lint + cargo check + test + licenses
check: fmt lint && test licenses
  cargo check
>>>>>>> Stashed changes

# Install binary locally
install:
    cargo install --path .
