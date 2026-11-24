---
level: nomicon
---

# The logring format (version 1)

This page describes the on-medium layout that `logring` writes and reads, as it is. Every number is little endian
unless the page says varint.

The medium is anything that implements `ww_stream::store::LogMedium` from the wire_weaver crates. Examples are a
window of an SD card, a NOR (logical not-or) flash chip and a simulated card in tests.

On an SD card the medium is `logring.wlr`, the preallocated ring file of an exFAT volume. Every structure that a
reader trusts has a CRC, the CRC-32C (`CRC_32_ISCSI`).

## 1. Layout

```text
segment 0        superblock slot A (block 0) ... slot B (block at segment_size / 2)
segments 1..=m   the meta area: a ring of m segments (m = meta_segments, 4 by default)
segments m+1..   the data ring: n segments, as many as fit (n = data_segments)
```

- A **segment** is `segment_size` bytes. The segment size follows these rules:
    - It is the card's AU, or 4 MB when the card reports none.
    - It is a multiple of 512 and at least 4 blocks.
    - When the medium needs erasing, it is a multiple of twice the erase size, so the superblock slots sit in
      separate erase units.
- A **block** is 512 bytes: 504 bytes of payload and an 8-byte trailer. The writer writes blocks in order inside a
  segment, once per lap of their ring, and never rewrites one in place.
- When the medium needs erasing, the writer erases a segment before it reuses it.
- Segment **seq** numbers count up for the life of the log, per ring. The segment with seq `s` sits in slot
  `s mod n` of its ring.
- In the data ring that slot starts at byte `segment_size * (1 + m + s mod n)`. In the meta ring it starts at byte
  `segment_size * (1 + s mod m)`.
- Blocks of an earlier lap in the same slot never check out as the current segment. Their tag and CRC depend on the
  seq.

The format fixes the geometry, and every mount checks it.

## 2. Blocks

Trailer, at payload offset 504:

| off | size | field |
|---|---|---|
| 504 | u16 | `tag`: from the log id, the ring (data/meta) and the segment seq: a fast "is this block ours" check |
| 506 | u16 | `flush_off`: blocks since the start of the flush this block belongs to (0: the flush starts here) |
| 508 | u32 | `crc`: CRC-32C over `log_id` (u32), the ring role (u8, 0 data, 1 meta), `seq` (u64), the block's index in the segment (u32), the 504 payload bytes, `tag` and `flush_off` |

<figure class="ww-figure" markdown="0">
--8<-- "docs/plots/trailer.svg"
<figcaption>A trailer from a ring `logring_figures` writes and reads back: the tag, the flush offset, and the
CRC-32C over the fields above. Drawn from the drive example, with segments of 8 KiB and flushes of 1 KiB instead
of 64 KiB and 16 KiB. The flush offset of 2 depends on that shrink.</figcaption>
</figure>

`tag = ((seq ^ (log_id << 20) ^ (role << 60)) * 0x9e3779b97f4a7c15) >> 48`, with `0xffff` replaced by `0x5a5a` and
`0x0000` by `0xa5a5` (never the erased pattern). A block whose tag matches but whose CRC doesn't is **torn**: the
write a power cut interrupted, or a block the card damaged.

The payload bytes of a segment's blocks, in order, form one byte stream; flush heads and records are read from it
across block boundaries.

## 3. Superblock

The superblock is one block in slot A at byte 0 or in slot B at byte `segment_size / 2` of the segment. The writer
writes the two slots in turn (`write_seq mod 2`), and the slot with the higher `write_seq` and a good CRC wins.

The writer writes the superblock at these points:

- at format and at every mount;
- when a data segment opens, every few segments (`clamp(n / 4, 1, 16)`);
- when a meta segment opens;
- when a recording starts or stops.

The superblock holds hints and counters only. A mount recovers the state of the log from the segments (§8).

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
| 124 | u32 | `flush_bytes`: the best flush size the card check measured at format (0: not measured) |
| 128 | u64 | `meta_next`: the position the next meta record gets, as of this write (§7) |
| 136..508 | | reserved, zero |
| 508 | u32 | CRC-32C of bytes 0..508 |

**`meta_next` and `META_SLACK`.** Every superblock write carries the writer's `meta_next`. Between two superblock
writes, a writer writes at most `META_SLACK` = 4 meta records.

Today a writer writes at most 2 there: the bundle and the session at a mount, and one record per recording start or
stop.

The newest valid slot was written at some point W in time. At most `META_SLACK` meta records came after W before
the next superblock write, and a torn next write leaves the slot of W intact.

The meta ring's real next position is therefore in `[meta_next, meta_next + META_SLACK]` and no later. A mount that
doesn't parse the meta tail can go on at `meta_next + META_SLACK` without ever reusing a position.

Such a mount writes the superblock with that `meta_next` before its first meta record. Otherwise a cut between that
record and the next superblock write would leave a record past the bound.

The next such mount would then reuse the position of that record. A mount that parses the meta tail takes the larger
of what it found and `meta_next`.

A reader finds slot B through the segment size. A reader without one (slot A torn, no config) tries the powers of two
from 2 KB to 64 MB.

## 4. Segments and flushes

A segment holds one session's data and consists of **flushes**. A flush is what the writer puts on the medium in one
go, triggered by `flush_bytes`, `flush_interval`, `sync()`, or right after a record of an `Immediate` track.

Every flush starts at a block boundary and carries its own head. It ends with padding to the end of its last block,
unless records fill it exactly.

Any flush therefore decodes without its neighbours. So does any block run cut at flush boundaries, such as a split
file.

The first flush of a segment starts with the **segment head**:

```text
u8    kind = 2 (a segment starts here)
u64   seq
u32   session
u32   flags: bit 0 FRESH (below), other bits 0
uvar  tracks: data tracks in the table (numbers 1..=tracks), at most 65535
       tracks × track entry (§5)
```

**`FRESH`** (flag bit 0) marks a session's first data segment when its mount didn't recover positions. That is the
case in a build without the Cargo feature `tail-positions`, which doesn't parse the tail.

In a `FRESH` segment every track counts its positions from 0, the system track included. Every data track's stream
starts there: `stream_start` is this seq, and the stream session has bit 31 set (§5).

The session's `SessionStart` has flag bit 1 (§7). The system track has no track table entry, so `FRESH` is where its
positions restart; otherwise they continue for the log's life.

The writer never sets `FRESH` on meta segments. A reader ignores flag bits it doesn't know, so a flag bit may only add
information, never change how the rest of the segment reads.

Every flush then has a **flush head**: the positions the flush's records are delta-coded against.

```text
u8    kind = 1 (only when the flush doesn't start a segment)
u8    time length (0: no time) + the device time last seen, a ww_si::Second in its ShrinkWrap encoding
uvar  full_back: 0 = a full head; otherwise its full head is that many blocks before this flush's first block
uvar  entries
       entries × (uvar gap, uvar next): track numbers in increasing order, each as the gap to the one before
       (track - previous - 1; the first one as its number), and the track's next position
```

A **full** head lists every track, track 0 included. A **sparse** head lists only the tracks that got records since
its full head.

A track that a sparse head leaves out is where the full head put it. Any flush therefore decodes with two heads, and a
few busy tracks among many quiet ones cost only their own entries.

The first flush of a segment always has a full head. Its `full_back` points to block 0: that flush, right after the
track table.

The writer writes a full head instead of a sparse one in two cases:

- The sparse heads since the last full one would list, all together, as many entries as a full head has. Sparse
  heads therefore never cost more than the full ones.
- At the latest, the writer writes one after 32 times a full head's size of data (`FULL_EVERY`).

`uvar` is LEB128 (7 bits per byte, low bits first, at most 10 bytes); `ivar` is zigzag then `uvar`.

Every boot writes into a **new data segment**, because a segment belongs to one session and holds its track table in
its head. The rest of the segment the device wrote before stays as it is.

The writer appends to the meta ring at the end the mount found. When a torn block is in the way or the medium needs
erasing, the next meta record opens the next meta segment instead.

## 5. Track table

A track is a stream of a `#[ww_trait]` in the session's API bundle. In a session it has a small number; 0 is the
system track and is never in the table.

The table holds one entry per data track, a `uvar` length and then these fields:

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

This reader ignores fields that a later version appends inside the entry.

### No strings in a track's identity

The device has numbers at hand, not names. A track is its path of ids and its item type's signature and global id.

A PC builds the names (`can[1].rx`, `ww_can_bus::Frame` 0.3) from the API bundle stored with the
session, in the meta `Bundle` record (§7). The path leads to the stream in the bundle, and the stream to its item
type with crate, version and type path.

Exporters recognise kinds of data by the signature and global id, never by the path. Examples are a CAN frame of `ww_can_bus` and a defmt frame of `ww_log`.

What a position counts (bytes, frames, samples) follows from the item type in the bundle.

### Stable key

The **stable key** names the same track across sessions, across firmware updates that keep its path, and across
split files. The key comes from numbers only.

The key starts as `h = FNV-1a(api_root as u64 LE)`, where LE is the byte order. FNV-1a then goes on over every
path id as a little-endian u32.

`api_root` names the device API across firmware updates. It is the FNV-1a of the API root crate's name and its major
version (u32, little endian).

That name and version are the `FullVersion` that wire_weaver generates as `<ROOT_TRAIT>_FULL_GID` for the API.

Since `logring::api_root_id` is a `const fn`, firmware computes `api_root` at build time. The crate name therefore
never reaches the card through a track.

FNV-1a, 64 bit: `h = 0xcbf29ce484222325; for each byte: h ^= byte; h *= 0x100000001b3`.

### Positions across sessions

A track's positions are monotonic from its `stream_start` segment on, across sessions. A mount continues them by key
from the tail segment's table.

A track that the tail segment doesn't name starts at 0 in the new session's segment, which becomes its
`stream_start` segment. A stream reset (`Logring::reset_track`) closes the data segment and opens the next one at once.

The head of that next segment carries the new `stream_session` and `stream_start`.

**Stream sessions with bit 31** (`STORE_SESSION`) are reserved for streams the logring starts by itself. A mount that
doesn't recover positions starts every data track's stream at 0 in session `0x8000_0000 | session`.

That `session` is the logring session of that boot, unique for the card's life.

Hosts and firmware choose stream sessions with bit 31 clear (`ww_stream`'s `StoreState::session`), and
`Logring::reset_track` refuses others. Such a session therefore never matches one a host remembers, and a host never
resumes across positions that restarted.

## 6. Records

A record is a track number, the event's `StreamHead` and the item's bytes, of any ShrinkWrap type. The head is what
`ww_stream` sends on the wire: position, units, lost, device time and rare markers.

The head is delta-coded against the track's previous record in the same flush, so a small item on a busy track costs
about 4 bytes.

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

These records store the head in full:

- the first record of a track in a flush, if it carries a time;
- a record whose time can't be a delta (another prefix or kind);
- a record with `ext` (a clock epoch marker).

A record may span blocks but never segments. A record that doesn't fit the rest of the segment closes it with padding
and starts the next one.

The item is at most `MAX_ITEM` (1 MB) and has to fit a segment after its head and the track table.

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

The flags of `SessionStart`:

- Bit 0, *formatted*: a new log was started.
- Bit 1, *positions not recovered*: the mount didn't parse the tail. Every track, the system track included, starts
  at 0 in the session's first segment, and `torn_records` 0 then means "not counted", not "none".
- Other bits are written 0, and a reader ignores bits it doesn't know.

**The meta area** uses the same segments, flushes and records: track 0 only, with positions counting meta records.
Every meta record is its own flush, written at once.

Meta positions increase strictly over the whole meta ring and may skip, by up to `META_SLACK` per mount (§3). A reader
counts them and never searches for them.

The meta area holds these records:

| kind | record | fields |
|---|---|---|
| 1 | Session | session u32, api_root u64 (§5), api (uvar length + the API root's `FullVersion` in its ShrinkWrap encoding, at most 128 bytes), device_id, fw, hw, bundle_hash u64, tracks uvar, then `tracks` track entries (§5, `released` 0) |
| 2 | Bundle | hash u64, len u32, then the bundle's bytes (the `ww_self` ApiBundle); stored once per hash while its record is still in the meta ring |
| 3 | Recording | id u32, stopped u8, why u8 (trigger at a start, reason at a stop), session u32, data seq u64, time (length byte + Second) |

The session's `api` is the one place that stores the API root's crate name and version as text. The firmware has them
as a constant anyway.

They are a fallback for a host that doesn't have the bundle with `bundle_hash` at hand. Such a host can still say
which API and version wrote the session, and look its snapshot up.

Tracks and records name nothing by string.

The writer reuses a meta segment when the ring goes round, and `meta_reused` counts it. The data segments still carry
their own track tables, so data stays decodable without the meta area.

The bundle is the exception. The writer stores it again when the superblock no longer points at a record inside the
ring.

## 8. Mount and recovery

The state of the log is in the segments, and the superblock only says where to look. A mount reads a bounded number of
blocks, whatever happened before (asserted in `tests/writer.rs`, `mount_reads_are_bounded`).

1. The mount reads both superblock slots (2 reads) and formats a new log if neither is valid. The geometry has to
   match the config.
2. For the meta ring, then the data ring, the mount finds the **tail segment**:
    - It reads the hinted seq's block 0, or else the block 0 of the seq before it.
    - It then goes forward while the next seq's block 0 checks out: normally at most as many segments as the
      superblock interval.
    - It counts a torn block 0, and the segment before it is the tail.
3. The mount finds the **end** of the tail segment:
    - A binary search finds the last valid block, since the written blocks form a prefix of the segment.
    - Four probes past it follow, because a block the card damaged inside the prefix would stop the search early.
    - The mount counts the torn blocks it sees.
4. The mount reads the **last flush** and continues every track's position from it (below).
5. The mount starts the new session, numbered `next_session`, and writes:
    - the meta records: the bundle if it isn't in the ring, and the session with its track table;
    - the superblock;
    - a new data segment with its segment head and a `SessionStart` record that carries the recovery counts.

The mount syncs everything before it returns.

### Positions from the last flush

The last block's `flush_off` says where the last flush starts. Its head, and its full head when the head is sparse,
give every track's position at its start.

The segment head gives the track table and `released` per track. The mount parses the flush's records to the end and
counts a record cut short by the end as torn.

The mount matches tracks to the new session's tracks by key: their `next` continues, and new keys start at zero. The
meta ring's next position is the larger of its last flush's and the superblock's `meta_next`.

Malformed content never fails a mount. What was parsed counts, and the mount reports the rest as torn.

### What a power cut costs

A power cut loses the records in the stage buffer since the last completed write. That is at most `flush_bytes`, or
`flush_interval` of data, or the stage buffer's size when that is smaller.

It may also lose the record being written when the cut hit, which then is torn. Nothing written before is lost:
blocks are never rewritten, and the writer never touches the filesystem while logging.

A torn block is skipped and counted. `Logring::durable(track)` says up to which position a track's records are on
the medium.

### Segment seqs on NOR flash

On NOR flash the writer erases a new segment when it opens it. A cut during the erase leaves garbage that fails the
block 0 check.

The next mount therefore takes the segment before it as the tail and reuses the slot.

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

**What a track costs.** In RAM, a track costs about 150 bytes per `TRACKS` slot in the writer, for its state,
positions and codec, as `tests/many_tracks.rs` asserts. Its `TrackDesc` adds 80 bytes and the path's ids, and it can
be a `static` in flash.

A device picks `TRACKS` for what it logs. Nothing else in the writer, the mount or the indexed reader grows with it.

The writer and the reader handle flush heads entry by entry, and `AllTracks<N>` decodes tracks below its own `N`.

On the medium, a track costs:

- its track entry in every segment head: about 35 bytes with a short path (1024 tracks take about 36 KB of a 4 MB
  segment);
- its track entry in the meta session record;
- an entry in every full flush head (2 to 4 bytes), and in the sparse heads after it was written;
- per record, one byte more for track numbers 7 to 134, and two from 135.

With 1024 tracks of which three are busy, the busy data takes about 7 % more room than with the three alone. Nearly
all of it is the segment heads' tables (`a_thousand_tracks`).

## 10. On an SD card: the ring file

On a card the medium is the file `logring.wlr` in the root directory of the card's exFAT volume. The module
`logring::exfat` handles it, behind the Cargo feature `exfat`.

The device writes the file by raw sector address, past the filesystem. It therefore has to know that the file is
still where it put it:

```text
MBR | gap to the first boundary unit | exFAT boot regions | FAT | cluster heap:
    bitmap, up-case table, root dir, README.txt, ... rw files (rw_bytes free, 128 MB default) ... logring.wlr
```

### Placement

At format and at re-create, the device places the ring file this way:

- The ring file is contiguous (`NoFatChain`) and leaves the FAT untouched.
- It starts on the card's allocation unit and on a cluster, and it is whole segments long.
- It sits at the end of the longest free run of clusters, with `rw_bytes` of all free space left free.
- Its attributes are read-only + system + hidden (+ archive).

### Segment size

The segment size comes from the card's SD Status (ACMD13):

- It is the largest of `AU_SIZE`, `UHS_AU_SIZE` and `VSC_AU_SIZE` (up to 64 MB), and at least 4 MB.
- When the AU is smaller than 4 MB, the segment size is a multiple of the AU.
- It is 4 MB when the card reports none.

The exFAT boundary unit is the AU when the AU is a power of two larger than the SD Association default for the
capacity.

### Binding

At format the superblock's `binding` gets the volume serial, the file's first cluster, the cluster count and the start
LBA. A mount (`exfat::open`) checks, in order:

1. The volume mounts (boot checksum and fields).
2. `logring.wlr` exists, is a file, `NoFatChain`, of whole sectors, with valid data length = data length, and every
   cluster set in the bitmap.
3. Slot A or B decodes. When A doesn't, the mount searches slot B at half of every AU size from 16 KB to 64 MB.
4. The binding matches. A different serial means *other card*, a different first cluster or start block means *moved*,
   and a different cluster count means *resized*.
5. The superblock's segments fit the file.

Anything else leaves the ring *unbound*. Nothing is written until the device re-creates the ring file (a new log; the
rw files stay) or formats the card.

Lost attributes are only a warning, since the mount checks the place anyway. The device can restore them.

### Card check

Before a format, the device runs a card check, which writes to the card. The check has two parts:

1. An aliasing test writes a tag at sector 0, then at every power of two, at 64 spread points and at the last sector.
   It reads each back with sector 0 after it, then reads all of them again.
2. A write-size sweep writes sizes from 16 KB to 1 MB in the second half of the card. It writes 4 MB at each size,
   each from its own AU.

A fake card wraps or drops writes, and the aliasing test fails it at its real size.

The smallest size within 90 % of the fastest becomes `flush_bytes`, rounded to 16 KB. The device stores it in the
superblock's `flush_bytes`.

When nothing was measured, `flush_bytes` is 64 KB. The sweep does not measure sizes larger than the scratch buffer the
device gives.

## 11. Reading

`logring::reader` (no_std, no alloc) reads a ring without a scan, from what the format above already carries.

### By track position

The reader binary-searches the data segments of one stream of the track, by the track's `next` in each segment's
first flush head. The stream is the one current at the segment asked for, normally its `stream_start`.

A segment whose entry names a later `stream_start` is past the stream, and a walk stops there. Positions restart in
every stream.

The reader then searches the segment's blocks. A block's `flush_off` gives the start of its flush.

The flush head gives the track's `next` there, or its full head when the track isn't in a sparse one. The reader scans
the last flush that starts at or before the position.

The seek takes about 2 log₂ n + 2 log₂ blocks reads. The reader steps over other tracks' records by their lengths,
without their delta state.

### The system track by position

The same search works for the system track, from the segment asked for up to the next `FRESH` segment. There the
system track's positions restart.

The reader finds that `FRESH` segment by reading the segment heads after the segment asked for, one block each.

### By time

The same search runs over a session's consecutive segments and their flushes, by the device time in the flush heads.
That time is the time last seen before the flush, so the flush found starts at or before the time asked for.

The reader compares times as numbers: a clock that wraps inside a session breaks the order.

### By recording

The `Recording` meta records give a recording's data segment. Its records are those between its `RecordingStart` and
`RecordingStop` in track 0.

### Segment summaries

A segment summary holds per track the count, bytes, first and last position and time, plus the markers and flushes.
The reader computes it from one segment's records; it is not written into the segment.

### What a track store serves

`TrackStore` (one track as `ww_stream`'s `StreamStore`) reads back only what is on the medium. If it served an event
from the stage buffer, a power cut could lose that event while a host already has it.

The positions after the reboot would then go to other events.

## 12. Not in this version

This version has none of these:

- per-track compression (the `compression` byte is reserved);
- pinned recordings or a keep ring;
- several sources per card (`source` is reserved);
- encryption;
- segment summaries written at a segment's end (the reader computes them).
