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
- 🧪 **LOG-2 reader-trackstore**: `ww_logring::reader` (no_std, no alloc): a block-cached `Walker` over the
  data or meta segments decoding one track by stable key (`OneTrack`, the other tracks stepped over) or all
  (`AllTracks`); `seek_track` (binary search over segment and flush heads by a track's position), `seek_time`
  (by device time in a session), `recordings` and the records of a recording, `summary` of a segment;
  `ImageFile` (std) reads an image or ring file read-only. `TrackStore`: one track of a logring shared through
  an `embassy-sync` mutex as `ww_stream`'s `StreamStore`, so `Reliable<TrackStore>` keeps a reliable stream on
  the card: positions and the stream session continue across reboots, the first event after a mount carries the
  clock epoch, `dropped_units` counts what the ring overwrote unacknowledged, `reset` opens a new segment for the
  new stream session. Tests: the reader against the sequential scan on a wrapped three-session ring and from an
  image file, the corruption harness and fuzz target run it on damaged images; `Reliable<TrackStore>` across five
  boots with power cuts (every event that reached the medium, once); a generated `wire_weaver` server with the
  real host event loop resuming across four reboots (`tests/stream_e2e`). Not on a real card yet (LOG-3,
  TEST-22). P2628#16oct-1001c. Plan R5. Differences from the plan: `TrackStore` reads back only what is on the
  medium, never from the stage buffer (a power cut would otherwise reuse positions a host already has), so a live
  view follows the flushes; segment summaries are computed by the reader, not written at a segment's end (the
  flush heads already make the binary searches work); a track's released position is kept in the segment heads,
  so a reboot can forget acknowledgements since the last segment opened (the host's resume cursor covers them);
  no `ww_stream` change was needed.
- 🧪 **LOG-3 logring-on-exfat**: `ww_logring::exfat` (feature `exfat`, on `exfat_lite`): `ExfatRing`, the ring
  file `logring.wlr` as a `LogMedium` window that refuses any access outside it; `open`, the mount binding check
  (volume, contiguous file, whole sectors, bitmap, header A/B bound to volume serial, first cluster, cluster count
  and start LBA) giving `Bound` or `Unbound(reason)`, lost attributes a warning with `restore_attributes`;
  `format_card` with the card check (aliasing test for fake cards, a write-size sweep 16 KB..1 MB that stores the
  best `flush_bytes` in the superblock) and geometry from `CardInfo` (SD Status `AU_SIZE`/`UHS_AU_SIZE`/`VSC_AU_SIZE`,
  erase fields, speed classes; 4 MB segments / 64 KB flushes as fallbacks); `recreate_ring` keeping the rw files;
  `bdd::BddDevice` (feature `block-device-driver`): a `block-device-driver` SD driver under it, multi-block writes
  in one call (straight from the writer's aligned stage buffer). Format: docs/format.md §10. Tests on in-memory
  card images: deleted, renamed, truncated, short valid length, moved, defragmented, freed clusters, replaced,
  damaged header, copied from another card, attributes changed; wrapping and dropping fake cards caught; sweeps on
  EVO-like (64 KB dip) and flat timing models; `fsck.exfat -n` clean after format and logging. Not yet run on a
  real card or rePort (needs the SD driver with `CardInfo`, P2628#16oct-1002; TEST-22). P2628#16oct-1001d. Plan
  R6. Differences from the plan: the recording unit `flush_bytes` is rounded to is 16 KB for every card (the
  speed-class RU of larger classes isn't applied: it would force 512 KB flushes); lost attributes don't unbind the
  ring; a `VSC_AU_SIZE` over 64 MB is left out.
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
