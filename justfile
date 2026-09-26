# virt-linux — manage Linux VMs on Linux hosts via QEMU/KVM
#
# This is the SOURCE OF TRUTH for all build/test/lint operations.
# Forgejo Actions calls these recipes directly — no duplication!

# Default recipe: show available commands
default:
    @just --list

# Format all code
fmt:
    @echo "Formatting code..."
    cargo fmt --all

# Check formatting without modifying files
fmt-check:
    @echo "Checking code formatting..."
    cargo fmt --all -- --check

# Lint (warnings are errors)
lint:
    @echo "Running clippy..."
    cargo clippy --locked --workspace --all-targets -- -D warnings

# Audit dependency licenses against deny.toml (requires cargo-deny)
license-audit:
    @echo "Auditing dependency licenses..."
    cargo deny check licenses
    @echo "License audit passed!"

# Run unit tests
test:
    @echo "Running tests..."
    cargo test --locked --workspace --all-targets

# Release build
build:
    @echo "Building release binary..."
    cargo build --locked --release
    @echo "Built: target/release/virt-linux"

# Install the release binary into ~/.cargo/bin
install:
    cargo install --path . --locked --force

# Clean build artifacts
clean:
    cargo clean

# Quick code stats: LOC, largest files, module tree.
# Requires `scc` (cargo install scc) and `cargo-modules`
# (cargo install cargo-modules) — both dev-only, not in Cargo.toml.
# The cargo-modules call falls back to a hint if the tool is missing.
stats:
    @echo "=== Workspace LOC ==="
    @scc src --no-cocomo
    @echo ""
    @echo "=== Largest Rust source files (top 15) ==="
    @scc src --by-file --no-cocomo -s lines -i rs | head -20
    @echo ""
    @echo "=== Module tree ==="
    @cargo modules structure --lib 2>/dev/null \
      || echo "(install cargo-modules for the module tree: cargo install cargo-modules)"

# Run all CI checks (same as the CI workflow)
# This is what developers should run before pushing
ci: fmt-check lint license-audit test build
    @echo ""
    @echo "Safe to push - CI will pass."
