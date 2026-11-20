---
level: basic
---

# A first log

The [[drive]] is a motor controller board. It should keep the last days of its log lines, its mode, its bus frames
and its battery voltage on its Secure Digital (SD) card. This chapter answers what its firmware has to
do for that. The firmware describes the streams once and lends the logger its buffers. It then
[[mounts|mount]] the ring at boot, pushes each [[item]] as it happens and calls a tick now and then.

All code on this page is the drive's firmware from the crate `logring_example`. It runs on a card simulated in RAM, so
it runs on a personal computer (PC) as it is. The same code builds for the drive's microcontroller.

## The drive's streams

The drive already has a WireWeaver API, so its streams have names and types. Each stream becomes a [[track]] of the
ring, and each value a stream sends becomes an item of that track:

```rs
--8<-- "crates/logring_example/src/api.rs:api"
```

The item types are ordinary shrink_wrap types, the same ones the API sends over its link. A log line borrows its text,
and a frame of the Controller Area Network (CAN) bus borrows its data bytes. Nothing on the way to the card allocates.

## Describing the tracks

The firmware tells the ring which tracks it has in a [`TrackDesc`] per track. A track names its stream by its path in
the API: `UNib32(2)` is the third stream, `can0`. The path, not a string, identifies a track on the card, as [Tracks,
sessions and recordings](tracks.md) explains.

```rs
--8<-- "crates/logring_example/src/drive.rs:tracks"
```

The `rate` is a hint for a reader on a PC: about how many items per second the track carries. The `mode` track is
[[Immediate|durability]], so a change of mode is on the card before the call returns. [Flushes and power
cuts](flushes.md) says what that costs and when it pays.

The descriptions go into a [`SessionInfo`] together with the API's name and version and the drive's serial number,
firmware and hardware versions. [`drive::session`] builds it.

## Mounting at boot

The logger needs RAM, and the firmware lends it in a [`Buffers`]. Its two numbers are the size of the [[stage
buffer]], 4096 bytes, and the number of tracks, five: the drive's four and the [[system track]]. The firmware keeps
`Buffers` in a static or on its main task's stack, since logring has no heap.

```rs
--8<-- "crates/logring_example/src/drive.rs:mount"
```

[`Logring::new`] checks the [`Config`] against the card and the tracks against the buffers, and writes nothing. The
[[mount]] then finds where the log ended, recovers every track's [[position]] and starts a new [[session]]. On a new
card it starts a new log instead. The config's segment size is 64 KiB on the simulated card, a size that fits the
small card in RAM. [The SD card](sd_card.md) explains how a real card sets it.

## Pushing items

Each track gets one call in the drive's logger. The call encodes the item into a buffer of 128 bytes. It then pushes
the item to its track with the time it happened, in microseconds since boot:

```rs
--8<-- "crates/logring_example/src/drive.rs:push"
```

[`Logring::push`] turns the item into a [[record]]: the item's bytes with its track, its position and its time. The
record goes into the stage buffer, and the call returns at once. The card sees nothing yet.

## The tick

The stage buffer goes to the card in a [[flush]]. A flush starts once the buffer holds 16 KiB for the card, or once
the oldest record in it has waited a second. The first case needs no help, but the second needs a clock. The
firmware therefore calls a tick with its millisecond clock about every 100 ms:

```rs
--8<-- "crates/logring_example/src/drive.rs:tick"
```

<figure class="ww-figure" markdown="0">
--8<-- "docs/figures/first_log.svg"
<figcaption>The drive's calls fill the stage buffer in RAM, and a flush writes it to the card in whole blocks. The
mode track is Immediate: its record goes to the card at once.</figcaption>
</figure>

## The first minute

The bench run [`bench::run`] drives the logger as the drive does on its endurance bench. It boots and runs the motor
from the first second on. It records a frame a millisecond, a voltage a second and a log line every two seconds. This block
mounts a ring on a new simulated card and runs the first minute:

```rust
use logring::Buffers;
use logring::tests_support::block_on;
use logring_example::bench::{self, Bench};
use logring_example::card::SIM_CARD_BYTES;
use logring_example::{Drive, SimCard, drive};

let mut card = vec![0u8; SIM_CARD_BYTES];
let mut bufs = Buffers::new();
let totals = block_on(async {
    let mut drive = Drive::mount(SimCard::new(&mut card), &mut bufs, drive::config()).await?;
    bench::run(&mut drive, Bench::FIRST_MINUTE).await
})
.unwrap();
assert_eq!(totals.can0, 60_000);
assert_eq!(totals.vbat, 60);
```

The minute holds 60 000 frames, 60 voltages, 31 log lines and 3 changes of mode. The ring took them in 70 flushes,
which filled 17 segments of 64 KiB. They take 1032 KiB of the card. A frame's item is 13 bytes, and its record adds its track, position and time. The example's `story` prints these numbers (`cargo run -p logring_example --features
std --example story`).

The drive logs now, but a reader of its card still wants to know which boot and which test run a record belongs to.
[Tracks, sessions and recordings](tracks.md) is about that.
