//! This benchmark tests the performance of looking up a single crate name in the Cargo.toml file.
#![cfg_attr(feature = "nightly", feature(test))]

use cargo_manifest_proc_macros::CargoManifest;

#[path = "../src/shared_testing_benchmarking.rs"]
mod shared;

#[cfg(all(feature = "nightly", not(feature = "proc-macro")))]
mod benches {
  use core::hint::black_box;
  extern crate test;
  use super::{shared::setup_bench, *};
  use rand::prelude::IndexedRandom;
  use test::Bencher;

  #[bench]
  fn single_item_lookup(bench: &mut Bencher) {
    let tmp_dir = tempfile::tempdir().unwrap();

    let possible_crate_names = setup_bench(&tmp_dir);
    let mut rng = rand::rng();

    bench.iter(|| {
      let possible_crate_name = possible_crate_names.choose(&mut rng).unwrap();
      let _ = black_box(CargoManifest::shared().try_resolve_crate_path(possible_crate_name, &[]));
    });
  }
}
