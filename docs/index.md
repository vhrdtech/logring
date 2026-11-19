# logring

logring is a log store for embedded devices. Firmware writes many streams of typed items onto a Secure Digital (SD)
card or a flash chip.

A power cut at any moment costs at most the last moments of data.

A host reads the log back over WireWeaver, as a USB disk, or from a card image with the `logring` tool. The tool
exports CSV, MCAP (a container format for robotics logs) or MF4 (Measurement Data Format version 4).

This book explains what logring keeps, how it survives a power cut, and how to read what a device recorded. Its readers
are firmware engineers who add logging to a device and anyone who reads its logs on a personal computer (PC).

## Why a ring

A device in the field may log for months without a host attached. Its storage has a fixed size, so sooner or later it
fills up.

logring keeps a **ring**: a region of fixed size, chosen when the card is formatted. Once the region is full, the newest
data overwrites the oldest.

The device therefore always holds its most recent hours or days. These usually matter most when someone asks what
happened.

For example, a data logger on a vehicle logs its Controller Area Network (CAN) bus traffic, its battery voltage and
its error messages. After a fault, a technician takes out the SD card and finds the last few days of all three,
but nothing older.

## What it keeps

A **track** is a stream of items of one type, such as the frames of one CAN bus or the readings of one sensor. An item
is any [shrink_wrap](https://ww.vhrd.tech/serdes/shrink_wrap/) type: the same Rust types a WireWeaver API sends over
its link.

Each item carries the device's own time, so the device needs no wall clock or real-time clock.

Around the tracks, logring keeps the structure that makes a long log readable:

- A **session** is one run of the device, from a mount to a power cut or a restart.
- A **recording** is a stretch of a session that the firmware starts and stops, such as one test drive.
- A **marker** is a labelled point in time with an optional value, such as the moment a button was pressed.
- The **API bundle** of the device, stored in the ring's meta records, names the tracks and their types. A reader on a
  PC decodes every item from it without the firmware's source code.

## Why it survives a power cut

The firmware stages new items in RAM and writes them out in a **flush** of whole 512-byte blocks. Each block ends with
a trailer that holds a CRC-32C checksum.

The writer writes a block once per lap of the ring and never rewrites it in place.

After a power cut, the next mount finds the last complete flush from the blocks' checksums. It counts any torn block
and carries on after the last complete flush.

A power cut loses what the stage buffer held since the last flush: by default at most 64 KB or one second of data.

logring needs no heap: the engine is `no_std` without `alloc`, and the firmware lends it its buffers. It works on any
medium behind WireWeaver's `LogMedium` trait.

On an SD card the ring is `logring.wlr`, a preallocated file on an exFAT volume. It sits beside a small region of
ordinary files that a PC reads and writes as usual.

## Reading a log on a PC

The `logring` command line tool and the Python package read a mounted card, a card image, a ring file or a split file.
To see what a card image holds and export it as CSV, run:

```sh
logring info card.img
logring export card.img --format csv --out ./out
```

`logring info` lists the sessions, recordings, tracks and counters. The same commands are part of WireWeaver's `ww`
tool as `ww logring`.

In Python, a track becomes a pandas data frame:

```python
import logring

card = logring.open("card.img")
df = card.track("can1_rx").to_pandas()
```

## Where to go next

- [WireWeaver and ww_tape](related_books.md) introduces the two projects logring works with.
- [Tapes and logring](tapes_and_logring.md) compares a logring with a tape, the recording format of ww_tape, and says
  when to use which.
- [The logring format](format.md) is the reference of the bytes on the medium, block by block.
