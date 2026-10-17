//! `ww_logring`: a power-cut-safe ring log store for embedded devices.
//!
//! Many tracks of any [shrink_wrap](https://github.com/vhrdtech/wire_weaver) type are written at once into one
//! preallocated ring file, with sessions, recordings and markers, read back over `wire_weaver`, as a USB disk or
//! with the `logring` PC tool. See `AGENTS.md` and `FEATURES.md` for the design and what is implemented; the
//! format itself lands in R1 (P2628#16oct-1000).
#![cfg_attr(not(feature = "std"), no_std)]
