#![doc = include_str!("../README.md")]
#![cfg_attr(feature = "nightly", feature(proc_macro_tracked_env))]
#![cfg_attr(feature = "nightly", feature(track_path))]

extern crate alloc;
#[cfg(feature = "nightly")]
extern crate proc_macro;

mod cargo_manifest;

/// Utilities for working with `syn` types.
pub mod syn_utils;

pub use cargo_manifest::*;
