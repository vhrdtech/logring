---
level: advanced
---

# Reading over WireWeaver and USB

After this chapter you can read a live device's ring over its API, and as files on a USB disk. The drive is still
running: an engineer wants to watch `can0` right now, not after the next power cut.

## Reading over WireWeaver

`` [`TrackStore`] `` turns one track of a mounted ring into a WireWeaver stream. It answers the same `push`, `read`
and `release` calls any stream server makes on any medium. A host that opens the drive's `can0` stream gets every
[[record]] the track holds, the way a card image does. The difference: the records keep arriving.

```rs
--8<-- "crates/logring_example/tests/it/live_reading.rs:live_read"
```

A `` [`TrackStore`] `` read only returns what already reached the card. The test pushes a second frame without
syncing: a read at its position comes back empty, because the frame still sits in the writer's `` [`Buffers`] ``
stage. Once the next sync writes it, the same read returns it. A host watching live therefore lags the push rate
by however long a frame waits in the stage buffer.

`ww_stream`'s resend buffer wraps the same store for a host that reboots, or loses the link for a while. It resumes
exactly where it left off: everything that reached the card since, once, in order, across [[sessions]] and power
cuts. That resend logic is `ww_stream`'s own job, covered in the WireWeaver book. This book's part of it stops at
`` [`TrackStore`] ``, which turns a track into a store.

## Reading as files on a USB disk

The same ring is meant to show up as ordinary files, too. A read-only tree with one split file per [[recording]]
and a plain index would let a bare laptop read the files directly. It could pull a day's `log` or `can0` straight
off a USB cable.

A passthrough logical unit would carry the card's own exFAT volume alongside it. Its small region of read-write
files would stay an ordinary Secure Digital (SD) card to the host.

This view is designed, not built: today's reading on a personal computer (PC) goes through a card image, as the
previous chapter showed.
[Tapes and logring](tapes_and_logring.md) comes next: when the device's own reach is enough, and when a host
recording is the better fit.
