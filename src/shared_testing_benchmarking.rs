#![allow(dead_code)]

use super::CargoManifest;
use parking_lot::Mutex;
use rand::Rng;
use std::{
  io::Write,
  path::{Path, PathBuf},
  sync::LazyLock,
};
use tempfile::TempDir;

pub const BENCHMARK_CRATE_NAME_LOOKUP_BATCH_SIZE: usize = 50_000;

pub static SERIAL_TEST: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub fn create_src_lib(path: &Path) {
  let src_dir = path.join("src");
  std::fs::create_dir(&src_dir).unwrap();
  let lib_file = src_dir.join("lib.rs");
  let mut lib_file = std::fs::File::create_new(&lib_file).unwrap();
  lib_file.write_all(b"pub fn test() {}").unwrap();
  lib_file.flush().unwrap();
}

pub fn setup_fs_test(
  temp_dir: &TempDir,
  workspace_cargo_toml: Option<&str>,
  cargo_toml: &str,
) -> ((PathBuf, std::fs::File), Option<(PathBuf, std::fs::File)>) {
  // create a random subdir to use
  let subdir_name: String = rand::rng()
    .sample_iter(&rand::distr::Alphanumeric)
    .take(7)
    .map(char::from)
    .collect();
  let workspace_path = temp_dir.path().join(subdir_name);
  std::fs::create_dir(&workspace_path).unwrap();
  let crate_path = workspace_path.join("test-crate");
  std::fs::create_dir(&crate_path).unwrap();

  let crate_manifest_path = crate_path.join("Cargo.toml");
  let mut crate_manifest_file = std::fs::File::create_new(&crate_manifest_path).unwrap();
  crate_manifest_file
    .write_all(cargo_toml.as_bytes())
    .unwrap();
  crate_manifest_file.flush().unwrap();
  create_src_lib(&crate_path);

  let mut workspace_return = None;
  let workspace_manifest_path = workspace_path.join("Cargo.toml");
  if let Some(workspace_cargo_toml) = workspace_cargo_toml {
    let mut workspace_manifest_file = std::fs::File::create_new(&workspace_manifest_path).unwrap();
    workspace_manifest_file
      .write_all(workspace_cargo_toml.as_bytes())
      .unwrap();
    workspace_manifest_file.flush().unwrap();
    create_src_lib(&workspace_path);
    workspace_return = Some((workspace_manifest_path, workspace_manifest_file));
  }

  ((crate_manifest_path, crate_manifest_file), workspace_return)
}

pub fn setup_bench(tmp_dir: &TempDir) -> Vec<String> {
  let cargo_toml = include_str!("../tests/test_data/a_big_cargo.toml");
  let ((cargo_manifest_path, _), _) = setup_fs_test(tmp_dir, None, cargo_toml);

  // set env var
  #[expect(unsafe_code)]
  // SAFETY: The test is marked as serial, so it is safe to set the environment variable.
  unsafe {
    std::env::remove_var("CARGO_MANIFEST_PATH");
    std::env::set_var("CARGO_MANIFEST_DIR", cargo_manifest_path.parent().unwrap());
  };

  let cargo_manifest = CargoManifest::shared();
  let possible_crate_names = cargo_manifest
    .available_dependencies()
    .map(String::from)
    .collect();
  possible_crate_names
}
