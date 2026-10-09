msrv_toolchain := "1.89.0"

# Lists all available commands.
list:
    just --list

# Install tools required by other recipes.
install-tools:
    rustup toolchain add {{ msrv_toolchain }}
    rustup target add wasm32-unknown-unknown
    cargo +stable install --locked cargo-hack
    cargo +stable install --locked cargo-minimal-versions
    cargo +stable install --locked cargo-msrv
    cargo +stable install --locked cargo-sort
    cargo +stable install --locked cargo-expand
    cargo +stable install --locked cargo-whatfeatures
    cargo +stable install --locked cargo-upgrades
    cargo +stable install --locked cargo-edit
    cargo +stable install --locked wasm-bindgen-cli
    cargo +stable install --locked cargo-leptos

# Find the minimum supported rust version.
msrv:
    cargo msrv find

# Check that the crate still builds on the declared minimum supported rust version.
check-msrv:
    cargo +{{ msrv_toolchain }} check --all-targets --locked

# Check if the current dependency version bounds are sufficient.
minimal-versions:
    cargo minimal-versions check --workspace --direct

# Format all code.
fmt:
    cargo fmt --all

# Check formatting without modifying files.
fmt-check:
    cargo fmt --all -- --check

# Check the code.
check:
    cargo check --all-targets --locked

# Lint the code.
lint:
    cargo clippy --all-targets --all-features --locked -- -D warnings

# Run the in-module unit tests.
test-unit:
    cargo test --lib --locked -- --nocapture

# Compile and run the README / rustdoc examples.
test-doc:
    cargo test --doc --locked

# Run the end-to-end smoke tests against the SSR fixture (requires cargo-leptos and the wasm target).
# Run sequentially so concurrent fixture builds don't race over the same `target/`.
test-smoke:
    cargo test --test smoke --locked -- --test-threads=1 --nocapture

# Run all tests.
test: test-unit test-doc test-smoke

# Build the crate.
build:
    cargo build --locked

# Build the crate documentation with warnings treated as errors.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked

# Update dependencies, sort manifests, and format code.
tidy:
    cargo update --workspace
    cargo sort --workspace
    cargo fmt --all

# Run the full non-mutating validation suite.
verify: fmt-check check lint test build doc check-msrv
