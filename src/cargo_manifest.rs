// Tests and workspace dependency support from: https://github.com/bkchr/proc-macro-crate/blob/a5939f3fbf94279b45902119d97f881fefca6a0d/src/lib.rs
// Based on: https://github.com/bevyengine/bevy/blob/main/crates/bevy_macro_utils/src/bevy_manifest.rs

use alloc::collections::BTreeMap;
use std::{
  path::{Path, PathBuf},
  process::Command,
  sync::{Mutex, MutexGuard},
};
use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table};
use tracing::{info, trace};

use crate::syn_utils::{crate_name_to_syn_path, pretty_format_syn_path};

fn get_env_var(name: &str) -> String {
  #[cfg(not(feature = "nightly"))]
  let env_var = std::env::var(name);
  #[cfg(feature = "nightly")]
  let env_var = proc_macro::tracked_env::var(name);
  env_var.unwrap_or_else(|_| panic!("The environment variable {name} must be set!"))
}

fn track_path(path: impl AsRef<str>) {
  #[cfg(not(feature = "nightly"))]
  let _ = path;
  #[cfg(feature = "nightly")]
  proc_macro::tracked_path::path(path);
}

/// A piece of a [`syn::Path`].
pub type PathPiece = syn::punctuated::Punctuated<syn::PathSegment, syn::Token![::]>;

/// A policy for how re-exporting crates re-export their dependencies.
pub trait CrateReExportingPolicy {
  /// Computes the re-exported path for a package name.
  fn get_re_exported_crate_path(&self, package_name: &str) -> Option<PathPiece>;
}

/// A known re-exporting crate.
pub struct KnownReExportingCrate<'a> {
  /// The package name of the crate that re-exports.
  pub re_exporting_crate_package_name: &'a str,
  /// The policy for how the crate re-exports its dependencies.
  pub crate_re_exporting_policy: &'a dyn CrateReExportingPolicy,
}

/// Errors that can occur when trying to resolve a crate path.
#[derive(Error, Clone, Debug, PartialEq, Eq)]
pub enum TryResolveCratePathError {
  /// The crate's package name is ambiguous in the dependencies of the user crate.
  #[error("Ambiguous crate dependency \"{0}\".")]
  AmbiguousDependency(String),
  /// The crate path could not be found.
  #[error("Could not find crate path for \"{0}\".")]
  CratePathNotFound(String),
  /// All known re-exporting crates failed to resolve the crate path.
  #[error(
    "All known re-exporting crates failed to resolve the crate path for \"{0}\" with the following errors: {1:?}"
  )]
  AllReExportingCratesFailedToResolve(String, Vec<TryResolveCratePathError>),
}

enum DependencyState {
  Ambiguous(String),
  Resolved(String),
}

/// The workspace dependency resolver lazily parses the workspace `Cargo.toml`, if any workspace dependencies are present.
struct WorkspaceDependencyResolver<'a> {
  /// The path to the user's crate manifest.
  user_crate_manifest_path: &'a Path,
  /// The key is the workspace dependency name.
  /// The value is the crate's package name.
  workspace_dependencies: Option<BTreeMap<String, String>>,
}

impl<'a> WorkspaceDependencyResolver<'a> {
  #[must_use = "This is a constructor."]
  const fn new(crate_manifest_path: &'a Path) -> Self {
    Self {
      user_crate_manifest_path: crate_manifest_path,
      workspace_dependencies: None,
    }
  }

  #[must_use]
  fn load_workspace_cargo_manifest(workspace_cargo_toml: &Table) -> BTreeMap<String, String> {
    // Get the `[workspace]` section.
    let workspace_section = workspace_cargo_toml
      .get("workspace")
      .expect("The workspace section is missing!")
      .as_table()
      .expect("The workspace section should be a table!");

    // Iterate all [workspace."dependencies"] sections.
    let dependencies_section_iter = CargoManifest::all_dependencies_tables(workspace_section);
    let dependencies_iter =
      dependencies_section_iter.flat_map(|dependencies_section| dependencies_section.iter());

    // Create a mapping from the workspace dependency name to the crate's package name.
    let mut resolved_workspace_dependencies = BTreeMap::new();
    for (dependency_key, dependency_item) in dependencies_iter {
      let crate_package_name = dependency_item
        .get("package")
        .and_then(Item::as_str)
        .unwrap_or(dependency_key);
      if resolved_workspace_dependencies
        .insert(dependency_key.to_string(), crate_package_name.to_string())
        .is_some()
      {
        // There was an old value for the key, which should not happen.
        panic!("Duplicated workspace dependency names are not permitted!");
      }
    }
    resolved_workspace_dependencies
  }

  /// Searches the workspace dependencies for the crate's package name.
  #[must_use]
  fn resolve_crate_package_name_from_workspace_dependency(
    &mut self,
    workspace_dependency_name: &str,
  ) -> &str {
    // Get or initialize the workspace dependencies.
    let workspace_dependencies = self.workspace_dependencies.get_or_insert_with(|| {
      let workspace_cargo_toml =
        CargoManifest::load_workspace_cargo_toml(self.user_crate_manifest_path);
      Self::load_workspace_cargo_manifest(workspace_cargo_toml.as_table())
    });

    let resolved_dependency = workspace_dependencies
      .get(workspace_dependency_name)
      .unwrap_or_else(|| {
        panic!(
          "Should have found the dependency \"{workspace_dependency_name}\" in the workspace dependencies \"{workspace_dependencies:?}\"!"
        )
      });

    trace!(
      "Resolved workspace dependency name \"{workspace_dependency_name}\" to its package name \"{resolved_dependency}."
    );
    resolved_dependency
  }
}

/// The [`CargoManifest`] is used to resolve a crate name to an absolute module path.
///
/// It stores information about the `Cargo.toml` of the crate that is currently being built
/// and its workspace `Cargo.toml` if it exists.
///
/// If there are uses of your proc-macro in your own crate, it may also point to the manifest of your own crate.
pub struct CargoManifest {
  /// The name of the crate that is currently being built.
  user_crate_name: String,

  /// The modified time of the `Cargo.toml`, when the `CargoManifest` instance was created.
  cargo_manifest_mtime: std::time::SystemTime,

  /// The key is the crate's package name.
  /// The value is the renamed crate name.
  crate_dependencies: BTreeMap<String, DependencyState>,
}

impl CargoManifest {
  /// Returns a global shared instance of the [`CargoManifest`] struct.
  #[must_use = "This method returns the shared instance of the CargoManifest."]
  #[expect(clippy::mut_mutex_lock)]
  pub fn shared() -> MutexGuard<'static, Self> {
    static MANIFESTS: Mutex<BTreeMap<PathBuf, &'static Mutex<CargoManifest>>> =
      Mutex::new(BTreeMap::new());

    // Get the current cargo manifest path and its modified time.
    // Rust-Analyzer keeps the proc macro server running between invocations and only the environment variables change.
    // This means 'static variables are not reset between invocations for different crates.
    let current_cargo_manifest_path = Self::get_current_cargo_manifest_path();
    let current_cargo_manifest_mtime = Self::get_cargo_manifest_mtime(&current_cargo_manifest_path);

    let mut manifests = MANIFESTS.lock().unwrap();

    // Get the cached shared instance of the CargoManifest.
    let existing_shared_instance =
      manifests
        .get(&current_cargo_manifest_path)
        .map(|cargo_manifest_mutex| {
          let mut shared_instance = cargo_manifest_mutex.lock().unwrap();
          // We do this to avoid leaking a new CargoManifest instance, when a Cargo.toml we had already parsed previously is changed.
          if shared_instance.cargo_manifest_mtime != current_cargo_manifest_mtime {
            // The mtime of the crate manifest has changed.
            // We need to recompute the CargoManifest.

            let cargo_manifest = Self::new_with_current_env_vars(
              &current_cargo_manifest_path,
              current_cargo_manifest_mtime,
            );

            // Overwrite the cache with the new cargo manifest version.
            *shared_instance = cargo_manifest;
          }

          shared_instance
        });

    let shared_instance = existing_shared_instance.unwrap_or_else(move || {
      // A new Cargo.toml has been requested, so we have to leak a new CargoManifest instance.
      let new_shared_instance = Box::leak(Box::new(Mutex::new(Self::new_with_current_env_vars(
        &current_cargo_manifest_path,
        current_cargo_manifest_mtime,
      ))));

      // Overwrite the cache with the new cargo manifest version.
      manifests.insert(current_cargo_manifest_path, new_shared_instance);

      new_shared_instance.lock().unwrap()
    });

    shared_instance
  }

  #[must_use]
  fn get_current_cargo_manifest_path() -> PathBuf {
    // The environment variable `CARGO_MANIFEST_PATH` is not consistently set in all environments.

    // Access environment variables through the `tracked_env` module to ensure that the proc-macro is re-run when the environment variables change.
    let mut cargo_manifest_path = PathBuf::from(get_env_var("CARGO_MANIFEST_DIR"));
    cargo_manifest_path.push("Cargo.toml");
    assert!(
      cargo_manifest_path.exists(),
      "Cargo.toml does not exist at \"{}\"!",
      cargo_manifest_path.display()
    );
    cargo_manifest_path
  }

  #[must_use]
  fn get_cargo_manifest_mtime(cargo_manifest_path: &Path) -> std::time::SystemTime {
    std::fs::metadata(cargo_manifest_path)
      .expect("Failed to get metadata for the crate manifest!")
      .modified()
      .expect("Failed to get the modified time of the crate manifest!")
  }

  #[must_use = "This is a constructor."]
  fn new_with_current_env_vars(
    cargo_manifest_path: &Path,
    cargo_manifest_mtime: std::time::SystemTime,
  ) -> Self {
    let crate_manifest = Self::parse_cargo_manifest(cargo_manifest_path);

    // Extract the user crate package name.
    let user_crate_name = Self::extract_user_crate_name(crate_manifest.as_table());

    let mut workspace_dependencies = WorkspaceDependencyResolver::new(cargo_manifest_path);

    let resolved_dependencies = Self::extract_dependency_map_for_cargo_manifest(
      crate_manifest.as_table(),
      &mut workspace_dependencies,
    );

    Self {
      user_crate_name,
      cargo_manifest_mtime,
      crate_dependencies: resolved_dependencies,
    }
  }

  #[must_use]
  fn extract_user_crate_name(cargo_manifest: &Table) -> String {
    cargo_manifest
      .get("package")
      .and_then(|package_section| package_section.get("name"))
      .and_then(Item::as_str)
      .expect("The package name in the Cargo.toml should be a string")
      .to_string()
  }

  fn target_dependencies_tables(cargo_toml: &Table) -> impl Iterator<Item = &Table> {
    cargo_toml
      .get("target")
      .into_iter()
      .filter_map(Item::as_table)
      .flat_map(|t| {
        t.iter()
          .map(|(_, value)| value)
          .filter_map(Item::as_table)
          .flat_map(Self::normal_dependencies_tables)
      })
  }

  fn normal_dependencies_tables(table: &Table) -> impl Iterator<Item = &Table> {
    table
      .get("dependencies")
      .into_iter()
      .chain(table.get("dev-dependencies"))
      .chain(table.get("build-dependencies"))
      .filter_map(Item::as_table)
  }

  fn all_dependencies_tables(cargo_toml: &Table) -> impl Iterator<Item = &Table> {
    Self::normal_dependencies_tables(cargo_toml).chain(Self::target_dependencies_tables(cargo_toml))
  }

  #[must_use]
  fn extract_dependency_map_for_cargo_manifest(
    cargo_manifest: &Table,
    workspace_dependency_resolver: &mut WorkspaceDependencyResolver<'_>,
  ) -> BTreeMap<String, DependencyState> {
    let dependencies_section_iter = Self::all_dependencies_tables(cargo_manifest);

    let dependencies_iter =
      dependencies_section_iter.flat_map(|dependencies_section| dependencies_section.iter());

    let mut resolved_dependencies = BTreeMap::new();

    for (dependency_key, dependency_item) in dependencies_iter {
      // True if `crate-name-possibly-renamed = { workspace = true }`
      let is_workspace_dependency = dependency_item
        .get("workspace")
        .is_some_and(|workspace_field| workspace_field.as_bool() == Some(true));

      let crate_package_name = if is_workspace_dependency {
        // Consult the workspace `Cargo.toml` to resolve the workspace dependency to its package name.
        workspace_dependency_resolver
          .resolve_crate_package_name_from_workspace_dependency(dependency_key)
      } else {
        // `crate-name-renamed = { package = "crate-name-package" }` or `crate-name-renamed = "0.1"`
        dependency_item
          .get("package")
          .and_then(Item::as_str)
          // If the package field is not present, the dependency key is the package name.
          .unwrap_or(dependency_key)
      };

      // Attempt to insert the package name to renamed dependency name mapping.
      if resolved_dependencies.contains_key(crate_package_name) {
        resolved_dependencies.insert(
          crate_package_name.to_string(),
          DependencyState::Ambiguous(crate_package_name.to_string()),
        );
      } else {
        resolved_dependencies.insert(
          crate_package_name.to_string(),
          DependencyState::Resolved(dependency_key.to_string()),
        );
      }
    }

    resolved_dependencies
  }

  #[must_use]
  fn parse_cargo_manifest(cargo_manifest_path: &Path) -> DocumentMut {
    // Track the path to ensure that the proc-macro is re-run when the `Cargo.toml` changes.
    track_path(cargo_manifest_path.to_string_lossy());
    let cargo_manifest_string =
      std::fs::read_to_string(cargo_manifest_path).unwrap_or_else(|err| {
        panic!(
          "Unable to read cargo manifest: {} - {err}",
          cargo_manifest_path.display()
        )
      });

    cargo_manifest_string
      .parse::<DocumentMut>()
      .unwrap_or_else(|err| {
        panic!(
          "Failed to parse cargo manifest: {} - {err}",
          cargo_manifest_path.display()
        )
      })
  }

  /// Gets the absolute module path for a crate from a supplied dependencies section.
  ///
  /// Crates that had their module path remapped are also supported.
  ///
  /// For the normal crate case:
  ///
  /// ```toml
  /// [dependencies]
  /// package-crate-name = "0.1"
  /// ```
  ///
  /// The function would return `Ok("package-crate-name")` for the `Item` above.
  ///
  /// For the remapped crate case:
  ///
  /// ```toml
  /// [dependencies]
  /// renamed-crate-name = { version = "0.1", package = "package-crate-name" }
  /// ```
  ///
  /// The function would return `Some("renamed-crate-name")` for the `Item` above.
  ///
  /// # Errors
  ///
  /// If the crate name is ambiguous or not found, an error is returned.
  fn try_resolve_crate_path_internal(
    &self,
    query_crate_name: &str,
    known_re_exporting_crates: &[&KnownReExportingCrate<'_>],
  ) -> Result<syn::Path, TryResolveCratePathError> {
    // Check if the user crate is our own crate.
    if query_crate_name == self.user_crate_name {
      return Ok(crate_name_to_syn_path(query_crate_name));
    }

    // Check if we have a direct dependency.
    let directly_mapped_crate_name = self.crate_dependencies.get(query_crate_name);
    if let Some(directly_mapped_crate_name) = directly_mapped_crate_name {
      return match directly_mapped_crate_name {
        DependencyState::Resolved(directly_mapped_crate_name) => {
          // We have a direct dependency.
          trace!(
            "Found direct dependency: \"{}\"",
            directly_mapped_crate_name
          );
          Ok(crate_name_to_syn_path(directly_mapped_crate_name))
        },
        DependencyState::Ambiguous(crate_name) => Err(
          TryResolveCratePathError::AmbiguousDependency(crate_name.clone()),
        ),
      };
    }

    // Check if we have a known re-exporting crate.
    let mut errors = Vec::new();
    for known_re_exporting_crate in known_re_exporting_crates {
      // Check if we have a known re-exporting crate.
      let indirect_mapped_exporting_crate_name = self
        .crate_dependencies
        .get(known_re_exporting_crate.re_exporting_crate_package_name);
      if let Some(indirect_mapped_exporting_crate_name) = indirect_mapped_exporting_crate_name {
        let indirect_mapped_exporting_crate_name = match indirect_mapped_exporting_crate_name {
          DependencyState::Resolved(crate_name) => crate_name,
          DependencyState::Ambiguous(crate_name) => {
            errors.push(TryResolveCratePathError::AmbiguousDependency(
              crate_name.clone(),
            ));
            continue;
          },
        };

        // We have a known re-exporting crate.
        let re_exported_crate_path = known_re_exporting_crate
          .crate_re_exporting_policy
          .get_re_exported_crate_path(query_crate_name);
        if let Some(re_exported_crate_path) = re_exported_crate_path {
          trace!(
            "Found re-exporting crate: {} -> {}",
            known_re_exporting_crate.re_exporting_crate_package_name, query_crate_name
          );
          let mut path = crate_name_to_syn_path(indirect_mapped_exporting_crate_name);
          path.segments.extend(re_exported_crate_path);
          return Ok(path);
        }
      }
    }

    if !errors.is_empty() {
      return Err(
        TryResolveCratePathError::AllReExportingCratesFailedToResolve(
          query_crate_name.to_string(),
          errors,
        ),
      );
    }

    Err(TryResolveCratePathError::CratePathNotFound(
      query_crate_name.to_string(),
    ))
  }

  /// https://github.com/bkchr/proc-macro-crate/blob/a5939f3fbf94279b45902119d97f881fefca6a0d/src/lib.rs#L243
  #[must_use]
  fn resolve_workspace_manifest_path(cargo_manifest_path: &Path) -> PathBuf {
    let stdout = Command::new(get_env_var("CARGO"))
      .arg("locate-project")
      .args(["--workspace", "--message-format=plain"])
      .arg(format!("--manifest-path={}", cargo_manifest_path.display()))
      .output()
      .expect("Failed to run `cargo locate-project`!")
      .stdout;

    let path_string =
      String::from_utf8(stdout).expect("Failed to parse `cargo locate-project` output!");
    let path_str = path_string.trim();

    let resolved_path = if path_str.is_empty() {
      // If `workspace_manifest_path` returns `None`, we are probably in a vendored deps
      // folder and cargo complaining that we have some package inside a workspace, that isn't
      // part of the workspace. In this case we just use the `manifest_path` as the
      // `workspace_manifest_path`.
      cargo_manifest_path.to_owned()
    } else {
      PathBuf::from(path_str)
    };

    trace!(
      "Resolved workspace manifest path: \"{}\"",
      resolved_path.display()
    );

    resolved_path
  }

  #[must_use]
  fn load_workspace_cargo_toml(cargo_manifest_path: &Path) -> DocumentMut {
    let workspace_cargo_toml_path = Self::resolve_workspace_manifest_path(cargo_manifest_path);

    let workspace_cargo_toml_string = std::fs::read_to_string(workspace_cargo_toml_path.clone())
      .unwrap_or_else(|err| {
        panic!(
          "Unable to read workspace cargo manifest: {} - {err}",
          workspace_cargo_toml_path.display()
        )
      });

    let workspace_cargo_toml = workspace_cargo_toml_string
      .parse::<DocumentMut>()
      .unwrap_or_else(|err| {
        panic!(
          "Failed to parse workspace cargo manifest: {} - {err}",
          workspace_cargo_toml_path.display()
        )
      });

    workspace_cargo_toml
  }

  /// Attempt to retrieve the absolute module path of a crate named [possible_crate_names](str) as an absolute [`syn::Path`].
  /// Remapped crate names are also supported.
  ///
  /// # Arguments
  ///
  /// * `crate_name` - The name of the crate to get the path for.
  ///
  /// * `known_re_exporting_crates` - A list of known crates that re-export the proc-macro.
  ///   This is useful for monorepos like bevy where the user typically only depends on the main bevy crate but uses proc-macros from subcrates like `bevy_ecs`.
  ///   If a direct dependency exists, it is preferred over a re-exporting crate.
  ///
  /// # Errors
  ///
  /// * If the crate name is ambiguous, an error is returned.
  /// * If the crate path could not be found, an error is returned.
  /// * If all known re-exporting crates failed to resolve the crate path, an error is returned.
  pub fn try_resolve_crate_path(
    &self,
    query_crate_name: &str,
    known_re_exporting_crates: &[&KnownReExportingCrate<'_>],
  ) -> Result<syn::Path, TryResolveCratePathError> {
    info!("Trying to get the path for: \"{query_crate_name}\"");

    let ret = self.try_resolve_crate_path_internal(query_crate_name, known_re_exporting_crates);

    info!(
      "Computed path: \"{:?}\" for \"{}\"",
      ret.as_ref().map(pretty_format_syn_path),
      query_crate_name
    );
    ret
  }

  /// Returns the path for the crate with the given name.
  #[must_use = "This method returns the resolved path for the crate."]
  pub fn resolve_crate_path(
    &self,
    crate_name: &str,
    known_re_exporting_crates: &[&KnownReExportingCrate<'_>],
  ) -> syn::Path {
    self
      .try_resolve_crate_path(crate_name, known_re_exporting_crates)
      .unwrap_or_else(|_| crate_name_to_syn_path(crate_name))
  }
}

#[cfg(test)]
#[doc(hidden)]
mod tests {
  use super::*;

  fn create_test_cargo_manifest(
    crate_manifest: &str,
    workspace_manifest: Option<&str>,
  ) -> CargoManifest {
    let crate_manifest_toml = crate_manifest
      .parse::<DocumentMut>()
      .expect("Failed to parse test manifest");
    let workspace_manifest_toml = workspace_manifest.map(|s| {
      s.parse::<DocumentMut>()
        .expect("Failed to parse workspace manifest")
    });
    let workspace_dependency_map =
      workspace_manifest_toml
        .as_ref()
        .map(|workspace_manifest_toml| {
          WorkspaceDependencyResolver::load_workspace_cargo_manifest(
            workspace_manifest_toml.as_table(),
          )
        });
    let test_path = PathBuf::from("Invalid Path During Testing");
    let mut workspace_resolver = WorkspaceDependencyResolver {
      user_crate_manifest_path: &test_path,
      workspace_dependencies: workspace_dependency_map,
    };

    let user_crate_name = CargoManifest::extract_user_crate_name(&crate_manifest_toml);
    let resolved_dependencies = CargoManifest::extract_dependency_map_for_cargo_manifest(
      crate_manifest_toml.as_table(),
      &mut workspace_resolver,
    );

    CargoManifest {
      user_crate_name,
      cargo_manifest_mtime: std::time::SystemTime::UNIX_EPOCH,
      crate_dependencies: resolved_dependencies,
    }
  }

  #[test]
  fn own_package() {
    let crate_manifest = r#"
      [package]
      name = "test"
    "#;
    let workspace_manifest = None;
    let crate_to_resolve = "test";
    let expected_path = "::test".to_string();

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
  }

  #[test]
  fn direct_dependency() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [dependencies]
      test_dep = "0.1"
    "#;
    let workspace_manifest = None;
    let crate_to_resolve = "test_dep";
    let expected_path = "::test_dep".to_string();

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
  }

  #[test]
  fn direct_dev_dependency() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [dev-dependencies]
      test_dep = "0.1"
    "#;
    let workspace_manifest = None;
    let crate_to_resolve = "test_dep";
    let expected_path = "::test_dep".to_string();

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
  }

  #[test]
  fn ambiguous_dependency() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [dependencies]
      test_dep = "0.1"
      test_dep_renamed = { package = "test_dep" }
    "#;
    let workspace_manifest = None;
    let crate_to_resolve = "test_dep";
    let expected_error = TryResolveCratePathError::AmbiguousDependency("test_dep".to_string());

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      cargo_manifest
        .try_resolve_crate_path(crate_to_resolve, &[])
        .as_ref()
        .map(pretty_format_syn_path)
        .unwrap_err(),
      &expected_error
    );
  }

  #[test]
  fn remapped_direct_dependency() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [dependencies]
      test_dep_renamed = { package = "test_dep" }
    "#;
    let workspace_manifest = None;
    let crate_to_resolve = "test_dep";
    let expected_path = "::test_dep_renamed".to_string();

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
  }

  #[test]
  fn remapped_direct_dependency_second() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [dependencies.test_dep_renamed]
      package = "test_dep"
    "#;
    let workspace_manifest = None;
    let crate_to_resolve = "test_dep";
    let expected_path = "::test_dep_renamed".to_string();

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
  }

  #[test]
  fn not_found() {
    let crate_manifest = r#"
      [package]
      name = "test"
    "#;
    let workspace_manifest = None;
    let crate_to_resolve = "test_dep";
    let expected_error = TryResolveCratePathError::CratePathNotFound("test_dep".to_string());

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      cargo_manifest
        .try_resolve_crate_path(crate_to_resolve, &[])
        .as_ref()
        .map(pretty_format_syn_path)
        .unwrap_err(),
      &expected_error
    );
  }

  #[test]
  fn target_dependency() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [target.'cfg(windows)'.dependencies]
      test_dep = "0.1"
    "#;
    let workspace_manifest = None;
    let crate_to_resolve = "test_dep";
    let expected_path = "::test_dep".to_string();

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
  }

  #[test]
  fn target_dependency2() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [target.x86_64-pc-windows-gnu.dependencies]
      test_dep = "0.1"
    "#;
    let workspace_manifest = None;
    let crate_to_resolve = "test_dep";
    let expected_path = "::test_dep".to_string();

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
  }

  #[test]
  fn workspace_dependency() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [dependencies]
      test_dep = { version = "0.1", workspace = true }
    "#;
    let workspace_manifest = Some(
      r#"
      [workspace.dependencies]
      test_dep = "0.1"
    "#,
    );
    let crate_to_resolve = "test_dep";
    let expected_path = "::test_dep".to_string();

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
  }

  #[test]
  fn remapped_workspace_dependency() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [dependencies]
      test_dep_renamed = { workspace = true }
    "#;
    let workspace_manifest = Some(
      r#"
      [workspace.dependencies]
      test_dep_renamed = { package = "test_dep" }
    "#,
    );
    let crate_to_resolve = "test_dep";
    let expected_path = "::test_dep_renamed".to_string();

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
  }

  #[test]
  fn workspace_dev_dependency() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [dev-dependencies]
      test_dep = { version = "0.1", workspace = true }
    "#;
    let workspace_manifest = Some(
      r#"
      [workspace.dependencies]
      test_dep = "0.1"
    "#,
    );
    let crate_to_resolve = "test_dep";
    let expected_path = "::test_dep".to_string();

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
  }

  #[test]
  fn workspace_ambiguous_dependency() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [dependencies]
      test_dep_renamed = { workspace = true }
      test_dep = { workspace = true }
    "#;
    let workspace_manifest = Some(
      r#"
      [workspace.dependencies]
      test_dep_renamed = { package = "test_dep" }
      test_dep = "0.1"
    "#,
    );
    let crate_to_resolve = "test_dep";
    let expected_error = TryResolveCratePathError::AmbiguousDependency("test_dep".to_string());

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    assert_eq!(
      cargo_manifest
        .try_resolve_crate_path(crate_to_resolve, &[])
        .as_ref()
        .map(pretty_format_syn_path)
        .unwrap_err(),
      &expected_error
    );
  }

  struct BevyReExportingPolicy;

  impl CrateReExportingPolicy for BevyReExportingPolicy {
    fn get_re_exported_crate_path(&self, crate_name: &str) -> Option<PathPiece> {
      crate_name.strip_prefix("bevy_").map(|s| {
        let mut path = PathPiece::new();
        path.push(syn::parse_str::<syn::PathSegment>(s).unwrap());
        path
      })
    }
  }

  #[test]
  fn known_re_exporting_crate() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [dependencies]
      bevy = "0.1"
    "#;
    let workspace_manifest = None;
    let crate_to_resolve = "bevy_ecs";
    let expected_path = "::bevy::ecs".to_string();

    let cargo_manifest = create_test_cargo_manifest(crate_manifest, workspace_manifest);

    let known_re_exporting_crate = KnownReExportingCrate {
      re_exporting_crate_package_name: "bevy",
      crate_re_exporting_policy: &BevyReExportingPolicy {},
    };

    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[&known_re_exporting_crate])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
  }
}
