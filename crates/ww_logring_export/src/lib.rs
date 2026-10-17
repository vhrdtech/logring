//! PC-side reading and exporting of `ww_logring` cards, images and split files.
//!
//! Opens a mounted card, a ring file copy, a raw image or a split `.wlr` file through `ww_logring` (`std`
//! feature), decodes tracks dynamically from the stored API bundle, and exports CSV, MCAP and MF4. See
//! `AGENTS.md` and `FEATURES.md`; export logic lands in R10 (P2628#16oct-1001h).
