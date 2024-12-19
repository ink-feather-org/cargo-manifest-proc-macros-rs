#![doc = include_str!("../README.md")]
#![feature(map_try_insert)]

mod cargo_manifest;

/// Utilities for working with `syn` types.
pub mod syn_utils;

pub use cargo_manifest::*;
