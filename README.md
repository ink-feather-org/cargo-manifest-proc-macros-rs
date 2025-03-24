# cargo-manifest-proc-macros

[![Daily-Nightly](https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/actions/workflows/rust_daily_nightly_check.yml/badge.svg)](https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/actions/workflows/rust_daily_nightly_check.yml)
[![Rust-Main-CI](https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/actions/workflows/rust_main.yml/badge.svg)](https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/actions/workflows/rust_main.yml)
[![docs.rs](https://docs.rs/cargo-manifest-proc-macros/badge.svg)](https://docs.rs/cargo-manifest-proc-macros)
[![crates.io](https://img.shields.io/crates/v/cargo-manifest-proc-macros.svg)](https://crates.io/crates/cargo-manifest-proc-macros)
[![rustc](https://img.shields.io/badge/rustc-stable-lightgrey)](https://doc.rust-lang.org/stable/std/)

## What can this crate do?

`cargo-manifest-proc-macros` is a library for creating proc-macros.
It provides a reliably way to compute the [`syn::Path`](https://docs.rs/syn/latest/syn/struct.Path.html) to other crates.

## How to use this crate?

Proc-macro crates are usually tightly coupled to other crates and need to know the module path to them.

Given the following common crate structure:

* `my-awesome-crate-proc-macros` has the proc-macro implementations of `my-awesome-crate`
* `my-awesome-crate` depends on `my-awesome-crate-proc-macros` and exports the macros in `my-awesome-crate-proc-macros`.
* `my-awesome-super-crate` depends on and re-exports `my-awesome-crate`. This is called a `KnownReExportingCrate`.

There are multiple ways users can gain access to the proc-macros and your other crates:

```toml
[dependencies]
my-awesome-crate = "0.1.0" # The classic and easy way
my-awesome-crate-renamed = { version = "0.1.0", package = "my-awesome-crate" } # The renamed way
my-awesome-super-crate = "0.1.0" # The re-exporting way
my-awesome-super-crate-renamed = { version = "0.1.0", package = "my-awesome-super-crate" } # The renamed re-exporting way
```

The [`CargoManifest`] struct is capable of computing the [`syn::Path`] to the crates in the all of the above cases.

## Example (non-re-exporting)

```rust
# #[cfg(feature = "proc-macro")]
# mod _mod {
extern crate proc_macro;

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};
use cargo_manifest_proc_macros::CargoManifest;

//#[proc_macro_derive(YourProcMacro, attributes(your_attributes))]
pub fn derive_your_proc_macro(input: TokenStream) -> TokenStream {
  let mut ast = parse_macro_input!(input as DeriveInput);
  // The [`CargoManifest::resolve_crate_path`] returns a global [`syn::Path`] to the crate no matter how it is depended on.
  let path_to_your_crate: syn::Path = CargoManifest::shared().resolve_crate_path("my-awesome-crate", &[]);
  TokenStream::default()
}
# }
```

## Example (re-exporting)

In this example the user may depend on either `my-awesome-super-crate` or `my-awesome-crate` to gain access to the proc-macros and features of `my-awesome-crate`.

```rust
# #[cfg(feature = "proc-macro")]
# mod _mod {
#![cfg(feature = "proc-macro")]
extern crate proc_macro;

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};
use cargo_manifest_proc_macros::{CargoManifest, CrateReExportingPolicy, KnownReExportingCrate, PathPiece};
use proc_macro2::Span;

struct SuperCrateReExportingPolicy {}

impl CrateReExportingPolicy for SuperCrateReExportingPolicy {
  fn get_re_exported_crate_path(
    &self,
    crate_name: &str,
  ) -> Option<PathPiece> {
    // Since crates can be re-exported with arbitrary names, we may need to transform the crate name to the re-exported name.
    let export_name = crate_name.strip_suffix("_crate"); // This would handle the case of `my-awesome-crate` being re-exported as just `my-awesome` by `my-awesome-super-crate`.
    export_name.map(|export_name| {
      let mut path_piece = PathPiece::new();
      path_piece.push(syn::PathSegment::from(syn::Ident::new(
          export_name,
          Span::call_site(),
        )));
      path_piece
    })
  }
}

/// We need to associate the re-exporting policy with the re-exporting crate name.
const SUPER_RE_EXPORTER_RESOLVER: KnownReExportingCrate<'_> = KnownReExportingCrate {
  re_exporting_crate_package_name: "my-awesome-super-crate",
  crate_re_exporting_policy: &SuperCrateReExportingPolicy {},
};

/// In case there are multiple re-exporting crates, we can list them here.
const KNOWN_RE_EXPORTING_CRATES: &[&KnownReExportingCrate<'_>] = &[
  &SUPER_RE_EXPORTER_RESOLVER,
];

//#[proc_macro_derive(YourProcMacro, attributes(your_attributes))]
pub fn derive_your_proc_macro(input: TokenStream) -> TokenStream {
  let mut ast = parse_macro_input!(input as DeriveInput);
  // The [`CargoManifest::resolve_crate_path`] returns a global [`syn::Path`] to the crate no matter how it is depended on.
  let path_to_your_crate: syn::Path = CargoManifest::shared().resolve_crate_path("my-awesome-crate", KNOWN_RE_EXPORTING_CRATES);
  TokenStream::default()
}
# }
```

## Features

* `nightly` - Enables nightly features. This requires a nightly compiler.

## License

This project is released under either:

- [MIT License](https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/blob/main/LICENSE-MIT)
- [Apache License (Version 2.0)](https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/blob/main/LICENSE-APACHE)

at your choosing.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

## How it works

This crate works by parsing the `$CARGO_MANIFEST_DIR/Cargo.toml` file and extracting the dependencies from it.
It then uses the `package` field in the `Cargo.toml` to resolve the path to the crate.

### Links

[`syn`](https://crates.io/crates/syn)

[`proc-macro-crate`](https://crates.io/crates/proc-macro-crate)

[`find-crate`](https://crates.io/crates/find-crate)
