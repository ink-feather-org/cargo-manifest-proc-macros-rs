//! This benchmark tests the performance of looking up a batch of crate names in parallel in the Cargo.toml file.
#![cfg_attr(feature = "nightly", feature(test))]

use cargo_manifest_proc_macros::CargoManifest;

#[path = "../src/shared_testing_benchmarking.rs"]
mod shared;

#[cfg(all(feature = "nightly", not(feature = "proc-macro")))]
mod benches {
  use core::hint::black_box;
  extern crate test;
  use super::{
    shared::{setup_bench, BENCHMARK_CRATE_NAME_LOOKUP_BATCH_SIZE},
    *,
  };
  use rand::prelude::IndexedRandom;
  use rayon::iter::{ParallelBridge, ParallelIterator};
  use test::Bencher;

  #[bench]
  fn multi_threaded_random_lookups(bench: &mut Bencher) {
    let tmp_dir = tempfile::tempdir().unwrap();

    let possible_crate_names = setup_bench(&tmp_dir);

    bench.iter(|| {
      (0..BENCHMARK_CRATE_NAME_LOOKUP_BATCH_SIZE)
        .par_bridge()
        .for_each(|_| {
          let mut rng = rand::rng();
          let possible_crate_name = possible_crate_names.choose(&mut rng).unwrap();
          let _ =
            black_box(CargoManifest::shared().try_resolve_crate_path(possible_crate_name, &[]));
        });
    });
  }
}
