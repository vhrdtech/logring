# The logring format (version 1)

The on-medium layout `logring` writes and reads, as it is. The medium is anything implementing `wire_weaver`'s
`ww_stream::store::LogMedium` (a window of an SD card, a NOR flash, a simulated card in tests); on an SD card it is
the preallocated ring file `logring.wlr` of an exFAT volume (LOG-3). Every number is little endian unless it says
varint. Every structure a reader trusts has a CRC-32C (`CRC_32_ISCSI`).

## 1. Layout

```text
segment 0        superblock slot A (block 0) ... slot B (block at segment_size / 2)
segments 1..=m   the meta area: a ring of m segments (m = meta_segments, 4 by default)
segments m+1..   the data ring: n segments, as many as fit (n = data_segments)
```

- A **segment** is `segment_size` bytes: the card's allocation unit (4 MB when the card reports none), a multiple
  of 512, at least 4 blocks, and when the medium needs erasing a multiple of twice its erase size (the superblock
  slots sit in separate erase units). Geometry is fixed at format and checked at every mount.
- A **block** is 512 bytes: 504 bytes of payload and an 8-byte trailer. Blocks are written once per lap of their
  ring, in order inside a segment, never rewritten in place. A segment is erased before it is reused when the
  medium needs it.
- Segment **seq** numbers count up for the life of the log, per ring; the segment with seq `s` sits in slot
  `s mod n` of its ring (data: at byte `segment_size * (1 + m + s mod n)`; meta: `segment_size * (1 + s mod m)`).
  Blocks of an earlier lap in the same slot never check out as the current segment (their tag and CRC depend on
  the seq).

## 2. Blocks

Trailer, at payload offset 504:

| off | size | field |
|---|---|---|
| 504 | u16 | `tag`: from the log id, the ring (data/meta) and the segment seq: a fast "is this block ours" check |
| 506 | u16 | `flush_off`: blocks since the start of the flush this block belongs to (0: the flush starts here) |
| 508 | u32 | `crc`: CRC-32C over `log_id` (u32), the ring role (u8, 0 data, 1 meta), `seq` (u64), the block's index in the segment (u32), the 504 payload bytes, `tag` and `flush_off` |

`tag = ((seq ^ (log_id << 20) ^ (role << 60)) * 0x9e3779b97f4a7c15) >> 48`, with `0xffff` replaced by `0x5a5a` and
`0x0000` by `0xa5a5` (never the erased pattern). A block whose tag matches but whose CRC doesn't is **torn**: the
write a power cut interrupted, or a block the card damaged.

The payload bytes of a segment's blocks, in order, form one byte stream; flush heads and records are read from it
across block boundaries.

## 3. Superblock

One block, in slot A (byte 0) or slot B (byte `segment_size / 2`), written in turn (`write_seq mod 2`); the slot
with the higher `write_seq` and a good CRC wins. Written at format, at every mount, when a data segment opens every
few segments (`clamp(n / 4, 1, 16)`), when a meta segment opens, and when a recording starts or stops. It holds
hints and counters only: the state of the log is recovered from the segments (§8).

| off | size | field |
|---|---|---|
| 0 | 4 | magic `WLRS` |
| 4 | u8 | version, 1 |
| 8 | u64 | `write_seq` |
| 16 | u32 | `log_id`: new at every format |
| 20 | u32 | `segment_size` |
| 24 | u32 | `meta_segments` (m) |
| 28 | u32 | `data_segments` (n) |
| 32 | 16 | `binding`: the ring file's place on the card, four u32: volume serial, first cluster, cluster count, start LBA (low 32 bits); zero on a raw medium (§10) |
| 48 | u32 | `next_session`: the session the next mount gets (sessions start at 1) |
| 52 | u32 | `next_recording`: the id the next recording gets (1 at format; 0 means "none") |
| 56 | u64 | `data_tail_seq`: highest data seq opened when this was written; `u64::MAX` before the first |
| 64 | u64 | `meta_tail_seq`: the same for the meta ring |
| 72 | u64 | `bundle_hash`: FNV-1a of the API bundle stored last |
| 80 | u64 | `bundle_meta_seq`: meta seq its record is in; `u64::MAX`: none stored |
| 88 | u32 | `recording_id`: the recording running (0: none) |
| 96 | u64 | `recording_start_seq`: the data segment it started in |
| 104 | u64 | `bytes_written`: data and meta bytes written in all |
| 112 | u64 | `segments_reused`: data segments the ring went round and overwrote |
| 120 | u32 | `meta_reused`: meta segments reused |
| 124 | u32 | `flush_bytes`: the best flush size the card check measured at format (0: not measured; LOG-3) |
| 128 | u64 | `meta_next`: the position the next meta record gets, as of this write (§7) |
| 136..508 | | reserved, zero |
| 508 | u32 | CRC-32C of bytes 0..508 |

**`meta_next` and `META_SLACK`.** Every superblock write carries the writer's `meta_next`, and a writer writes at
most `META_SLACK` = 4 meta records between two superblock writes (today at most 2: the bundle and the session at a
mount, one record per recording start or stop). The newest valid slot was written at some point W; at most
`META_SLACK` meta records came after W before the next superblock write, and a torn next write leaves W's slot
intact. So the meta ring's real next position is in `[meta_next, meta_next + META_SLACK]`, and a mount that doesn't
parse the meta tail can go on at `meta_next + META_SLACK` without ever reusing a position; it writes the superblock
with that `meta_next` before its first meta record (else a cut after that record and before the next superblock
write would leave a record past the bound, and the next such mount would reuse its position). A mount that parses
it takes the larger of what it found and `meta_next`.

Slot B is found through the segment size; a reader without one (slot A torn, no config) tries the powers of two
from 2 KB to 64 MB.

## 4. Segments and flushes

A segment holds one session's data and consists of **flushes**: what the writer puts on the medium in one go
(`flush_bytes`, `flush_interval`, `sync()`, or right after a record of an `Immediate` track). Every flush starts at
a block boundary, carries its own head, and ends with padding to the end of its last block (unless records fill it
exactly); so any flush, and any block run cut at flush boundaries (a split file), decodes without its neighbours.

The first flush of a segment starts with the **segment head**:

```text
u8    kind = 2 (a segment starts here)
u64   seq
u32   session
u32   flags: bit 0 FRESH (below), other bits 0
uvar  tracks: data tracks in the table (numbers 1..=tracks), at most 65535
       tracks × track entry (§5)
```

**`FRESH`** (flag bit 0) marks a session's first data segment when its mount didn't recover positions (a small
build that doesn't parse the tail): every track counts its positions from 0 here, the system track included,
and every data track's stream starts here (`stream_start` = this seq, a stream session with bit 31, §5); its
`SessionStart` has flag bit 1 (§7). The system track has no track table entry: `FRESH` is where its positions
restart (they continue for the log's life otherwise). Never set on meta segments. A reader ignores flag bits it
doesn't know, so a flag bit may only add information, never change how the rest of the segment reads.

Every flush then has a **flush head**: the positions the flush's records are delta-coded against.

```text
u8    kind = 1 (only when the flush doesn't start a segment)
u8    time length (0: no time) + the device time last seen, a ww_si::Second in its ShrinkWrap encoding
uvar  full_back: 0 = a full head; otherwise its full head is that many blocks before this flush's first block
uvar  entries
       entries × (uvar gap, uvar next): track numbers in increasing order, each as the gap to the one before
       (track - previous - 1; the first one as its number), and the track's next position
```

A **full** head lists every track (track 0 included). A **sparse** head lists only the tracks that got records
since its full head; a track it leaves out is where the full head put it. So any flush decodes with two heads, and
a few busy tracks among many quiet ones cost only their own entries. The first flush of a segment always has a
full head (its `full_back` points to block 0: that flush, right after the track table). The writer writes a full
head instead of a sparse one once the sparse heads since the last full one would list, all together, as many
entries as a full head has (so sparse heads never cost more than the full ones), and at the latest after 32 times
a full head's size of data (`FULL_EVERY`).

`uvar` is LEB128 (7 bits per byte, low bits first, at most 10 bytes); `ivar` is zigzag then `uvar`.

Every boot writes into a **new data segment** (the session's track table is in its head; a segment belongs to one
session); the rest of the segment it was in before is left as it is. The meta ring is appended to at the end found
by the mount, unless a torn block is in the way or the medium needs erasing (then the next meta record opens the
next meta segment).

## 5. Track table

A track is a stream of a `#[ww_trait]` in the session's API bundle; in a session it has a small number (0 is the
system track and is never in the table). One entry per data track, a `uvar` length then:

| field | type | what |
|---|---|---|
| number | uvar | track number in the session, 1 to 65535 |
| source | u8 | 0: this device (reserved for several devices logging into one card) |
| durability | u8 | 0 normal, 1 immediate |
| compression | u8 | 0 (reserved) |
| key | u64 | stable key, see below |
| flags | u8 | bit 0: a signature follows, bit 1: a global id follows |
| signature | 8 bytes | only with flag bit 0: the item type's `ww_self` signature (wire_weaver `docs/std_library/ww_self.md`) |
| global_id | uvar | only with flag bit 1: the `ww_global` id of the item type's crate (0 is a real id: `ww_log`) |
| rate | u32 | nominal items per second, 0 unknown |
| released | uvar | position released by the host (acknowledged) when the segment was opened |
| path | uvar length + bytes | the stream's resource path in the device API: a `RefVec<UNib32>` of wire_weaver ids in its ShrinkWrap encoding, the form requests use (wire_weaver `docs/api/addressing.md`), array indices included, at most 16 ids |
| stream_session | uvar | the track's stream session (`ww_stream`, set by `TrackStore::reset`), 0 before the first; bit 31 set: a session the logring started itself (below) |
| stream_start | uvar | data seq of the segment the stream session started in: the track's positions count from 0 there and grow in every segment after it |

Fields a later version appends inside the entry are ignored by this reader.

**No strings in a track's identity.** The device has numbers at hand, not names: a track is its path of ids and its
item type's signature and global id. Names (`can[1].rx`, `ww_can_bus::Frame` 0.3) are made on the PC from the API
bundle stored with the session (§6, meta `Bundle`): the path leads to the stream in the bundle, the stream to its
item type with crate, version and type path. Exporters recognise kinds of data (a CAN frame of `ww_can_bus`, a defmt
frame of `ww_log`) by the signature and global id, never by the path. What a position counts (bytes, frames,
samples) follows from the item type in the bundle.

**Stable key**: the same track across sessions, firmware updates that keep its path, and split files. From numbers
only: `h = FNV-1a(api_root as u64 LE)`, then FNV-1a goes on over every path id as u32 LE. `api_root` names the
device API across firmware updates: FNV-1a of the API root crate's name and its major version (u32 LE), the
`FullVersion` wire_weaver generates as `<ROOT_TRAIT>_FULL_GID`; `logring::api_root_id` is a `const fn`, so firmware
computes it at build time and the crate name never reaches the card through a track.

A track's positions are monotonic from its `stream_start` segment on, across sessions: a mount continues them by
key from the tail segment's table, and a track the tail segment doesn't name starts at 0 in the new session's
segment (its `stream_start`). A stream reset (`Logring::reset_track`) closes the data segment and opens the next
one at once, its head carrying the new `stream_session` and `stream_start`.

**Stream sessions with bit 31** (`STORE_SESSION`) are reserved for streams the logring starts by itself: a mount
that doesn't recover positions starts every data track's stream at 0 in session `0x8000_0000 | session` (the
logring session of that boot, unique for the card's life). Hosts and firmware choose stream sessions with bit 31
clear (`ww_stream`'s `StoreState::session`; `Logring::reset_track` refuses others), so such a session never
matches one a host remembers and a host never resumes across positions that restarted.

FNV-1a, 64 bit: `h = 0xcbf29ce484222325; for each byte: h ^= byte; h *= 0x100000001b3`.

## 6. Records

A record is a track number, the event's `StreamHead` (as `ww_stream` sends it on the wire: position, units, lost,
device time, rare markers) and the item's bytes, any ShrinkWrap type. The head is delta-coded against the track's
previous record in the same flush, so a small item on a busy track costs about 4 bytes.

```text
byte 0:  t t t f f f f f   ttt: track 0..6, or 7 = a uvar of (track - 7) follows
                           fffff: FULL POS UNITS LOST T
FULL:    u8 head length, then the StreamHead's ShrinkWrap bytes (the other flags are 0)
POS:     uvar pos; otherwise pos = the track's previous end + lost
UNITS:   uvar units; otherwise 1
LOST:    uvar lost; otherwise none
T:       ivar time delta against the track's previous time (same prefix, both U32 or both U64); otherwise no time
then:    uvar item length, the item bytes
0xff:    padding: no record here, skip to the end of the block
```

The "previous end" of a track at a flush start is its `next` in the flush head; the previous time is unknown then.
The first record of a track in a flush (if it carries a time), a record whose time can't be a delta (another
prefix or kind) and a record with `ext` (a clock epoch marker) store the head in full. A record may span blocks but
never segments: a record that doesn't fit the rest of the segment closes it (padding) and starts the next one. The
item is at most `MAX_ITEM` (1 MB) and has to fit a segment after its head and the track table.

## 7. System track and meta records

**Track 0** (data ring) is the recorder's own, written at once (immediate). Its items: a kind byte, then fields;
unknown kinds are skipped by length.

| kind | event | fields |
|---|---|---|
| 1 | SessionStart | session u32, torn_blocks uvar, torn_records uvar, flags u8 (below), device_id, fw, hw (strings, ≤ 64 bytes), bundle_hash u64 |
| 2 | Marker | kind u8, label string, value: u8 0 (none) or 1 + ivar |
| 3 | RecordingStart | id u32, trigger u8 |
| 4 | RecordingStop | id u32, reason u8 |
| 5 | Dropped | track uvar, units uvar |
| 6 | CardEvent | kind u8, value uvar |

`SessionStart` flags: bit 0 *formatted* (a new log was started), bit 1 *positions not recovered* (the mount
didn't parse the tail: every track, the system track included, starts at 0 in the session's first segment;
`torn_records` 0 then means "not counted", not "none"). Other bits are written 0; a reader
ignores bits it doesn't know.

**The meta area** uses the same segments, flushes and records (track 0 only, positions counting meta records;
every meta record is its own flush, written at once). Meta positions increase strictly over the whole meta ring
and may skip (by up to `META_SLACK` per mount, §3): they are counted, never searched. Its items:

| kind | record | fields |
|---|---|---|
| 1 | Session | session u32, api_root u64 (§5), api (uvar length + the API root's `FullVersion` in its ShrinkWrap encoding, at most 128 bytes), device_id, fw, hw, bundle_hash u64, tracks uvar, then `tracks` track entries (§5, `released` 0) |
| 2 | Bundle | hash u64, len u32, then the bundle's bytes (the `ww_self` ApiBundle); stored once per hash while its record is still in the meta ring |
| 3 | Recording | id u32, stopped u8, why u8 (trigger at a start, reason at a stop), session u32, data seq u64, time (length byte + Second) |

The session's `api` is the one place the API root's crate name and version are stored as text: what the firmware has
as a constant anyway, kept as a fallback for a host that doesn't have the bundle with `bundle_hash` (it can still
say which API and version wrote the session, and look its snapshot up). Tracks and records name nothing by string.

A meta segment is reused when the ring goes round (`meta_reused` counts it); the data segments still carry their
own track tables, so data stays decodable without the meta area (the bundle is the exception: it is stored again
when the superblock no longer points at a record inside the ring).

## 8. Mount and recovery

The state of the log is in the segments; the superblock only says where to look. A mount reads a bounded number of
blocks, whatever happened before (asserted in `tests/writer.rs`, `mount_reads_are_bounded`):

1. Both superblock slots (2 reads). None valid: a new log is formatted. Geometry has to match the config.
2. For the meta ring, then the data ring: the **tail segment**: the hinted seq's block 0; if it isn't there, the
   seq before it; then forward while the next seq's block 0 checks out (at most as many as the superblock interval,
   normally). A torn block 0 is counted and the segment before it is the tail.
3. The **end** of the tail segment: a binary search for the last valid block (the written blocks form a prefix of
   the segment), then four probes past it (a block the card damaged inside the prefix would stop the search early;
   torn blocks seen are counted).
4. The last block's `flush_off` says where the **last flush** starts; its head and, when it is sparse, its full
   head give every track's position at its start, the segment head gives the track table (and `released` per
   track); the flush's records are parsed to the end (a record cut short by the end is counted as torn). Tracks are matched to the new session's by key: their
   `next` continues, new keys start at 0. The meta ring's next position is the larger of its last flush's and
   the superblock's `meta_next`. Malformed content never fails a mount: what was parsed counts, the rest
   is reported as torn.
5. The new session: `next_session`, the meta records (the bundle if it isn't in the ring, the session with its
   track table), the superblock, a new data segment with its segment head and a `SessionStart` record carrying
   the recovery counts; everything synced before the mount returns.

What a power cut costs: the records in the stage buffer since the last completed write (at most `flush_bytes`,
or `flush_interval` of data, or the stage buffer's size when that is smaller), and possibly the record being written
when it hit (torn). Nothing written before is lost: blocks are never rewritten, the filesystem is never touched
while logging, and a torn block is skipped and counted. `Logring::durable(track)` says up to which position a
track's records are on the medium.

Segment seqs on NOR: a new segment is erased at open; a cut during the erase leaves garbage its block 0 doesn't
pass, so the next mount takes the segment before it as the tail and reuses the slot.

## 9. Parameters

| | default | where |
|---|---|---|
| `segment_size` | 4 MB (the card's AU) | `Config`, fixed at format |
| `meta_segments` | 4 | `Config`, fixed at format |
| `flush_bytes` | 64 KB | `Config` / `set_flush`, a run-time setting (rounded to whole blocks; at most the segment's payload) |
| `flush_interval_ms` | 1000 | `Config` / `set_flush` |
| `full` | drop oldest | `Config`; drop newest refuses records (`Error::Full`) once the data ring has been written once |
| stage buffer | `BUF` const generic, 512 B and up | whole blocks are assembled in it; a flush bigger than it is written in `BUF`-sized pieces, sequential inside the segment |
| tracks | `TRACKS` const generic, up to 65536 | the system track included; what one costs: below |

**What a track costs.** In RAM: about 150 bytes per `TRACKS` slot in the writer (its state, positions and codec;
asserted in `tests/many_tracks.rs`), plus its `TrackDesc` (80 bytes and the path's ids, can be a `static` in
flash). A device picks `TRACKS` for what it logs; nothing else in the writer, the mount or the indexed reader grows
with it (flush heads are written and read entry by entry; `AllTracks<N>` decodes tracks below its own `N`). On the
medium: its track entry in every segment head (about 35 bytes with a short path, 1024 tracks take about 36 KB of a
4 MB segment) and in the meta session record, an entry in every full flush head (2 to 4 bytes) and in the sparse
heads after it was written, and per record one byte more for track numbers 7 to 134 (two from 135). With 1024
tracks of which three are busy, the busy data takes about 7 % more room than with the three alone, nearly all of
it the segment heads' tables (`a_thousand_tracks`).

## 10. On an SD card: the ring file

On a card the medium is the file `logring.wlr` in the root directory of the card's exFAT volume (`logring::exfat`,
feature `exfat`). The device writes it by raw sector address, past the filesystem, so it has to know the file
is still where it put it:

```text
MBR | gap to the first boundary unit | exFAT boot regions | FAT | cluster heap:
    bitmap, up-case table, root dir, README.txt, ... rw files (rw_bytes free, 128 MB default) ... logring.wlr
```

- **Placement** (format and re-create): the ring file is contiguous (`NoFatChain`, the FAT untouched), starts on
  the card's allocation unit (and a cluster) and is whole segments long, at the end of the longest free run of
  clusters, with `rw_bytes` of all free space left free. Attributes read-only + system + hidden (+ archive).
- **Segment size** from the card's SD Status (ACMD13): the largest of `AU_SIZE`, `UHS_AU_SIZE` and
  `VSC_AU_SIZE` (up to 64 MB), at least 4 MB (a multiple of the AU when that is smaller); 4 MB when the card
  reports none. The exFAT boundary unit is the AU when it is a power of two larger than the SD Association
  default for the capacity.
- **Binding**: at format the superblock's `binding` gets the volume serial, the file's first cluster, cluster
  count and start LBA. A mount (`exfat::open`) checks, in order: the volume mounts (boot checksum and fields);
  `logring.wlr` exists, is a file, `NoFatChain`, of whole sectors, valid data length = data length, every
  cluster set in the bitmap; slot A or B decodes (slot B searched at half of every AU size from 16 KB to 64 MB
  when A doesn't); the binding matches (serial: else *other card*; first cluster and start LBA: else *moved*;
  cluster count: else *resized*); the superblock's segments fit the file. Anything else leaves the ring
  *unbound*: nothing is written until the device re-creates the ring file (a new log; the rw files stay) or
  formats the card. Lost attributes are only a warning (the place is checked anyway) and can be restored.
- **Card check** before a format (it writes to the card): an aliasing test (a tag at sector 0, then at every
  power of two, 64 spread points and the last sector, each read back with sector 0 after it, then all read again:
  a fake card wraps or drops writes and fails at its real size) and a write-size sweep (16 KB to 1 MB, 4 MB at each
  size, each from its own AU, in the second half of the card): the smallest size within 90 % of the fastest
  becomes `flush_bytes` (rounded to 16 KB), stored in the superblock's `flush_bytes`; 64 KB when nothing was
  measured. Sizes larger than the scratch buffer the device gives are not measured.

## 11. Reading

`logring::reader` (no_std, no alloc) reads a ring without a scan, from what the format above already carries:

- **By track position**: a binary search over the data segments of one stream of the track (the one current at
  the segment asked for, normally its `stream_start`: a segment whose entry names a later `stream_start` is past
  the stream, and a walk stops there; positions restart in every stream)
  by the track's `next` in each segment's first flush head, then over the segment's blocks: a block's `flush_off`
  gives the start of its flush, the flush head (or its full head, when the track isn't in a sparse one) the
  track's `next` there. The last flush starting at or before the
  position is scanned (about 2 log₂ n + 2 log₂ blocks reads for the seek). Other tracks' records are stepped over
  by their lengths, without their delta state.
- **The system track by position**: the same, from the segment asked for up to the next `FRESH` segment (found
  by reading the segment heads after it, one block each), where its positions restart.
- **By time**: the same over a session's consecutive segments and its flushes, by the device time in the flush
  heads (the time last seen before the flush, so the flush found starts at or before the time). Times are
  compared as numbers: a clock that wraps inside a session breaks the order.
- **By recording**: the `Recording` meta records give its data segment; its records are those between its
  `RecordingStart` and `RecordingStop` in track 0.
- **Segment summaries** (per track count, bytes, first and last position and time; markers; flushes) are
  computed from one segment's records; they are not written into the segment.

`TrackStore` (one track as `ww_stream`'s `StreamStore`) reads back only what is on the medium: an event sent from
the stage buffer would be lost by a power cut while a host already has it, and the positions after the reboot
would go to other events.

## 12. Not in this version

Per-track compression (the `compression` byte is reserved), pinned recordings or a keep ring, several sources per
card (`source` reserved), encryption, segment summaries written at a segment's end (they are computed by the
reader).
