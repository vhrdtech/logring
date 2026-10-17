# Size examples

What `ww_logring` costs in flash and RAM on small MCUs. `just size` builds every bin here for every target at
opt-level `s` and `z` (fat LTO, one codegen unit, `panic = "abort"`) and prints Markdown tables:

- flash (text + rodata + data) and static RAM per bin and target, minus `baseline`;
- per bin on thumbv8m.main (Cortex-M33, RP2350): flash by crate, source file and function, from the DWARF line
  tables, so code that LTO inlined into a caller still counts for the crate and function it came from;
- per bin: RAM of logring's types and futures (`-Zprint-type-sizes`, nightly); the app's top future is what the
  executor keeps on its stack or in its task arena.

The bins are no_std firmware-like programs that use the device-side API on a dummy SD card whose registers are
read and written with volatile accesses, so nothing is optimised away. The writer has a 4 KB stage buffer and
8 tracks (`BUF`, `TRACKS` in `src/lib.rs`).

| bin | what it uses |
|---|---|
| `baseline` | the dummy card, the executor, the inputs: subtracted from the others |
| `raw_writer` | the writer on a raw medium (no exFAT): mount with recovery, push, markers, recordings, flushes |
| `raw_store` | `raw_writer` plus a reliable stream on a `TrackStore` (reads the ring back) |
| `exfat_writer` | the exFAT mount check binding `logring.wlr`, then `raw_writer`'s loop |
| `exfat_store` | `exfat_writer` plus a `TrackStore` |
| `exfat_recreate` | `exfat_store` plus re-creating the ring file on an unbound card and restoring its attributes |
| `full` | `exfat_recreate` plus the card check and format (`format_card`) |
| `reader` | the reader alone: open, recordings, a segment summary, seeks by track position and by time |

`report/` is the host tool behind `just size` (`size-report`, its own workspace): `just size matrix`,
`just size attribute --bin full`, `just size types --bin raw_writer`, `just size --help`.

Targets it needs:

```sh
rustup target add thumbv6m-none-eabi thumbv7em-none-eabihf thumbv8m.main-none-eabihf \
    riscv32imac-unknown-none-elf riscv32imc-unknown-none-elf
rustup +nightly target add thumbv8m.main-none-eabihf
```
