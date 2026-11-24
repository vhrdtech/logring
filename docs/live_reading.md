---
level: advanced
---

# Reading over WireWeaver and USB

The drive is still running on the bench, pushing `can0` as fast as the bus delivers it. An engineer wants to
watch it right now, not wait for the next power cut or carry the card to a desk. How does a live device's ring
reach a host, over its WireWeaver API, or as files on a USB disk?

## Reading over WireWeaver

`` [`TrackStore`] `` turns one track of a mounted ring into a WireWeaver stream. It answers the same `push`, `read`
and `release` calls any stream server makes on any medium. A host that opens the drive's `can0` stream gets every
[[record]] the track holds, the way a card image does, but new records keep arriving.

```rs
--8<-- "crates/logring_example/tests/it/live_reading.rs:live_read"
```

A `` [`TrackStore`] `` read only returns what already reached the card. The test pushes a second frame without
syncing: a read at its position comes back empty, because the frame still sits in the writer's `` [`Buffers`] ``
stage. Once the next sync writes it, the same read returns it.

`` [`Logring::durable`] `` moves forward at every write of the stage buffer to the card, not only when a [[flush]]
ends. A host watching live therefore lags the push rate by about one stage write, not a whole flush interval.

The drive's stage buffer is 4 KiB, a quarter of its 16 KiB flush. A frame reaches the card, and a live reader,
well before its flush closes, not after a full second.

`ww_stream`'s resend buffer wraps the same store for a host that reboots, or loses the link for a while. It resumes
exactly where it left off: everything that reached the card since, once, in order, across [[sessions]] and power
cuts. That resend logic is `ww_stream`'s own job, covered in the WireWeaver book. This book's part of it stops at
`` [`TrackStore`] ``, which turns a track into a store.

## Reading as files on a USB disk

The same ring is meant to show up as ordinary files, too. A read-only tree with one split file per [[recording]]
and a plain index would let a bare laptop read the files directly. It could pull a day's `log` or `can0` straight
off a USB cable.

A passthrough logical unit would carry the card's own exFAT volume alongside it. Its small region of read-write
files would stay an ordinary SD card to the host.

This view is designed, not built. Today's reading on a PC still goes through [Reading a log
on a PC](reading.md)'s sources, never live.

[Tapes and logring](tapes_and_logring.md) comes next: when the device's own reach is enough, and when a host
recording is the better fit.
