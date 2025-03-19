#![doc = include_str!("../README.md")]
#![cfg_attr(
  all(feature = "nightly", feature = "proc-macro"),
  feature(proc_macro_tracked_env)
)]
#![cfg_attr(all(feature = "nightly", feature = "proc-macro"), feature(track_path))]
#![cfg_attr(feature = "nightly", feature(test))]

extern crate alloc;
#[cfg(all(feature = "nightly", feature = "proc-macro"))]
extern crate proc_macro;

mod cargo_manifest;

/// Utilities for working with `syn` types.
pub mod syn_utils;

pub use cargo_manifest::*;
