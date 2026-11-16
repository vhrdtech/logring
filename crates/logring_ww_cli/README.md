# logring_ww_cli

`ww logring`: the commands of the `logring` tool (`info`, `export`, `cat`, `check`, `make-image`) as a command of
the `ww` command line. Linking this crate into a binary built on `wire_weaver_cli` registers them with its command
registry; the `ww` binary of `ww_tools` does, so `ww logring ...` works with no plugin on `PATH`.

The commands themselves live in `logring_cli`'s library, which the standalone `logring` binary runs too.
