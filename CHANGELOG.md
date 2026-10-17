# Changelog

## [Unreleased]

### Added

- `ww_logring`: the ring format, the writer and power-cut recovery (`Logring::mount`/`format`, `push`,
  `push_head`, `marker`, `recording_start`/`stop`, `sync`, `flush_if_due`, `durable`), with sessions, track
  tables, the system track and the meta area; `docs/format.md` describes the layout; a `std` sequential `scan`
  and a cargo-fuzz target for mount with seed images (LOG-1, P2628#16oct-1000).
- Repo scaffold: workspace (`ww_logring`, `ww_logring_export`, `ww_logring_cli` binary `logring`,
  `ww_logring_py`), `just test`/`lint`/`install`/`deploy`, CI (fmt, clippy, tests, typos, cargo sort), licence
  (MIT OR Apache-2.0) (P2628#16oct-1016).
- FEATURES.md with the planned items and ideas of `prj/P2628-logring/design/plan.md`: areas `LOG` (LOG-1..8),
  `VIEW` (VIEW-1..4), `PC` (PC-1, PC-2), `TEST` (TEST-22), each bound to its P2628 task where one exists
  (P2628#16oct-1016).
