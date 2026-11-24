---
level: basic
---

# Tapes and logring

A **logring** is a log store that keeps the newest data on a device's own storage. A **tape** is the recording format
of [ww_tape](https://tape.vhrd.tech/), and it keeps a whole recording, mostly on a host.

Both store tracks of WireWeaver's shrink_wrap items with times, so a reader new to both may wonder which to pick. This
page compares them on the points that decide the choice and says when to use which.

The ww_tape book explains the terms of tapes in [Tape concepts](https://tape.vhrd.tech/concepts/).

## What each is for

A logring runs on the device. It is a power-cut-safe ring on an SD card or a flash chip, and firmware
with no heap writes it (`no_std`, no `alloc`).

The ring has a fixed size, chosen when the card is formatted. When the ring is full, the newest data overwrites the
oldest.

A device that stays in the field for months therefore always holds its most recent hours or days.

A host reads the ring over WireWeaver, as a USB disk, or from a card image with the `logring` tool. The tool exports
CSV, MCAP or MF4.

A tape runs mostly on the host. It is an append-only recording that never drops data by itself: a capture of a link, a
test run, an analysis result.

A tape file grows until the producer stops or the writer cuts a piece. The tape's index, summaries and column-coded
chunks let a reader reach one track among thousands, or a week's overview, in a few reads.

<figure class="ww-figure" markdown="0">
--8<-- "docs/figures/logring.svg"
<figcaption>Two ways to keep data. A logring is a fixed ring on the device: its newest segment overwrites the oldest.
A tape grows by appending pieces, and a piece goes only when someone deletes it.</figcaption>
</figure>

## Comparison

| | logring | tape (`ww_tape`) |
|---|---|---|
| Where it is written | firmware on a microcontroller: a preallocated ring file on an SD card's exFAT volume, written by raw sector address, or another medium behind its `LogMedium` trait (NOR flash is simulated in its tests) | a host program into a file; also a live feed or a raw medium |
| Size | a fixed ring; when full, the oldest segment is overwritten (the default) or new records are refused | grows by appending; nothing is dropped unless a person deletes whole pieces |
| Power loss | loses what the stage buffer held since the last flush: at most 64 KB (or the size measured when the card is formatted) or 1 s of data by default | loses what wasn't synced yet: about 1 s by default; a reader rescans at most about 4 MiB after the newest checkpoint |
| Wear and erase | 512-byte blocks written once per lap of the ring; segments match the card's allocation unit | blocks are appended and never rewritten; a raw NAND writer pads a page at each sync; erase and wear are left to the medium |
| Unit of writing | a flush: whole 512-byte blocks, each with an 8-byte trailer | a block of any length: 32-byte header, body, CRC-32C |
| Per-record overhead | a record head delta-coded per flush: about 4 bytes for a small item on a busy track | records blocks: track, time delta and length as varints; chunks: columns per track, 0.022 to 3.84 bytes per event start ([Numbers](https://tape.vhrd.tech/numbers/#event-times) in the ww_tape book) |
| Time | the device's own time as a `ww_si::Second`; no wall clock or real-time clock needed; a clock that wraps inside a session breaks reads by time | ticks of defined clocks with exact rates and origins; clock maps relate them, including to wall time |
| Finding data | from what the ring's segments carry, without a scan | an index written every 4 MiB and at the finish, per-track directories, summary trees written with the data |
| Large tracks | records one after another | column-coded chunks read in place, sample codecs, summaries |
| What a reader needs | the format, and the API bundle stored in the ring's meta records to name tracks and types | the format; track 0 carries every definition and the API bundles, near the data that uses them |
| Integrity | CRC-32C per 512-byte block | CRC-32C per block and per 64 KiB of a chunk; a BLAKE3 hash of every finished piece |

## When to use which

Use **logring** when the device itself must keep data:

- The device logs for months with no host attached, and the newest data matters most.
- Storage is an SD card or flash chip that the firmware writes directly, and power can fail at any moment.
- The firmware has no heap and little RAM.

Use a **tape** when a host records or keeps data:

- A host captures a link, a test bench or a simulation, and nothing may be dropped.
- Readers need to reach one track, one time range or an overview fast, from a file or over HTTP.
- Data from several devices and clocks goes into one recording, on a common time base.
- High-rate series need compact columns and summaries.

A system often uses both. A logger keeps its recent history in a logring, and the host turns what it reads into tapes
for analysis and archiving.

An importer that does this does not exist yet.

## What the two formats share

The formats are separate, and a logring block is not a tape block. They share a few small conventions:

- **Varints.** Both code unsigned numbers as LEB128 (7 bits per byte, low bits first) and signed numbers zigzag first.
- **Checksums.** Both use CRC-32C, over different spans.
- **Track keys.** Both build stable track keys with 64-bit FNV-1a, but over different inputs.
    - logring hashes the API root (the crate's name and major version) and the path's numeric ids.
    - `ww_tape` hashes the crate's name and the path's names, indices and keys, so its key survives renumbered ids.

The same track therefore has different keys in the two formats.

Everything else differs: blocks, record heads, what track 0 means, how time is stored, and how a reader finds data.
