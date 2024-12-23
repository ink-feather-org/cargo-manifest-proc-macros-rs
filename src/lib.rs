#![doc = include_str!("../README.md")]
#![feature(map_try_insert)]
#![feature(proc_macro_tracked_env)]
#![feature(track_path)]

extern crate alloc;
extern crate proc_macro;

mod cargo_manifest;

/// Utilities for working with `syn` types.
pub mod syn_utils;

pub use cargo_manifest::*;
