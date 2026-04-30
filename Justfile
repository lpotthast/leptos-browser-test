# Lists all available commands.
list:
  just --list

# Install dependencies for maintenance work, profiling and more...
install-tools:
  cargo +stable install --locked cargo-hack
  cargo +stable install --locked cargo-minimal-versions
  cargo +stable install --locked cargo-msrv
  cargo +stable install --locked cargo-expand
  cargo +stable install --locked cargo-whatfeatures
  cargo +stable install --locked cargo-upgrades
  cargo +stable install --locked cargo-edit

# Find the minimum supported rust version
msrv:
    cargo msrv find

# Check if the current dependency version bounds are sufficient.
minimal-versions:
    cargo minimal-versions check --workspace --direct

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --all-targets --all-features -- -D warnings

test:
    cargo test

build:
    cargo build

doc:
    cargo doc

bench:
    cargo bench

bench-smoke:
    cargo bench --no-run

bench-chunks:
    cargo bench --bench chunk_delivery

bench-lines:
    cargo bench --bench line_delivery

# Run the full validation suite: check, clippy, test, build, doc
verify: fmt-check lint test build doc
