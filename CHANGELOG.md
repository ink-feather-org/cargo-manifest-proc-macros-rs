# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/ink-feather-org/trait-cast-rs/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/ink-feather-org/trait-cast-rs/releases/tag/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ink-feather-org/trait-cast-rs/releases/tag/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ink-feather-org/trait-cast-rs/releases/tag/v0.0.1...v0.1.0
[0.0.1]: https://github.com/ink-feather-org/trait-cast-rs/releases/tag/v0.0.1
