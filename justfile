# logring: ww_logring (no_std, no alloc engine), ww_logring_export/cli/py (PC).

py_venv := "/tmp/ww_logring_py-venv"

default:
    @just --list

# Tests (host, no card or image needed), Rust and Python, plus a no_std build check for the engine
test: test-py
    cargo test --workspace --quiet
    cargo check -p ww_logring --no-default-features --target thumbv8m.main-none-eabihf

# Build the Python module (crates/ww_logring_py) into a venv under /tmp and run its pytest smoke test
[working-directory('crates/ww_logring_py')]
test-py:
    UV_PROJECT_ENVIRONMENT={{py_venv}} uv run --quiet --group dev pytest -q

# Format check, cargo-sort, typos and clippy without warnings, all crates
lint:
    cargo fmt --all --check
    cargo sort --workspace --grouped --check
    typos
    cargo clippy --workspace --all-targets -- -D warnings

# Build and install logring on this PC; completion: source <(COMPLETE=bash logring)
install:
    cargo install --quiet --path crates/ww_logring_cli --locked

# Bring this PC up to main
deploy:
    tpm repos pull logring
    just install
