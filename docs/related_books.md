# WireWeaver and ww_tape

Two other books cover the projects that logring is used with. Each book stands on its own, so this page says what the
other two are about and where they connect to logring.

## WireWeaver: typed APIs between devices and hosts

[WireWeaver](https://ww.vhrd.tech/) is a framework for APIs between microcontrollers and host programs. An API is
written once as a Rust trait, and WireWeaver generates the device side and the host side from it. The messages travel
in shrink_wrap, a dense binary format that needs no allocator on the device.

logring builds on three parts of WireWeaver:

- **shrink_wrap.** Every item in a logring is a shrink_wrap value, so a device logs the same types it sends over its
  API.
- **API bundles.** A bundle is the compiled description of an API: its types, its resources and their names. logring
  stores the device's bundle in the ring, so a PC can name every track and decode every item without the firmware.
- **Streams and media.** WireWeaver's `ww_stream` crate defines the `LogMedium` trait that logring writes to. A
  WireWeaver stream can also be served from a logring, so a host that reconnects receives what it missed from the
  ring.

The `ww` command line tool includes logring's commands as `ww logring`, and the `wire_weaver` Python package includes
its Python API as `wire_weaver.logring`.

## ww_tape: recordings on a host

[ww_tape](https://vhrdtech.github.io/ww_tape/) is the recording format of WireWeaver. A **tape** holds tracks of typed
items over time, like a logring, but it is built for a host: it grows by appending and never drops data by itself.
Its index, summaries and column-coded chunks let a reader reach one track out of thousands, or an overview of a week,
in a few reads.

A system often uses both: a logger keeps its recent history in a logring, and a host turns what it reads into tapes for
analysis and archiving. [Tapes and logring](tapes_and_logring.md) compares the two in detail.
