# Working on logring

Guidance for AI agents and contributors. Read this before changing code.

## What this is

`logring` holds the power-cut-safe ring log store for embedded devices: many tracks of any
[shrink_wrap](https://github.com/vhrdtech/wire_weaver) type written at once into one preallocated ring file (default
extension `.wlr`), with sessions, recordings and markers, read back over `wire_weaver`, as a USB disk, or with the
`logring` PC tool. First user: rePort (P2424); not tied to one product or one medium (SD today, SPI/QSPI flash and
eMMC behind the same `LogMedium` trait later).

This is one of three repos split out of the tpm project P2628 (design: `prj/P2628-logring/design/plan.md` in the
tpm folder on Syncthing, not in this repo — design notes stay out of code repos):

- **`logring`** (this repo): the engine and the PC tools.
- [`exfat_lite`](https://github.com/vhrdtech/exfat_lite): no-alloc exFAT for SD cards, knows nothing of logring.
- [`usb_msc_device`](https://github.com/vhrdtech/usb_msc_device): a USB Mass Storage device class for
  embassy-usb, knows nothing of logring.

`wire_weaver` keeps `ww_stream`'s `StreamStore`/`LogStore`/`LogMedium` as they are (SRV-15, done and tested) and
the `ww_recorder` API crate (`ww_stdlib`, STD-34): this repo depends on both, not the other way round.

## FEATURES.md is the source of truth

[FEATURES.md](FEATURES.md) lists every feature with its status, every known bug, and what is planned, with stable
IDs per area (`LOG` the engine and format, `VIEW` the USB view, `PC` export/CLI/Python, `TEST` hardware runs).

- **Read the relevant area before starting.** A "new" idea is often already recorded there.
- **Name IDs with a short slug when talking to the user**: `LOG-1 format-writer`, never a bare `LOG-1`. Commit
  messages, CHANGELOG and code `TODO`s keep the bare ID.
- **New IDs**: `tpm work claim AREA` prints the next free one. Never renumber or reuse IDs.
- Status ✅ means verified on real hardware (a card, rePort); 🧪 recorded fixtures/images and unit tests only;
  📋 planned with a P2628 task; 💡 an idea, not filed as a task yet. Keep that honest.

## CHANGELOG.md records every change

[CHANGELOG.md](CHANGELOG.md) is the history, FEATURES.md the current state; keep both.

- Every change a user would notice gets an entry under `## [Unreleased]` in the same commit: `### Added`,
  `### Changed`, `### Fixed`, `### Removed`. Short and user-facing, with the feature ID in parentheses.
- Pure refactors and typo fixes don't need an entry.

## Layout

Cargo workspace, edition 2024, one version for all crates (`[workspace.package]`), the same layout as the other
instrument/tool repos (tpm CLAUDE.md "Code repos": tools are a library first):

- `crates/ww_logring` — the engine: `no_std`, no `alloc`. Format (superblock, meta area, segments, blocks,
  records), writer (stage buffers, `flush_bytes`/`flush_interval`, `sync`), recovery, reader/index, `TrackStore`
  (implements `wire_weaver`'s `ww_stream::StreamStore` so a reliable stream backfills from the ring), sessions,
  recordings, markers, the view model (`view` module: snapshot tree by recording/time/track, native split files,
  fixed-width CSV), `ExfatRing` (an `exfat_lite`-backed `LogMedium` window, `exfat` feature), a
  `block-device-driver` medium, a simulated card for `std` tests. The `std` feature is for host-side use
  (`ww_logring_export`, the CLI, tests): no RAM/flash limits assumed.
- `crates/ww_logring_export` — PC: opens a mounted card, an image, a ring file or a split `.wlr` (through
  `ww_logring`'s `std` feature, no raw device access, no root); decodes tracks dynamically from the stored API
  bundle (plus `wire_weaver_snapshots` for stdlib types); exports CSV, MCAP (JSON + jsonschema from the `ww_self`
  type), MF4 (numeric channels, CAN as ASAM bus logging). DBC is never stored; it applies only at export.
- `crates/ww_logring_cli`, binary **`logring`** — `info`, `ls` (the same tree as the USB view, via
  `ww_logring::view`), `export`, `cat`, `check` (like fsck), `make-image`; `--version` with git SHA and commit
  time (`build.rs`, no extra crates), shell completions, colour, progress bars (tpm CLAUDE.md "CLI quality").
- `crates/ww_logring_py` (PyO3 + maturin, package `ww_logring`) — `open()`, tracks, recordings, markers,
  `track.to_pandas()`, built on `ww_logring_export`.

[docs/format.md](docs/format.md) documents the on-medium format as it is (LOG-1); `fuzz/` holds the cargo-fuzz
targets (`just fuzz`, nightly) with seed images; the `prj/P2628-logring` tpm folder has the design history and
open questions.

## Commands

```sh
just test      # workspace tests (host; no card or image needed) + a no_std build check for ww_logring + Python smoke test
just test-py   # builds crates/ww_logring_py into a venv under /tmp (uv + maturin), runs pytest
just lint      # fmt --check, clippy -D warnings, all crates
just fuzz      # cargo-fuzz (nightly) on the mount target with the seed images in fuzz/seeds
just install   # puts `logring` in ~/.cargo/bin; completion: source <(COMPLETE=bash logring)
just deploy    # bring this PC up to main
```

Before declaring a change done: `just lint` and `just test`. Nothing here needs a real SD card or rePort to
build and unit-test (fixtures and a simulated card); hardware runs (R2 SD spike, R9 OS/MSC behaviour, R12
power-cut cycles) need rePort, a probe and real cards — a piece of hardware next to a PC, not something a
session can touch on its own. Ask the user or the parent session (`tpm work say parent`) when a change needs
checking against it, then carry on with what can be tested without it. Say in the report which commands were
run against real hardware versus only against fixtures/images.

## Dependencies

Pure Rust only. `wire_weaver` crates (`ww_stream`, `shrink_wrap`, `ww_self`, `wire_weaver_snapshots`) aren't on
crates.io yet (wire_weaver 0.5 unreleased): depend on them by path, `../wire_weaver/...`, so logring always builds
against wire_weaver's master checked out next to it (`just deps-pull` updates it; CI checks it out beside this repo);
move to crates.io versions once wire_weaver 0.5 ships (crates.io takes no path or git deps, so logring publishes after
it). Same for `exfat_lite` and `usb_msc_device`
until they are published. Check new dependencies with `cargo tree` for `-sys` crates and `cc`/`bindgen` build
deps (tpm CLAUDE.md "Pure-Rust crates, no C bindings").

## Code conventions

- Errors: typed enums with `thiserror` in the library crates (never panic on untrusted input: a card or a host
  SCSI command is untrusted data); `anyhow` only in the CLI.
- The CLI and the Python module stay thin: new behaviour goes into `ww_logring`/`ww_logring_export` first.
- `no_std`, no `alloc` in `ww_logring`'s default build; anything needing an allocation is behind `std`.

## Tests

- Pure logic (format encode/decode, records, track tables): unit tests next to the code, `no_std`-compatible
  where the code they test is.
- Power-cut recovery: simulated power cuts at every write unit (as `wire_weaver`'s `ww_stream` does), on SD and
  NOR, asserting nothing written is lost and torn blocks are counted (`src/tests/writer.rs`); damaged images
  through the fuzz harness (`src/harness.rs`, `src/tests/corrupt.rs`); the cargo-fuzz target `fuzz/mount` with
  seed images (`fuzz/seeds`, regenerated by the ignored test `write_fuzz_seeds`).
- The Python module: `crates/ww_logring_py/tests/test_smoke.py` (pytest, offline).
- A bug fix starts with a failing test when the bug can be reproduced in one.

## Commits

Conventional Commits with a scope: `feat(log): ...`, `fix(view): ...`, scopes `log`, `view`, `pc`, `cli`, `py`,
`build`. Short imperative summary, blank line, body with what and why; reference feature IDs.

Commit on your own initiative on a session branch (`tpm work new SLUG logring`), each finished step one commit
with FEATURES.md and CHANGELOG.md updated in it. When the work is done and the user agrees, `tpm land` puts it
on main.

## Versions

Landing bumps the version, not each work commit (tpm CLAUDE.md "Landing bumps the version"): session commits add
CHANGELOG entries under `[Unreleased]` without bumping. This repo's version is its next crates.io release
(`bump = "at-publish"` in tpm's repos.toml): `tpm land` never bumps or tags it on its own; `tpm release logring`
does, once there is something to publish. `logring --version` prints version, git SHA and commit time.
