# logring: logring (no_std, no alloc engine), logring_export/cli/py (PC).

py_venv := "/tmp/logring_py-venv"
# The Zensical release the book is built with, so a new release cannot break the build unseen
zensical := "zensical==0.1.2"
# The prose lints book-check runs: at deny in spirit_level.toml
prose_lints := "--only prose-run-words --only page-reading-time --only heading-words --only code-wall --only code-block-language --only passive-voice --only reading-ease --only orphan-pages --only page-title --only abbreviation-first-use --only term-before-definition --only paragraph-sentences --only sentence-words --only emoji --only page-links --only nav-entries"

default:
    @just --list

# The tests. `just test`: all of them (test-all), the gate run before landing changes. `just test --changed`: only
# the workspace crates changed against main and the crates that depend on them, what to run locally before
# calling a change ready.
test *flags:
    {{ if flags == "--changed" { "changedtests run" } else { just_executable() + " test-all " + flags } }}

# Tests (host, no card or image needed), Rust and Python, plus a no_std build check for the engine
test-all: test-py
    cargo test --workspace --quiet
    cargo test -p logring --features std,block-device-driver --quiet
    # without tail-positions (S3): the small build's mount, its power-cut, round-trip and cross-build tests
    cargo test -p logring --no-default-features --features std,block-device-driver,card-check,crc-hw --quiet
    cargo test -p logring --no-default-features --features std --quiet
    cargo check -p logring --no-default-features --target thumbv8m.main-none-eabihf
    cargo check -p logring --no-default-features --features block-device-driver --target thumbv8m.main-none-eabihf
    # the book's drive example as firmware for the RP2350's core
    cargo build --quiet -p logring_example --target thumbv8m.main-none-eabihf
    cargo build --quiet --manifest-path size/Cargo.toml --release --target thumbv8m.main-none-eabihf

# Build the Python module (crates/logring_wheel over logring_py) into a venv under /tmp and run its pytest smoke test
[working-directory('crates/logring_wheel')]
test-py:
    UV_PROJECT_ENVIRONMENT={{py_venv}} uv run --quiet --group dev pytest -q

# Format check, cargo-sort, typos and clippy without warnings, all crates
lint:
    cargo fmt --all --check
    cargo sort --workspace --grouped --check
    typos
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy -p logring --all-targets --features std,block-device-driver -- -D warnings
    cargo clippy -p logring --all-targets --no-default-features --features std,block-device-driver,card-check,crc-table -- -D warnings
    cargo clippy -p logring --all-targets --no-default-features --features std,card-check,crc-table -- -D warnings
    cargo check -p logring --no-default-features --target thumbv8m.main-none-eabihf
    cargo clippy -p logring --no-default-features --features block-device-driver --target thumbv8m.main-none-eabihf -- -D warnings
    cargo clippy -p logring --no-default-features --features block-device-driver,card-check,crc-hw --target thumbv8m.main-none-eabihf -- -D warnings
    cargo clippy -p logring_example --target thumbv8m.main-none-eabihf -- -D warnings
    cargo fmt --manifest-path size/Cargo.toml --check
    cargo fmt --manifest-path size/report/Cargo.toml --check
    cargo sort --grouped --check size size/report
    cargo clippy --manifest-path size/Cargo.toml --release --target thumbv8m.main-none-eabihf -- -D warnings
    cargo clippy --manifest-path size/report/Cargo.toml -- -D warnings

# Flash and RAM of the device side per MCU target, crate, file, function and type, as Markdown (size/README.md)
size *args="all":
    cargo run --quiet --release --manifest-path size/report/Cargo.toml -- {{args}}

# Build the book (docs/, zensical.toml) into site/ with Zensical
[group('book')]
book:
    uv run --with {{ zensical }} zensical build --clean

# Serve the book at http://localhost:8000 and rebuild it on every change
[group('book')]
book-serve:
    uv run --with {{ zensical }} zensical serve

# The book's checks: the strict build, then the prose lints, so a broken link or a long page cannot land
[group('book')]
book-check: book-strict book-prose

# Build the book with warnings as errors: a link to a missing page, anchor or figure fails; links to other sites are not fetched
[group('book')]
book-strict:
    uv run --with {{ zensical }} zensical build --clean --strict

# Fuzz the mount on garbage and damaged rings (nightly, cargo install cargo-fuzz); seeds: fuzz/seeds
[working-directory('fuzz')]
fuzz:
    cargo +nightly fuzz run mount corpus/mount seeds

# Fuzz the PC tool's dynamic decode on an untrusted bundle (nightly, cargo install cargo-fuzz)
[working-directory('fuzz')]
fuzz-bundle:
    cargo +nightly fuzz run bundle_decode corpus/bundle_decode

# With tpm on PATH: installs through tpm's kept-releases stack (one copy on PATH); otherwise: cargo install
# Build and install logring on this PC; completion: source <(COMPLETE=bash logring)
install:
    cargo build --profile deploy --locked -p logring_cli
    if command -v tpm >/dev/null; then tpm install target/deploy/logring; else cargo install --locked --path crates/logring_cli; fi

# Move every git dependency to its repository's main in Cargo.lock (commit the lock)
deps-pull:
    cargo update -p wire_weaver -p ww_si -p shrink_wrap -p ww_tape -p exfat_lite

# Bring this PC up to main: pull, install, check --version against main
deploy:
    if command -v tpm >/dev/null; then tpm repos pull logring; else git pull --ff-only; fi
    just install
    logring --version | grep -q "($(git rev-parse --short HEAD)" || { echo "logring --version does not show $(git rev-parse --short HEAD)"; logring --version; exit 1; }
    logring --version

# The book's prose lints of spirit_level.toml (sentence and paragraph length, reading time, terms). Without spl it fails
# in CI and is skipped elsewhere
[group('book')]
book-prose:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v spl >/dev/null 2>&1; then
        if [ -n "${CI:-}" ]; then
            echo "error: spl is missing in CI; install it as ci.yml does"
            exit 1
        fi
        echo "skipped: spl check of the book's prose, spl missing (install spirit_level's spl)"
        exit 0
    fi
    spl check --progress never {{ prose_lints }}
