# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.4] - 2025-02-11

* Fix false positives in the crate path ambiguity detection.
* Add more tests.
* Update dependencies.

## [0.3.3] - 2025-01-19

* Add support for single file rust scripts that use `CARGO_MANIFEST_PATH`.

## [0.3.2] - 2025-01-18

* Relax rust edition to 2021 for bevy.
* Support bevy's minimum supported rust version.

## [0.3.1] - 2025-01-12

* Properly reload the workspace when the `Cargo.toml` file changes.

## [0.3.0] - 2025-01-02

* Added support for stable rust compilers.
* Extended the CI.

## [0.2.2] - 2025-01-02

* Fixed a deadlock.

## [0.2.1] - 2025-01-01

* Fixed a bug where rust analyser would resolve the crate paths incorrectly due to caching of `'static` lifetimes.
  See [rust-analyzer#18798](https://github.com/rust-lang/rust-analyzer/issues/18798) and [bevy#17004](https://github.com/bevyengine/bevy/issues/17004).

## [0.2.0] - 2024-12-23

* Added proper support for workspace dependencies.
* Added support for target-specific dependencies.
* Added a proper test suite.

## [0.1.0] - 2024-12-19

Support reactive compilation using `proc_macro_tracked_env` and `track_path` nightly features.

## [0.0.1] - 2024-12-19

Initial release.

[Unreleased]: https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/compare/v0.3.4...HEAD
[0.3.4]: https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/ink-feather-org/cargo-manifest-proc-macros-rs/releases/tag/v0.0.1
