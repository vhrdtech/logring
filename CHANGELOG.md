# Changelog

## [Unreleased]

### Added

- `ww_logring::exfat` (feature `exfat`, on `exfat_lite`): the ring as the file `logring.wlr` of an SD card's exFAT
  volume. `ExfatRing`, a `LogMedium` window over the file's sectors that refuses anything outside it; `open`, the
  mount check (volume, contiguous file of whole sectors, bitmap bits, header A/B and its binding to the volume
  serial, first cluster, cluster count and start LBA) giving a `Bound` ring or `Unbound` with the reason
  (missing, not contiguous, size, not allocated, no header, other card, moved, resized), attribute changes as a
  warning with `restore_attributes`; `format_card` with the card check (aliasing test for fake cards, a write-size
  sweep 16 KB..1 MB that picks the card's `flush_bytes`) and the geometry from `CardInfo` (SD Status: AU, UHS/VSC
  AU, erase, speed classes; 4 MB / 64 KB fallbacks); `recreate_ring`, keeping the rw files (LOG-3,
  P2628#16oct-1001d).
- `ww_logring::bdd::BddDevice` (feature `block-device-driver`): an SD driver implementing `block-device-driver`'s
  `BlockDevice<512>` under `ExfatRing` and the format, each multi-sector write one driver call (LOG-3).
- `Config::binding` (written into the superblock at format; a mount of a log bound elsewhere fails with
  `Error::Unbound`) and `Config::card_flush_bytes` (the measured flush size, kept in the superblock) (LOG-3).

- `ww_logring`: the ring format, the writer and power-cut recovery (`Logring::mount`/`format`, `push`,
  `push_head`, `marker`, `recording_start`/`stop`, `sync`, `flush_if_due`, `durable`), with sessions, track
  tables, the system track and the meta area; `docs/format.md` describes the layout; a `std` sequential `scan`
  and a cargo-fuzz target for mount with seed images (LOG-1, P2628#16oct-1000).
- Repo scaffold: workspace (`ww_logring`, `ww_logring_export`, `ww_logring_cli` binary `logring`,
  `ww_logring_py`), `just test`/`lint`/`install`/`deploy`, CI (fmt, clippy, tests, typos, cargo sort), licence
  (MIT OR Apache-2.0) (P2628#16oct-1016).
- FEATURES.md with the planned items and ideas of `prj/P2628-logring/design/plan.md`: areas `LOG` (LOG-1..8),
  `VIEW` (VIEW-1..4), `PC` (PC-1, PC-2), `TEST` (TEST-22), each bound to its P2628 task where one exists
  (P2628#16oct-1016).

### Changed

- wire_weaver crates by path (`../wire_weaver`, its master) instead of a pinned git rev; `just deps-pull`, `deploy`
  pulls it first, CI checks wire_weaver out next to the repo.

