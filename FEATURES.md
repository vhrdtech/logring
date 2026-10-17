# Features

Single source of truth for what logring does and what is planned (see [AGENTS.md](AGENTS.md)).

Status: ✅ done, 🧪 tested on fixtures/images only, 📋 planned (with a P2628 task), 💡 idea, not filed as a task yet.

## Engine (`LOG`)

- 🧪 **LOG-1 format-writer**: the format ([docs/format.md](docs/format.md): superblock A/B, meta area, AU-sized
  segments, self-identifying 512-byte blocks with an 8-byte trailer written once per lap, flushes with a head per
  flush, compact delta-coded records of track + `StreamHead` + item at ~4 bytes of overhead, track tables in every
  segment head, system track 0, meta records for sessions, bundles and recordings), the writer (`Logring`:
  mount/format, `push`/`push_head`, markers, recordings, stage buffer, `flush_bytes`/`flush_interval`/`sync`,
  `Immediate` tracks, drop oldest/newest) and recovery (bounded mount, torn blocks and records counted), on
  `ww_stream`'s `LogMedium` with `SimSd`/`SimNor`. Tests: random multi-track workloads round-trip, a power cut
  at every write unit on SD and NOR loses nothing written before it, mount reads under a computed bound, damaged
  images never panic; `fuzz/` mount target with seed images. Not yet checked on a real card (LOG-3, TEST-22).
  P2628#16oct-1000. Plan: `prj/P2628-logring/design/plan.md` R1. Differences from the plan: per-track release
  cursors live in the segment heads (not the superblock, which stays one fixed block); the API bundle is hashed
  with FNV-1a (64 bit); a cut leaves what the stage buffer held since the last completed *write* (the stage
  buffer `BUF` may be smaller than `flush_bytes`).
- 📋 **LOG-2 reader-trackstore**: reader by track/time/recording with binary search, segment summaries;
  `TrackStore` implementing `wire_weaver` `ww_stream`'s `StreamStore` so a reliable stream backfills from the
  logring. After LOG-1. P2628#16oct-1001c. Plan R5.
- 📋 **LOG-3 logring-on-exfat**: `ExfatRing` medium on `exfat_lite`, mount binding check + `Unbound`, re-create
  ring file, format flow with card check (aliasing for fake cards, a write-size sweep storing the best
  `flush_bytes`), geometry from `CardInfo` (SD Status AU/RU/erase, with fallbacks), a `block-device-driver`
  medium with multi-block writes. After LOG-1, FS-1. P2628#16oct-1001d. Plan R6.
- 💡 **LOG-4 merge-logstore**: whether `ww_stream`'s `LogStore` (one track, raw NOR/SD) could become a one-track
  logring later; decided after the logring ships.
- 💡 **LOG-5 pinned-recordings-keep-ring**: pinned recordings the ring skips when wrapping, or a second small
  "keep" ring for important tracks (events, errors, config changes) so bulk data doesn't overwrite them.
- 💡 **LOG-6 encryption-signing**: optional encryption or signing of customer data at rest.
- 💡 **LOG-7 multi-source-card**: several devices logging into one card (the track table's `source` field is
  reserved for this, not built).
- 💡 **LOG-8 defmt-elf-fw-registry**: fetch the defmt ELF for decoding by build id from a future fw_registry,
  instead of a path given by hand.

## USB view (`VIEW`)

- 📋 **VIEW-1 usb-view**: `ww_logring::view` — a snapshot tree by recording/time/track, native `.wlr` split
  files, `index.csv`, `README.txt`, fixed-width CSV, lowercase names (paired with `exfat_lite`'s FS-2). After
  FS-1, LOG-2. P2628#16oct-1001f. Plan R8.
- 📋 **VIEW-2 passthrough-lun-guard**: the real exFAT volume's LUN, with the ring-extent write guard (SCSI data
  protect sense) and the ring file's read-only/system/hidden attributes, so rw files work as on any card. After
  LOG-3, VIEW-1; `usb_msc_device`'s MSC-1 for the class itself. P2628#16oct-1001g. Plan R9.
- 💡 **VIEW-3 mtp-view**: an MTP view as well as (or instead of) MSC — handles a growing tree cleanly, but no
  device-side MTP exists in Rust yet and macOS needs a third-party app; an idea for later.
- 💡 **VIEW-4 mf4-can-on-device**: render MF4 for CAN tracks on the device itself, not only on the PC.

## PC (`PC`)

- 📋 **PC-1 pc-tool**: `ww_logring_export` (dynamic decode from the stored bundle, CSV, MCAP JSON + jsonschema,
  MF4 incl. CAN bus logging), the `logring` CLI (`info`/`ls`/`export`/`cat`/`check`/`make-image`, `--version`,
  completions, progress bars), `ww_logring_py` (PyO3 + maturin, via uv). After LOG-2 (VIEW-1 for `ls`).
  P2628#16oct-1001h. Plan R10.
- 💡 **PC-2 fuse-mount**: `logring mount` (FUSE), so a card or image shows as a normal read-only filesystem
  without the `logring` CLI's own subcommands.

## Hardware tests (`TEST`)

- 📋 **TEST-22 sd-power-cuts**: power cut through switched VBUS (U601 rig, as `wire_weaver` TEST-21) during
  logging on rePort, thousands of cycles per card and per `flush_bytes`; report per card and setting. After
  rePort integration (usb_io_fw). P2628#16oct-1002b. Plan R12. (Claimed as TEST-22, not TEST-1: `tpm work
  claim`'s next-free scan picked up the "TEST-21" cross-reference to `wire_weaver` in this task's own text as if
  it were a used id in this repo's TEST area; harmless, just a gap in the numbering — see the P2628#16oct-1016
  report.)
