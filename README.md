# logring

A power-cut-safe log store for embedded devices in Rust (`no_std`, no `alloc`, embassy-friendly): many tracks of
any [shrink_wrap](https://github.com/vhrdtech/wire_weaver) type written at once into a ring, with sessions,
recordings and markers, read back over `wire_weaver`, as a USB disk, or with the `logring` PC tool.

Its main medium is an SD card: a small normal exFAT region (128 MB by default, configurable) for files that can
be read and written as usual, and the rest is a preallocated ring file (`logring.wlr`). Over USB the device shows
up as a normal disk: the rw files work as usual, and the log appears as read-only files split by recording and by
time; exports to MF4, MCAP and CSV. The card is formatted on the device: put in a new card, say yes, done.

| crate | what |
|---|---|
| [`crates/logring`](crates/logring) | the engine: format, writer, recovery, reader, `TrackStore`, the view model, media |
| [`crates/logring_export`](crates/logring_export) | PC-side reader and exporter (CSV, MCAP, MF4) |
| [`crates/logring_cli`](crates/logring_cli) | the `logring` command line tool |
| [`crates/logring_py`](crates/logring_py) | the `logring` Python module (PyO3 + maturin) |

```sh
cargo install --path crates/logring_cli     # (in a checkout with tpm: `just install`)
logring info card.img         # sessions, recordings, tracks, counters
logring export card.img --format csv --out ./out
```

```python
import logring

card = logring.open("card.img")
df = card.track("can1_rx").to_pandas()
```

The medium is a trait (`LogMedium`, from `wire_weaver`'s `ww_stream`), so storage other than SD (SPI/QSPI flash,
eMMC) can follow. Not tied to one product: rePort (hardware project P2424) is the first user, and this is meant
to be the base for every logging device built after it.

See [AGENTS.md](AGENTS.md) for how the repo is laid out, [FEATURES.md](FEATURES.md) for what is implemented,
planned and still an idea, and [CHANGELOG.md](CHANGELOG.md) for the history.

Licensed under MIT OR Apache-2.0.
