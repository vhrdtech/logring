# logring_wheel

The `logring` Python wheel: the extension module over `logring_py`, built by maturin from `pyproject.toml` here
(`uv run --group dev pytest` builds and tests it, `just test-py` does the same). It embeds the API snapshots of the
crates in the build's lock, so a recording's skipped standard types decode offline.

The same API is in the `wire_weaver` wheel as `wire_weaver.logring`.
