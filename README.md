# logring

logring is a power-cut-safe log store for embedded devices, written in Rust (`no_std`, no `alloc`, embassy-friendly).
Firmware writes many tracks of any [shrink_wrap](https://github.com/vhrdtech/wire_weaver) type at once into a ring,
with sessions, recordings and markers.

A host reads the ring back in three ways: over `wire_weaver`, as a USB disk, or with the `logring` tool on a personal
computer (PC).

Its main medium is a Secure Digital (SD) card. The card holds a small normal exFAT region (128 MB by default,
configurable) for files that a host reads and writes as usual.

The rest of the card is a preallocated ring file (`logring.wlr`).

Over USB the device shows up as a normal disk. The read-write files work as usual, and the log appears as read-only
files split by recording and by time.

The log exports to MF4 (Measurement Data Format version 4), MCAP (a container format for robotics logs) and CSV. The
device formats the card itself: put in a new card, say yes, done.

| crate | what |
|---|---|
| [`crates/logring`](crates/logring) | the engine: format, writer, recovery, reader, `TrackStore`, the view model, media |
| [`crates/logring_export`](crates/logring_export) | PC-side reader and exporter (CSV, MCAP, MF4) |
| [`crates/logring_cli`](crates/logring_cli) | the `logring` command line tool, its commands as a library |
| [`crates/logring_ww_cli`](crates/logring_ww_cli) | the same commands as `ww logring`, inside the `ww` binary |
| [`crates/logring_py`](crates/logring_py) | the `logring` Python API (PyO3), also `wire_weaver.logring` in the `wire_weaver` wheel |
| [`crates/logring_wheel`](crates/logring_wheel) | the `logring` Python wheel over it (maturin) |

```sh
cargo install --path crates/logring_cli     # (or, from a checkout: `just install`)
logring info card.img         # sessions, recordings, tracks, counters
logring export card.img --format csv --out ./out
```

```python
import logring

card = logring.open("card.img")
df = card.track("can1_rx").to_pandas()
```

The medium is a trait, `LogMedium`, from the `ww_stream` crate of WireWeaver. Other storage than SD cards can
therefore follow, such as eMMC or flash chips on a Serial Peripheral Interface (SPI) or quad SPI (QSPI) bus.

logring serves more than one product. rePort is its first user, and logring aims to be the base for every logging
device after it.

[AGENTS.md](AGENTS.md) describes the layout of the repo. [FEATURES.md](FEATURES.md) lists what is implemented,
planned and still an idea, and [CHANGELOG.md](CHANGELOG.md) holds the history.

Licensed under `MIT OR Apache-2.0`.
