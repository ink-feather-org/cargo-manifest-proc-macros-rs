//! This benchmark tests the performance of looking up a batch of crate names serially in the Cargo.toml file.
//! It does not use bencher so that it can be more easily profiled using flamegraph.
#![cfg_attr(feature = "nightly", feature(test))]

use cargo_manifest_proc_macros::CargoManifest;

#[path = "../src/shared_testing_benchmarking.rs"]
mod shared;

#[cfg(all(feature = "nightly", not(feature = "proc-macro")))]
mod benches {
  const BENCHMARK_FIXED_SINGLE_THREADED_CRATE_NAME_LOOKUP_COUNT: usize = 5_000_000;

  use super::{shared::setup_bench, *};
  use core::hint::black_box;
  use rand::prelude::IndexedRandom;

  #[test]
  pub fn fixed_single_threaded_random_lookups() {
    let tmp_dir = tempfile::tempdir().unwrap();

    let possible_crate_names = setup_bench(&tmp_dir);
    let mut rng = rand::rng();

    for _ in 0..BENCHMARK_FIXED_SINGLE_THREADED_CRATE_NAME_LOOKUP_COUNT {
      let possible_crate_name = possible_crate_names.choose(&mut rng).unwrap();
      let _ = black_box(CargoManifest::shared().try_resolve_crate_path(possible_crate_name, &[]));
    }
  }
}
