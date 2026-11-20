# logring_example

The running example of logring's book: the firmware side of the drive, a motor controller board that keeps the
last days of its log lines, its mode, its CAN frames and its battery voltage on an SD card.

The crate is `no_std` and allocates nothing. It runs on a simulated card in RAM (`SimCard`), so the same code
builds for a PC and for an MCU such as the RP2350 (`thumbv8m.main-none-eabihf`):

- `api`: the drive's WireWeaver API; each of its four streams is a track of the ring.
- `drive`: `Drive`, the logger a firmware keeps: mount, one call per track, the flush tick.
- `bench`: a stretch of the endurance bench at any length, the same items every time.

The book's pages include marked sections of its code and tests, and `logring_figures` draws figures from its runs.

```sh
cargo nextest run -p logring_example
cargo build -p logring_example --target thumbv8m.main-none-eabihf
```
