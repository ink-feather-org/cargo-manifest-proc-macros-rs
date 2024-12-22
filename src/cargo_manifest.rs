// Tests and workspace dependency support from: https://github.com/bkchr/proc-macro-crate/blob/a5939f3fbf94279b45902119d97f881fefca6a0d/src/lib.rs
// Based on: https://github.com/bevyengine/bevy/blob/main/crates/bevy_macro_utils/src/bevy_manifest.rs

use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  process::Command,
  sync::LazyLock,
};
use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table};
use tracing::{info, trace};

use crate::syn_utils::{crate_name_to_path, pretty_format_syn_path};

/// A piece of a [`syn::Path`].
pub type PathPiece = syn::punctuated::Punctuated<syn::PathSegment, syn::Token![::]>;

/// A policy for how re-exporting crates re-export their dependencies.
pub trait CrateReExportingPolicy {
  /// Computes the re-exported path for a crate name.
  fn get_re_exported_crate_path(&self, crate_name: &str) -> Option<PathPiece>;
}

/// A known re-exporting crate.
pub struct KnownReExportingCrate<'a> {
  /// The name of the crate that re-exports.
  pub re_exporting_crate_name: &'a str,
  /// The policy for how the crate re-exports its dependencies.
  pub crate_re_exporting_policy: &'a dyn CrateReExportingPolicy,
}

/// Errors that can occur when trying to resolve a crate path.
#[derive(Error, Clone, Debug, PartialEq, Eq)]
pub enum TryResolveCratePathError {
  /// The crate name is ambiguous.
  #[error("Ambiguous crate dependency {0}.")]
  AmbiguousDependency(String),
  /// The crate path could not be found.
  #[error("Could not find crate path for {0}.")]
  CratePathNotFound(String),
  /// All known re-exporting crates failed to resolve the crate path.
  #[error(
    "All known re-exporting crates failed to resolve crate path for {0} with the following errors: {1:?}"
  )]
  AllReExportingCratesFailedToResolve(String, Vec<TryResolveCratePathError>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DependencyState {
  Ambiguous(String),
  Resolved(String),
}

struct WorkspaceDependencyResolver<'a> {
  crate_manifest_path: &'a Path,
  /// The key is the workspace dependency name.
  /// The value is the original crate name.
  resolved_workspace_dependencies: Option<BTreeMap<String, String>>,
}

impl<'a> WorkspaceDependencyResolver<'a> {
  const fn new(crate_manifest_path: &'a Path) -> Self {
    Self {
      crate_manifest_path,
      resolved_workspace_dependencies: None,
    }
  }

  fn resolve_workspace_dependencies(workspace_cargo_toml: &Table) -> BTreeMap<String, String> {
    let workspace_section = workspace_cargo_toml
      .get("workspace")
      .expect("The workspace section is missing")
      .as_table()
      .expect("The workspace section should be a table");
    let dependencies_section_iter = CargoManifest::all_dependencies_tables(workspace_section);
    let dependencies_iter =
      dependencies_section_iter.flat_map(|dependencies_section| dependencies_section.iter());

    let mut resolved_workspace_dependencies = BTreeMap::new();
    for (dependency_key, dependency_item) in dependencies_iter {
      let original_crate_name = dependency_item
        .get("package")
        .and_then(|package_field| package_field.as_str())
        .unwrap_or(dependency_key);
      resolved_workspace_dependencies
        .try_insert(dependency_key.to_string(), original_crate_name.to_string())
        .expect("Duplicate workspace dependency");
    }
    resolved_workspace_dependencies
  }

  /// TODO: write test for ambiguous workspace dependency
  fn resolve_original_crate_name_for_workspace_dependency(
    &mut self,
    workspace_dependency_name: &str,
  ) -> &str {
    let workspace_dependencies = self.resolved_workspace_dependencies.get_or_insert_with(|| {
      let workspace_cargo_toml = CargoManifest::load_workspace_cargo_toml(self.crate_manifest_path);
      Self::resolve_workspace_dependencies(workspace_cargo_toml.as_table())
    });

    let resolved_dependency = workspace_dependencies
      .get(workspace_dependency_name)
      .unwrap_or_else(|| {
        panic!(
          "Should have found the dependency in the workspace dependencies {} {:?}",
          workspace_dependency_name, workspace_dependencies
        )
      });

    trace!(
      "Resolved workspace dependency: {} -> {:?}",
      workspace_dependency_name, resolved_dependency
    );
    resolved_dependency
  }
}

/// The cargo manifest (`Cargo.toml`) of the crate that is being built.
/// This can be the user's crate that either directly or indirectly depends on your crate.
/// If there are uses of the proc-macro in your own crate, it may also point to the manifest of your own crate.
///
/// The [`CargoManifest`] is used to resolve a crate name to an absolute module path.
pub struct CargoManifest {
  user_crate_name: String,

  /// The key is the original crate name.
  /// The value is the resolved crate name.
  crate_dependencies: BTreeMap<String, DependencyState>,
}

impl CargoManifest {
  /// Returns a global shared instance of the [`CargoManifest`] struct.
  #[must_use = "This method returns the shared instance of the CargoManifest."]
  pub fn shared() -> &'static LazyLock<Self> {
    static LAZY_MANIFEST: LazyLock<CargoManifest> = LazyLock::new(|| {
      let cargo_manifest_dir = proc_macro::tracked_env::var("CARGO_MANIFEST_DIR");

      let cargo_manifest_path = cargo_manifest_dir
        .map(PathBuf::from)
        .map(|mut path| {
          path.push("Cargo.toml");
          info!("CARGO_MANIFEST_PATH: {:?}", path);
          proc_macro::tracked_path::path(path.to_string_lossy());
          assert!(
            path.exists(),
            "Cargo.toml does not exist at: {}",
            path.display()
          );
          path
        })
        .expect("The CARGO_MANIFEST_DIR environment variable is not defined.");

      let crate_manifest = CargoManifest::parse_cargo_manifest(&cargo_manifest_path);

      // parse user crate name
      let user_crate_name = CargoManifest::extract_user_crate_name(crate_manifest.as_table());

      let mut workspace_dependencies = Some(WorkspaceDependencyResolver::new(&cargo_manifest_path));

      let resolved_dependencies = CargoManifest::extract_dependency_map_for_cargo_manifest(
        crate_manifest.as_table(),
        &mut workspace_dependencies,
      );

      CargoManifest {
        user_crate_name,
        crate_dependencies: resolved_dependencies,
      }
    });
    &LAZY_MANIFEST
  }

  #[must_use]
  fn extract_user_crate_name(cargo_manifest: &Table) -> String {
    cargo_manifest
      .get("package")
      .and_then(|package_section| package_section.get("name"))
      .and_then(|name_field| name_field.as_str())
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
    workspace_dependency_resolver: &mut Option<WorkspaceDependencyResolver<'_>>,
  ) -> BTreeMap<String, DependencyState> {
    let cargo_manifest = if workspace_dependency_resolver.is_none() {
      // If we are not resolving workspace dependencies, we should only consider the `[workspace.dependencies]` section.
      cargo_manifest
        .get("workspace")
        .expect("The workspace section is missing")
        .as_table()
        .expect("The workspace section should be a table")
    } else {
      // We are not resolving workspace dependencies, so we use the top-level `[dependencies]` section.
      cargo_manifest
    };

    let dependencies_section_iter = Self::all_dependencies_tables(cargo_manifest);

    let dependencies_iter =
      dependencies_section_iter.flat_map(|dependencies_section| dependencies_section.iter());

    let mut resolved_dependencies = BTreeMap::new();

    for (dependency_key, dependency_item) in dependencies_iter {
      // Get the actual dependency name whether it is remapped or not
      let original_crate_name = dependency_item
        .get("package")
        .and_then(|package_field| package_field.as_str())
        .unwrap_or(dependency_key);

      // Check if the dependency is a workspace dependency.
      let is_workspace_dependency = dependency_item
        .get("workspace")
        .is_some_and(|workspace_field| workspace_field.as_bool() == Some(true));

      let original_crate_name = if is_workspace_dependency {
        workspace_dependency_resolver
          .as_mut()
          .expect("Encountered a workspace dependency in the [workspace.dependencies] section.")
          .resolve_original_crate_name_for_workspace_dependency(dependency_key)
      } else {
        original_crate_name
      };

      let insert_result = resolved_dependencies.try_insert(
        original_crate_name.to_string(),
        DependencyState::Resolved(dependency_key.to_string()),
      );
      if insert_result.is_err() {
        resolved_dependencies.insert(
          original_crate_name.to_string(),
          DependencyState::Ambiguous(original_crate_name.to_string()),
        );
      }
    }

    resolved_dependencies
  }

  #[must_use]
  fn parse_cargo_manifest(cargo_manifest_path: &Path) -> DocumentMut {
    proc_macro::tracked_path::path(cargo_manifest_path.to_string_lossy());
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
  /// original-crate-name = "0.1"
  /// ```
  ///
  /// The function would return `Some("original-crate-name")` for the `Item` above.
  ///
  /// For the remapped crate case:
  ///
  /// ```toml
  /// [dependencies]
  /// renamed-crate-name = { version = "0.1", package = "original-crate-name" }
  /// ```
  ///
  /// The function would return `Some("renamed-crate-name")` for the `Item` above.
  ///
  /// # Errors
  ///
  /// If the crate name is ambiguous, an error is returned.
  fn try_resolve_crate_path_internal(
    &self,
    query_crate_name: &str,
    known_re_exporting_crates: &[&KnownReExportingCrate<'_>],
  ) -> Result<syn::Path, TryResolveCratePathError> {
    // Check if the user crate is our own crate.
    if query_crate_name == self.user_crate_name {
      return Ok(crate_name_to_path(query_crate_name));
    }

    // Check if we have a direct dependency.
    let directly_mapped_crate_name = self.crate_dependencies.get(query_crate_name);
    if let Some(directly_mapped_crate_name) = directly_mapped_crate_name {
      return match directly_mapped_crate_name {
        DependencyState::Resolved(directly_mapped_crate_name) => {
          // We have a direct dependency.
          trace!("Found direct dependency: {}", directly_mapped_crate_name);
          Ok(crate_name_to_path(directly_mapped_crate_name))
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
        .get(known_re_exporting_crate.re_exporting_crate_name);
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
            known_re_exporting_crate.re_exporting_crate_name, query_crate_name
          );
          let mut path = crate_name_to_path(indirect_mapped_exporting_crate_name);
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
    let stdout = Command::new(
      proc_macro::tracked_env::var("CARGO")
        .expect("The CARGO environment variable is not defined."),
    )
    .arg("locate-project")
    .args(["--workspace", "--message-format=plain"])
    .arg(format!("--manifest-path={}", cargo_manifest_path.display()))
    .output()
    .expect("Failed to run `cargo locate-project`")
    .stdout;

    let path_string =
      String::from_utf8(stdout).expect("Failed to parse `cargo locate-project` output");
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
      "Resolved workspace manifest path: {}",
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
    info!("Trying to get the path for: {}", query_crate_name);

    trace!("Trying to get the path for: {}", query_crate_name);
    trace!("Dependencies: {:?}", self.crate_dependencies);

    let ret = self.try_resolve_crate_path_internal(query_crate_name, known_re_exporting_crates);

    info!(
      "Computed path: {:?} for {}",
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
      .unwrap_or_else(|_| crate_name_to_path(crate_name))
  }
}

#[cfg(test)]
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
          WorkspaceDependencyResolver::resolve_workspace_dependencies(
            workspace_manifest_toml.as_table(),
          )
        });
    let test_path = PathBuf::from("Invalid Path During Testing");
    let workspace_resolver = WorkspaceDependencyResolver {
      crate_manifest_path: &test_path,
      resolved_workspace_dependencies: workspace_dependency_map,
    };

    let user_crate_name = CargoManifest::extract_user_crate_name(&crate_manifest_toml);
    let resolved_dependencies = CargoManifest::extract_dependency_map_for_cargo_manifest(
      crate_manifest_toml.as_table(),
      &mut Some(workspace_resolver),
    );

    CargoManifest {
      user_crate_name,
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
      re_exporting_crate_name: "bevy",
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
