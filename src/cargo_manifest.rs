// Tests and workspace dependency support from: https://github.com/bkchr/proc-macro-crate/blob/a5939f3fbf94279b45902119d97f881fefca6a0d/src/lib.rs
// Based on: https://github.com/bevyengine/bevy/blob/main/crates/bevy_macro_utils/src/bevy_manifest.rs

use alloc::collections::BTreeMap;
use core::{
  cell::RefCell,
  sync::atomic::{self, AtomicU64},
};
use parking_lot::{RwLock, RwLockReadGuard};
use std::{
  path::{Path, PathBuf},
  time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use toml_edit::{ImDocument, Item, Table};
use tracing::{debug, error, info, trace};

use crate::{
  syn_utils::{crate_name_to_syn_path, pretty_format_syn_path},
  toml_strip,
};

fn get_system_time_fast(invalidate_cache: bool) -> SystemTime {
  thread_local! {
    static LAST_TIME: RefCell<SystemTime> = const { RefCell::new(UNIX_EPOCH) };
  }

  LAST_TIME.with(|last_time| {
    let mut last_time = last_time.borrow_mut();
    if invalidate_cache || *last_time == UNIX_EPOCH {
      *last_time = SystemTime::now();
    }

    *last_time
  })
}

/// # Errors
/// If the file metadata could not be read, an error is returned.
fn get_mtime_from_path(file_path: &Path) -> Result<SystemTime, std::io::Error> {
  std::fs::metadata(file_path).and_then(|metadata| metadata.modified())
}

fn system_time_to_ms(sys_time: SystemTime) -> u64 {
  u64::try_from(sys_time.duration_since(UNIX_EPOCH).unwrap().as_millis()).unwrap_or(0)
}

fn get_env_var(name: &str) -> Option<String> {
  #[cfg(not(all(feature = "nightly", feature = "proc-macro")))]
  let env_var = std::env::var(name);
  #[cfg(all(feature = "nightly", feature = "proc-macro"))]
  let env_var = proc_macro::tracked_env::var(name);
  env_var.ok()
}

fn track_path(path: impl AsRef<str>) {
  #[cfg(not(all(feature = "nightly", feature = "proc-macro")))]
  let _ = path;
  #[cfg(all(feature = "nightly", feature = "proc-macro"))]
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
  /// The path to the workspace `Cargo.toml`, if it is specified in the user's crate manifest.
  package_workspace_hint: Option<&'a Path>,
  /// The path to the user's crates workspace `Cargo.toml`.
  workspace_manifest_path: Option<PathBuf>,
  /// The modified time of the workspace `Cargo.toml`, when the `WorkspaceDependencyResolver` instance was created.
  workspace_manifest_mtime: Option<u64>,
  /// The key is the workspace dependency name.
  /// The value is the crate's package name.
  workspace_dependencies_map: Option<BTreeMap<String, String>>,
}

impl<'a> WorkspaceDependencyResolver<'a> {
  #[must_use = "This is a constructor."]
  const fn new(
    user_crate_manifest_path: &'a Path,
    package_workspace_hint: Option<&'a Path>,
  ) -> Self {
    Self {
      user_crate_manifest_path,
      package_workspace_hint,
      workspace_manifest_path: None,
      workspace_manifest_mtime: None,
      workspace_dependencies_map: None,
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
    let workspace_dependencies = self.workspace_dependencies_map.get_or_insert_with(|| {
      let workspace_cargo_toml_path = CargoManifest::resolve_workspace_manifest_path(
        self.user_crate_manifest_path,
        self.package_workspace_hint,
      );
      self.workspace_manifest_mtime = get_mtime_from_path(&workspace_cargo_toml_path)
        .ok()
        .map(system_time_to_ms);
      let workspace_cargo_toml =
        CargoManifest::load_workspace_cargo_toml(&workspace_cargo_toml_path);
      self.workspace_manifest_path = Some(workspace_cargo_toml_path);
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
      "Resolved workspace dependency name \"{workspace_dependency_name}\" to its package name \"{resolved_dependency}\"."
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

  /// The path to the user's crates workspace `Cargo.toml`.
  workspace_manifest_path: Option<PathBuf>,

  /// The modified time of the `Cargo.toml`, when the `CargoManifest` instance was created.
  user_crate_manifest_mtime_ms: u64,

  /// The modified time of the workspace `Cargo.toml`, when the `CargoManifest` instance was created.
  workspace_manifest_mtime_ms: Option<u64>,

  /// The key is the crate's package name.
  /// The value is the renamed crate name.
  crate_dependencies: BTreeMap<String, DependencyState>,

  package_last_mtime_syscall_ms: AtomicU64,
  workspace_last_mtime_syscall_ms: AtomicU64,
}

impl CargoManifest {
  const MTIME_CACHE_TIMEOUT_MS: u64 = 200;

  /// Returns a global shared instance of the [`CargoManifest`] struct.
  #[must_use = "This method returns the shared instance of the CargoManifest."]
  pub fn shared() -> RwLockReadGuard<'static, Self> {
    static MANIFESTS: RwLock<BTreeMap<PathBuf, &'static RwLock<CargoManifest>>> =
      RwLock::new(BTreeMap::new());

    let _ = get_system_time_fast(true);

    // Get the current cargo manifest path and its modified time.
    // Rust-Analyzer keeps the proc macro server running between invocations and only the environment variables change.
    // This means 'static variables are not reset between invocations for different crates.
    let current_cargo_manifest_path = Self::get_current_cargo_manifest_path();

    let existing_shared_instance_rw_lock =
      MANIFESTS.read().get(&current_cargo_manifest_path).copied();

    // Get the cached shared instance of the CargoManifest.
    if let Some(existing_shared_instance_rw_lock) = existing_shared_instance_rw_lock {
      let existing_shared_instance_guard = existing_shared_instance_rw_lock.read();

      let current_cargo_manifest_mtime_ms = Self::get_mtime_from_path_cached(
        &current_cargo_manifest_path,
        &existing_shared_instance_guard.package_last_mtime_syscall_ms,
        existing_shared_instance_guard.user_crate_manifest_mtime_ms,
      )
      .expect("Could not get the mtime of the crate manifest!");

      // Check if the mtime of the crate manifest has changed.
      let mut a_relevant_cargo_toml_changed = false;

      if existing_shared_instance_guard.user_crate_manifest_mtime_ms
        != current_cargo_manifest_mtime_ms
      {
        a_relevant_cargo_toml_changed = true;
        debug!(
          "Package manifest changed: {:?} {:?}",
          existing_shared_instance_guard.user_crate_manifest_mtime_ms,
          current_cargo_manifest_mtime_ms
        );
      }

      // Check if the mtime of the workspace manifest has changed.
      if let Some(workspace_manifest_path) = existing_shared_instance_guard
        .workspace_manifest_path
        .as_ref()
      {
        let workspace_cargo_toml_mtime = Self::get_mtime_from_path_cached(
          workspace_manifest_path,
          &existing_shared_instance_guard.workspace_last_mtime_syscall_ms,
          existing_shared_instance_guard
            .workspace_manifest_mtime_ms
            .unwrap(),
        )
        .ok();

        if existing_shared_instance_guard.workspace_manifest_mtime_ms != workspace_cargo_toml_mtime
        {
          a_relevant_cargo_toml_changed = true;
          debug!(
            "Workspace manifest changed: {:?} {:?}",
            existing_shared_instance_guard.workspace_manifest_mtime_ms, workspace_cargo_toml_mtime
          );
        }
      }

      drop(existing_shared_instance_guard);

      // We do this to avoid leaking a new CargoManifest instance, when a Cargo.toml we had already parsed previously is changed.
      if a_relevant_cargo_toml_changed {
        // The mtime of the crate manifest has changed.
        // We need to recompute the CargoManifest.

        let cargo_manifest = Self::new_with_current_env_vars(
          &current_cargo_manifest_path,
          current_cargo_manifest_mtime_ms,
        );

        // Overwrite the cache with the new cargo manifest version.
        *existing_shared_instance_rw_lock.write() = cargo_manifest;
      }

      return existing_shared_instance_rw_lock.read();
    }

    let mut manifests_guard_w = MANIFESTS.write();

    // While we were waiting for write access another thread may have already create the new CargoManifest for us
    if let Some(existing_shared_instance_rw_lock) =
      manifests_guard_w.get(&current_cargo_manifest_path)
    {
      return existing_shared_instance_rw_lock.read();
    }

    let current_cargo_manifest_mtime_ms = system_time_to_ms(
      get_mtime_from_path(&current_cargo_manifest_path)
        .expect("Could not get the mtime of the crate manifest!"),
    );

    // A new Cargo.toml has been requested, so we have to leak a new CargoManifest instance.
    let new_shared_instance = Box::leak(Box::new(RwLock::new(Self::new_with_current_env_vars(
      &current_cargo_manifest_path,
      current_cargo_manifest_mtime_ms,
    ))));

    // Overwrite the cache with the new cargo manifest version.
    manifests_guard_w.insert(current_cargo_manifest_path, new_shared_instance);

    // We have to drop the write guard before we can read the shared instance.
    drop(manifests_guard_w);

    new_shared_instance.read()
  }

  #[must_use]
  fn get_current_cargo_manifest_path() -> PathBuf {
    // Access environment variables through the `tracked_env` module to ensure that the proc-macro is re-run when the environment variables change.
    if let Some(cargo_manifest_dir) = get_env_var("CARGO_MANIFEST_DIR") {
      let mut cargo_manifest_path = PathBuf::with_capacity(cargo_manifest_dir.len() + 30);
      cargo_manifest_path.push(cargo_manifest_dir);
      cargo_manifest_path.push("Cargo.toml");
      return cargo_manifest_path;
    }
    // If the `CARGO_MANIFEST_DIR` environment variable is not set, we fall back to the `CARGO_MANIFEST_PATH` environment variable.
    if let Some(cargo_manifest_path) = get_env_var("CARGO_MANIFEST_PATH") {
      return PathBuf::from(cargo_manifest_path);
    }

    panic!("Could not find the crate manifest path! Either `CARGO_MANIFEST_DIR` or `CARGO_MANIFEST_PATH` must be set.");
  }

  /// # Errors
  /// If the file metadata could not be read, an error is returned.
  fn get_mtime_from_path_cached(
    file_path: &Path,
    last_mtime_syscall_ms: &AtomicU64,
    initial_mtime_ms: u64,
  ) -> Result<u64, std::io::Error> {
    let current_time_ms = system_time_to_ms(get_system_time_fast(false));

    let mtime_ms = if current_time_ms - last_mtime_syscall_ms.load(atomic::Ordering::Relaxed)
      > Self::MTIME_CACHE_TIMEOUT_MS
    {
      // We only check the mtime of the file when the cached value is too old.
      let fresh_mtime = get_mtime_from_path(file_path)?;
      let fresh_mtime_ms = system_time_to_ms(fresh_mtime);
      last_mtime_syscall_ms.store(current_time_ms, atomic::Ordering::Relaxed);

      trace!(
        "File mtime retrieved from fs: {:?} {:?}",
        initial_mtime_ms,
        fresh_mtime_ms,
      );

      fresh_mtime_ms
    } else {
      trace!("File mtime not checked: {:?}", initial_mtime_ms);

      initial_mtime_ms
    };

    Ok(mtime_ms)
  }

  #[must_use = "This is a constructor."]
  fn new_with_current_env_vars(
    cargo_manifest_path: &Path,
    user_crate_manifest_mtime_ms: u64,
  ) -> Self {
    let crate_manifest = Self::parse_cargo_manifest_from_fs(cargo_manifest_path);

    // Extract the user crate package name.
    let user_crate_name = Self::extract_user_crate_name(crate_manifest.as_table());
    // Extract the workspace hint.
    let package_workspace_hint =
      Self::extract_user_crate_package_workspace_hint(crate_manifest.as_table());

    let mut workspace_dependencies =
      WorkspaceDependencyResolver::new(cargo_manifest_path, package_workspace_hint.as_deref());

    let resolved_dependencies = Self::extract_dependency_map_for_cargo_manifest(
      crate_manifest.as_table(),
      &mut workspace_dependencies,
    );

    let workspace_manifest_mtime_ms = workspace_dependencies.workspace_manifest_mtime;
    let workspace_manifest_path = workspace_dependencies.workspace_manifest_path;

    let current_time_ms = system_time_to_ms(get_system_time_fast(false));

    Self {
      user_crate_name,
      user_crate_manifest_mtime_ms,
      workspace_manifest_path,
      workspace_manifest_mtime_ms,
      crate_dependencies: resolved_dependencies,

      package_last_mtime_syscall_ms: AtomicU64::new(current_time_ms),
      workspace_last_mtime_syscall_ms: AtomicU64::new(0),
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

  #[must_use]
  fn extract_user_crate_package_workspace_hint(cargo_manifest: &Table) -> Option<PathBuf> {
    cargo_manifest
      .get("package")
      .and_then(|package_section| package_section.get("workspace"))
      .and_then(Item::as_str)
      .map(PathBuf::from)
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

      // Attempt to insert a mapping from the package name the to renamed dependency name.
      if let Some(DependencyState::Resolved(previously_resolved)) =
        resolved_dependencies.get(crate_package_name)
      {
        if previously_resolved != dependency_key {
          // If the dependency previously mapped to a different crate, we mark it as ambiguous.
          resolved_dependencies.insert(
            crate_package_name.to_string(),
            DependencyState::Ambiguous(crate_package_name.to_string()),
          );
        }
      } else {
        // The dependency has never been resolved yet, so we insert it.
        resolved_dependencies.insert(
          crate_package_name.to_string(),
          DependencyState::Resolved(dependency_key.to_string()),
        );
      }
    }

    resolved_dependencies
  }

  #[must_use]
  fn parse_cargo_manifest_from_fs(cargo_manifest_path: &Path) -> ImDocument<String> {
    // Track the path to ensure that the proc-macro is re-run when the `Cargo.toml` changes.
    track_path(cargo_manifest_path.to_string_lossy());
    let full_cargo_manifest_string =
      std::fs::read_to_string(cargo_manifest_path).unwrap_or_else(|err| {
        panic!(
          "Unable to read cargo manifest: {} - {err}",
          cargo_manifest_path.display()
        )
      });

    let stripped_cargo_manifest_string =
      toml_strip::strip_irrelevant_sections_from_cargo_manifest(&full_cargo_manifest_string);

    stripped_cargo_manifest_string
      .parse::<ImDocument<String>>()
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
        #[allow(if_let_rescope)]
        if let Some(re_exported_crate_path) = re_exported_crate_path {
          trace!(
            "Found re-exporting crate: {} -> {}",
            known_re_exporting_crate.re_exporting_crate_package_name,
            query_crate_name
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

  /// A pure rust re-implementation of the `cargo locate-project` command.
  /// The algorithm is explained here: https://doc.rust-lang.org/cargo/reference/manifest.html#the-workspace-field
  ///
  /// 1) We take a look at the crate manifest to see if it has a [package.workspace] section. (If it is a folder path we append `Cargo.toml`.)
  /// 2) If it has we already have the workspace manifest path.
  /// 3) If not specified this will be inferred as the first Cargo.toml with [workspace] upwards in the filesystem.
  #[must_use]
  fn resolve_workspace_manifest_path(
    cargo_manifest_path: &Path,
    package_workspace_hint: Option<&Path>,
  ) -> PathBuf {
    if let Some(package_workspace_hint) = package_workspace_hint {
      let absolute_package_workspace_hint = if package_workspace_hint.is_absolute() {
        package_workspace_hint.to_owned()
      } else {
        cargo_manifest_path
          .parent()
          .unwrap()
          .join(package_workspace_hint)
      };
      if absolute_package_workspace_hint.ends_with("Cargo.toml") {
        return absolute_package_workspace_hint;
      }
      return absolute_package_workspace_hint.join("Cargo.toml");
    }

    let mut current_path = cargo_manifest_path.parent().unwrap();
    loop {
      let workspace_manifest_path = current_path.join("Cargo.toml");
      if workspace_manifest_path.exists() {
        // Parsing the full toml here is too expensive.
        // We only need to check if there is a line as follows: `<optional-whitespace> [workspace] <optional-whitespace>`
        let workspace_manifest_string = std::fs::read_to_string(&workspace_manifest_path)
          .unwrap_or_else(|err| {
            panic!(
              "Unable to read workspace cargo manifest: {} - {err}",
              workspace_manifest_path.display()
            )
          });

        // This breaks when a user has a Cargo.toml in the middle with a multiline string that contains `[workspace]` in one line.
        // However, this is a rare edge case.
        for line in workspace_manifest_string.lines() {
          if line.trim() == "[workspace]" {
            return workspace_manifest_path;
          }
        }
      }

      // Move up one directory.
      match current_path.parent() {
        Some(parent) => current_path = parent,
        None => break,
      }
    }

    panic!(
      "Could not find a workspace manifest for the crate manifest at \"{}\"!",
      cargo_manifest_path.display()
    );
  }

  #[must_use]
  fn load_workspace_cargo_toml(workspace_cargo_toml_path: &Path) -> ImDocument<String> {
    let workspace_cargo_toml_string = std::fs::read_to_string(workspace_cargo_toml_path)
      .unwrap_or_else(|err| {
        panic!(
          "Unable to read workspace cargo manifest: {} - {err}",
          workspace_cargo_toml_path.display()
        )
      });

    let workspace_cargo_toml = workspace_cargo_toml_string
      .parse::<ImDocument<String>>()
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

  /// Returns all the available dependencies of the user crate.
  #[must_use = "This method returns all the available dependencies of the user crate."]
  pub fn available_dependencies(&self) -> impl Iterator<Item = &str> {
    self.crate_dependencies.keys().map(String::as_str)
  }
}

#[doc(hidden)]
#[cfg(test)]
#[path = "shared_testing_benchmarking.rs"]
mod shared_testing_benchmarking;

#[cfg(test)]
#[doc(hidden)]
mod resolve_workspace_path_tests {
  use super::*;
  use tracing_test::traced_test;

  #[test]
  fn package_hint() {
    let some_cargo_toml = PathBuf::from("/super-workspace/my-crate/Cargo.toml");

    assert_eq!(
      CargoManifest::resolve_workspace_manifest_path(
        &some_cargo_toml,
        Some(&PathBuf::from("workspace"))
      ),
      PathBuf::from("/super-workspace/my-crate/workspace/Cargo.toml")
    );
    assert_eq!(
      CargoManifest::resolve_workspace_manifest_path(
        &some_cargo_toml,
        Some(&PathBuf::from("workspace/Cargo.toml"))
      ),
      PathBuf::from("/super-workspace/my-crate/workspace/Cargo.toml")
    );
    assert_eq!(
      CargoManifest::resolve_workspace_manifest_path(
        &some_cargo_toml,
        Some(&PathBuf::from("/workspace/Cargo.toml"))
      ),
      PathBuf::from("/workspace/Cargo.toml")
    );
  }

  fn fs_test_n_folders(number_of_nested_folders: usize) {
    info!("Testing with {} nested folders", number_of_nested_folders);

    let cargo_toml_no_workspace = r#"
      [package]
      name = "test"
    "#;
    let cargo_toml_workspace = r"
      [workspace]
    ";

    let temp_dir = tempfile::tempdir().unwrap();

    let cargo_toml_workspace_path = temp_dir.path().join("Cargo.toml");
    std::fs::write(&cargo_toml_workspace_path, cargo_toml_workspace).unwrap();

    let mut current_path = cargo_toml_workspace_path.parent().unwrap().to_owned();
    for folder_number in 0..number_of_nested_folders {
      let folder_name = "subfolder";
      current_path = current_path.join(folder_name);
      std::fs::create_dir(&current_path).unwrap();

      if folder_number % 2 == 1 {
        let cargo_toml_path = current_path.join("Cargo.toml");
        std::fs::write(&cargo_toml_path, cargo_toml_no_workspace).unwrap();
      }
    }

    let cargo_package_path = current_path.join("Cargo.toml");

    assert_eq!(
      CargoManifest::resolve_workspace_manifest_path(&cargo_package_path, None),
      cargo_toml_workspace_path
    );
  }

  #[test]
  #[traced_test]
  fn fs_test() {
    fs_test_n_folders(1);
    fs_test_n_folders(2);
    fs_test_n_folders(3);
    fs_test_n_folders(4);
  }
}

#[cfg(test)]
#[doc(hidden)]
mod resolver_tests {
  use super::*;

  fn create_test_cargo_manifest(
    crate_manifest: &str,
    workspace_manifest: Option<&str>,
  ) -> CargoManifest {
    let crate_manifest_toml = crate_manifest
      .parse::<ImDocument<String>>()
      .expect("Failed to parse test manifest");
    let workspace_manifest_toml = workspace_manifest.map(|s| {
      s.parse::<ImDocument<String>>()
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
      package_workspace_hint: None,
      workspace_manifest_path: None,
      workspace_manifest_mtime: None,
      workspace_dependencies_map: workspace_dependency_map,
    };

    let user_crate_name = CargoManifest::extract_user_crate_name(&crate_manifest_toml);
    let resolved_dependencies = CargoManifest::extract_dependency_map_for_cargo_manifest(
      crate_manifest_toml.as_table(),
      &mut workspace_resolver,
    );

    CargoManifest {
      user_crate_name,
      user_crate_manifest_mtime_ms: 0,
      workspace_manifest_path: None,
      workspace_manifest_mtime_ms: None,
      crate_dependencies: resolved_dependencies,

      package_last_mtime_syscall_ms: AtomicU64::new(0),
      workspace_last_mtime_syscall_ms: AtomicU64::new(0),
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
  fn direct_dev_and_normal_dependency() {
    let crate_manifest = r#"
      [package]
      name = "test"

      [dependencies]
      test_dep = "0.1"

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

#[cfg(all(not(feature = "proc-macro"), test))]
#[cfg(test)]
#[doc(hidden)]
pub mod fs_tests {
  use super::{
    shared_testing_benchmarking::{setup_fs_test, SERIAL_TEST},
    *,
  };
  use std::{
    io::{Seek, Write},
    time::Duration,
  };
  use tracing_test::traced_test;

  #[test]
  #[traced_test]
  fn single_lookup() {
    let _guard = SERIAL_TEST.lock();
    let tmp_dir = tempfile::tempdir().unwrap();

    let crate_manifest = r#"
      [package]
      name = "test"

      [dependencies]
      test_dep = "0.1"
    "#;

    let ((crate_manifest_path, _), _) = setup_fs_test(&tmp_dir, None, crate_manifest);

    // set env var
    #[expect(unsafe_code)]
    // SAFETY: The test is marked as serial, so it is safe to set the environment variable.
    unsafe {
      std::env::remove_var("CARGO_MANIFEST_PATH");
      std::env::set_var("CARGO_MANIFEST_DIR", crate_manifest_path.parent().unwrap());
    };

    let cargo_manifest = CargoManifest::shared();

    let crate_to_resolve = "test_dep";
    let expected_path = "::test_dep".to_string();
    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );

    drop(cargo_manifest);

    let cargo_manifest = CargoManifest::shared();
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
  fn modify_crate_manifest() {
    let _guard = SERIAL_TEST.lock();
    let tmp_dir = tempfile::tempdir().unwrap();

    let initial_crate_manifest = r#"
      [package]
      name = "test"

      [dependencies]
      test_dep = "0.1"
    "#;

    let updated_crated_manifest = r#"
      [package]
      name = "test"

      [dependencies]
      test_dep_renamed = { package = "test_dep" }
    "#;

    let ((crate_manifest_path, mut crate_manifest_file), _) =
      setup_fs_test(&tmp_dir, None, initial_crate_manifest);

    // set env var
    #[expect(unsafe_code)]
    // SAFETY: The test is marked as serial, so it is safe to set the environment variable.
    unsafe {
      std::env::remove_var("CARGO_MANIFEST_PATH");
      std::env::set_var("CARGO_MANIFEST_DIR", crate_manifest_path.parent().unwrap());
    };
    let initial_mtime = get_mtime_from_path(&crate_manifest_path).unwrap();

    let cargo_manifest = CargoManifest::shared();

    // resolve the path for the initial crate manifest
    let crate_to_resolve = "test_dep";
    let expected_path = "::test_dep".to_string();
    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );

    // update the crate manifest
    crate_manifest_file
      .seek(std::io::SeekFrom::Start(0))
      .unwrap();
    crate_manifest_file
      .write_all(updated_crated_manifest.as_bytes())
      .unwrap();
    // update the mtime
    std::thread::sleep(Duration::from_millis(10));
    crate_manifest_file.set_modified(SystemTime::now()).unwrap();
    crate_manifest_file.flush().unwrap();
    let new_mtime = get_mtime_from_path(&crate_manifest_path).unwrap();
    drop(cargo_manifest);

    // check that the mtime has changed
    assert_ne!(initial_mtime, new_mtime);

    // sleep for the cache to expire
    std::thread::sleep(Duration::from_millis(
      CargoManifest::MTIME_CACHE_TIMEOUT_MS + 100,
    ));

    let cargo_manifest = CargoManifest::shared();

    // resolve the path for the updated crate manifest
    let expected_path = "::test_dep_renamed".to_string();
    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
    drop(cargo_manifest);

    // check it again
    let cargo_manifest = CargoManifest::shared();
    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
    drop(cargo_manifest);
  }

  #[test]
  #[traced_test]
  fn modify_workspace_manifest() {
    let _guard = SERIAL_TEST.lock();
    let tmp_dir = tempfile::tempdir().unwrap();

    let initial_crate_manifest = r#"
      [package]
      name = "test-crate"

      [dependencies]
      a = { workspace = true }
      b = { workspace = true }
    "#;

    let initial_workspace_manifest = r#"
      [workspace]
      resolver = "2"
      members = ["test-crate"]

      [workspace.dependencies]
      a = { package = "a", version = "0.1" }
      b = { package = "b", version = "0.1" }
    "#;

    let updated_workspace_manifest = r#"
      [workspace]
      resolver = "2"
      members = ["test-crate"]

      [workspace.dependencies]
      a = { package = "b", version = "0.1" }
      b = { package = "a", version = "0.1" }
    "#;

    let ((crate_manifest_path, _), workspace_return) = setup_fs_test(
      &tmp_dir,
      Some(initial_workspace_manifest),
      initial_crate_manifest,
    );
    let (workspace_manifest_path, mut workspace_manifest_file) = workspace_return.unwrap();

    // set env var
    #[expect(unsafe_code)]
    // SAFETY: The test is marked as serial, so it is safe to set the environment variable.
    unsafe {
      std::env::remove_var("CARGO_MANIFEST_PATH");
      std::env::set_var("CARGO_MANIFEST_DIR", crate_manifest_path.parent().unwrap());
    };
    let initial_mtime = get_mtime_from_path(&crate_manifest_path).unwrap();

    let cargo_manifest = CargoManifest::shared();

    // resolve the path for the initial workspace manifest
    let crate_to_resolve = "a";
    let expected_path = "::a".to_string();
    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );

    // update the workspace manifest
    workspace_manifest_file
      .seek(std::io::SeekFrom::Start(0))
      .unwrap();
    workspace_manifest_file
      .write_all(updated_workspace_manifest.as_bytes())
      .unwrap();
    // update the mtime
    std::thread::sleep(Duration::from_millis(10));
    workspace_manifest_file
      .set_modified(SystemTime::now())
      .unwrap();
    workspace_manifest_file.flush().unwrap();
    let new_mtime = get_mtime_from_path(&workspace_manifest_path).unwrap();
    drop(cargo_manifest);

    // sleep for the cache to expire
    std::thread::sleep(Duration::from_millis(
      CargoManifest::MTIME_CACHE_TIMEOUT_MS + 100,
    ));

    let cargo_manifest = CargoManifest::shared();

    // check that the mtime has changed
    assert_ne!(initial_mtime, new_mtime);

    // resolve the path for the updated workspace manifest
    let expected_path = "::b".to_string();
    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
    drop(cargo_manifest);

    // check it again
    let cargo_manifest = CargoManifest::shared();
    assert_eq!(
      pretty_format_syn_path(
        cargo_manifest
          .try_resolve_crate_path(crate_to_resolve, &[])
          .as_ref()
          .unwrap()
      ),
      expected_path
    );
    drop(cargo_manifest);
  }
}

#[cfg(all(feature = "nightly", not(feature = "proc-macro"), test))]
mod benches {
  use super::{shared_testing_benchmarking::setup_fs_test, *};
  use std::hint::black_box;
  extern crate test;
  use test::Bencher;

  #[bench]
  fn test_system_time_to_ms(b: &mut Bencher) {
    let now = SystemTime::now();
    b.iter(|| {
      let _ = black_box(system_time_to_ms(now));
    });
  }

  #[bench]
  fn get_mtime_from_path(b: &mut Bencher) {
    let tmp_dir = tempfile::tempdir().unwrap();
    let ((path, _), _) = setup_fs_test(&tmp_dir, None, "");
    b.iter(|| {
      let _ = black_box(super::get_mtime_from_path(&path));
    });
  }

  #[bench]
  fn get_mtime_from_path_cached(b: &mut Bencher) {
    let tmp_dir = tempfile::tempdir().unwrap();
    let ((path, _), _) = setup_fs_test(&tmp_dir, None, "");
    let last_cached_mtime = AtomicU64::new(system_time_to_ms(SystemTime::now()));
    let orginal_mtime_ms = 0_u64;
    b.iter(|| {
      let _ = black_box(CargoManifest::get_mtime_from_path_cached(
        &path,
        &last_cached_mtime,
        orginal_mtime_ms,
      ));
    });
  }
}
