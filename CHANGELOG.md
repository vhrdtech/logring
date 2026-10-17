# Changelog

## [Unreleased]

### Added

- Features `crc-table` (default on) and `crc-hw` for the CRC-32C of every block (`format::crc`): without
  `crc-table` it is computed bit by bit; with `crc-hw` on the MCU's CRC unit, a `Crc32cHw` the firmware names
  with `ww_logring::set_crc32c!` (a build without it fails to link). Same value in every build. `just size cuts`,
  M33: 0.9-1.3 KB less flash without the table (opt s), `crc-hw` 1.0 KB less with a dummy unit (LOG-1,
  P2628#17oct-1623c).
- Feature `card-check` (default on): `format_card`'s card check (aliasing test, write-size sweep, `CheckOptions`,
  `alias_test`, `sweep`); a small build without it formats unchecked, its `CardReport` says so
  (`alias_checked: false`, an all-zero `Sweep`, `card_flush_bytes` 0: the fallback or the setting). `just size
  cuts`, M33: `full` 5.0 KB (opt s) / 3.6 KB (opt z) less flash without it (LOG-3, P2628#17oct-1623b).
- `just size cuts`: the flash each feature of the size crate costs, every bin built with it off (LOG-9).
- `just size`: flash and RAM of the device side per MCU target (Cortex-M0+, M4/M7, M33, RISC-V imac/imc) at
  opt-level s and z, per crate, source file, function and type, from size examples in `size/` (LOG-9).
- `ww_logring::reader`: the indexed reader (no_std, no alloc): seeks by track position and by device time with
  binary searches over segment and flush heads, recordings and their records, segment summaries; `ImageFile`
  reads a ring image on a PC (LOG-2, P2628#16oct-1001c).
- `TrackStore`: one track of a logring as `wire_weaver`'s `StreamStore`, so a reliable stream is kept on the
  card and a host backfills from it after reboots; `Logring::release`, `reset_track`, `durable_newest`; track
  entries carry the track's stream session and its first segment (LOG-2).
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

- The reader is smaller on the device: `seek_time` orders times with `reader::time_cmp` (u64, no 128-bit
  arithmetic; `time_ps` stays for the PC), and `recordings` walks the meta area with `OneTrack` instead of its own
  `AllTracks<1>` (one copy of the walker less). `just size`, M33 opt s: the reader alone 45.9 -> 39.4 KB, the
  other bins unchanged (LOG-2, P2628#17oct-1623d).
- `format::CRC` (the `crc` crate's `Crc<u32>`) is now `format::crc::{checksum, Digest}` (LOG-1).
- The writer no longer sits in RAM twice: the stage buffer and the per-track state are `Buffers<BUF, TRACKS>`
  (`const fn new`, all zero, so a static of it is in `.bss`), lent by the firmware to `Logring::new(medium,
  &mut buffers, cfg, info)`, which checks and writes nothing; then `mount().await` / `format().await` run in
  place on `&mut self` (writing before is `Error::NotMounted`, twice `Error::Mounted`); `Buffers::stage` is
  `format_card`'s scratch before that. `mount_leaked`/`format_leaked` (`std`) for tests and tools. `just size`,
  M33 opt s, static + task RAM: raw writer 17.0 -> 9.4 KB, exFAT + `TrackStore` 19.6 -> 11.8 KB, `full` 24.5 ->
  13.8 KB; flash 0.9-1.3 KB less (LOG-1, P2628#17oct-1623).
- Track identity is numbers only: `TrackDesc::path` is the stream's path in the device API as wire_weaver ids
  (`RefVec<UNib32>`, up to `MAX_PATH`), the item type is its `ww_self` signature and `ww_global` id (both
  optional); crate name, version, type path and units are no longer stored with a track: the PC takes them from
  the bundle. `SessionInfo::api` takes the API root's `FullVersion` (`<ROOT_TRAIT>_FULL_GID`) instead of
  `api_crate`/`api_major`; the stable key is FNV-1a of `api_root_id` (a `const fn`) and the path ids. The meta
  session record carries the API root id and its `FullVersion`, a fallback for a host without the bundle. The
  format changed, no old image reads (LOG-1, P2628#17oct-1507).
- Up to 65535 data tracks per session (was 63): track counts and numbers are uvar in segment heads, track entries
  and the meta session record; `MAX_TRACKS` is 65536 and the mount, the reader and the flush heads need no array
  sized by it (`AllTracks<N>`, `seek_time::<N, _>`, `summary::<N, _>`, `recording::<N>` take the tracks to decode).
  Flush heads are full or sparse: a full head lists every track (a segment's first flush, then whenever the
  sparse heads since would add up to one, at the latest after 32 times its size of data), a sparse one only the
  tracks written since its full head and how many blocks back that one is; so many quiet tracks don't cost in every
  flush. docs/format.md says what a track costs in RAM and on the card (LOG-1, P2628#17oct-1507).
- wire_weaver crates by path (`../wire_weaver`, its master) instead of a pinned git rev; `just deps-pull`, `deploy`
  pulls it first, CI checks wire_weaver out next to the repo.

