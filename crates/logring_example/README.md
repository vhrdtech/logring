# logring_example

This crate is the running example of logring's book: the firmware side of the drive, a motor controller board. The
drive keeps the last days of four tracks on a Secure Digital (SD) card. They hold its log lines, its mode, its battery
voltage and the frames of its Controller Area Network (CAN) bus.

The crate is `no_std` and allocates nothing. It runs on a simulated card in RAM (`SimCard`). The same code therefore
builds for a personal computer (PC) and for a microcontroller (MCU) such as the RP2350 (`thumbv8m.main-none-eabihf`):

- `api`: the drive's WireWeaver API; each of its four streams is a track of the ring.
- `drive`: `Drive`, the logger a firmware keeps: mount, one call per track, the flush tick.
- `sd`: `mount_sd`, the ring as the file `logring.wlr` of the card's exFAT volume: mounted, re-created or formatted.
- `bench`: a stretch of the endurance bench at any length, the same items every time.
- `story` (feature `std`): the runs behind every number of the book's guide; `examples/story.rs` prints them.

The book's pages include marked sections of its code and tests, and `logring_figures` draws figures from its runs.

```sh
cargo nextest run -p logring_example
cargo run -p logring_example --features std --example story
cargo build -p logring_example --target thumbv8m.main-none-eabihf
```
