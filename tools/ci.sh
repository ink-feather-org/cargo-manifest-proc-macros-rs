#!/bin/bash
set -euo pipefail

# Function to install and set Rust toolchain
setup_toolchain() {
  local toolchain=$1
  echo "Installing and setting up Rust toolchain: $toolchain"
  rustup install "$toolchain" || true
  rustup override set "$toolchain"
}

# Function to extract MSRV from Cargo metadata
get_msrv() {
  echo "Extracting the MSRV (Minimum Supported Rust Version)..."
  local msrv=$(cargo metadata --no-deps --format-version=1 | jq -r '.packages[0].rust_version // empty')
  if [[ -z "$msrv" ]]; then
    echo "Error: Couldn't determine MSRV from Cargo.toml (rust-version)."
    exit 1
  fi
  echo "MSRV is $msrv"
  export MSRV=$msrv
}

# Function to extract all available features, excluding nightly if not using nightly
get_features() {
  echo "Extracting available features..."
  local toolchain=$1
  local features=$(cargo metadata --no-deps --format-version=1 | jq -r '.packages[0].features | keys | join(",")')
  if [[ -z "$features" ]]; then
    echo "No additional features found."
    export CUSTOM_FEATURES=""
    return
  fi

  if [[ "$toolchain" != "nightly" ]]; then
    # Remove nightly feature if it exists
    features=$(echo "$features" | sed 's/,nightly,/,/; s/^nightly,//; s/,nightly$//')
  fi

  echo "Available features for toolchain $toolchain: $features"
  export CUSTOM_FEATURES=$features
}

# Function to wrap cargo commands and print them before executing
run_cargo() {
  echo "> cargo $*"
  cargo "$@"
}

# Function to run validation steps
run_steps() {
  echo "Running validation steps for toolchain: $(rustc --version)"

  # Use --features when calling `cargo` if CUSTOM_FEATURES is not empty
  local features_flag=""
  if [[ -n "$CUSTOM_FEATURES" ]]; then
    features_flag="--features $CUSTOM_FEATURES"
  fi

  echo "Performing build"
  run_cargo +"$RV" build --verbose

  echo "Performing build --no-default-features"
  run_cargo +"$RV" build --verbose --no-default-features

  echo "Performing build (all features)"
  run_cargo +"$RV" build --verbose $features_flag --all-targets

  echo "Running tests"
  run_cargo +"$RV" test --verbose

  echo "Running tests --no-default-features"
  run_cargo +"$RV" test --verbose --no-default-features

  echo "Running tests (all features)"
  run_cargo +"$RV" test --verbose $features_flag --all-targets
}

# Ensure required tools are installed
if ! command -v cargo &>/dev/null; then
  echo "Error: Cargo is not installed."
  exit 1
fi

if ! command -v jq &>/dev/null; then
  echo "Error: jq is not installed. Install it to parse JSON data."
  exit 1
fi

echo "Setting up the required toolchains"
get_msrv # Fetch MSRV from Cargo.toml
setup_toolchain "$MSRV"
setup_toolchain "stable"
setup_toolchain "nightly"

# Main script execution
echo "Starting CI validation..."

echo "Checking format"
run_cargo +nightly fmt --all --check

echo "Running Clippy (default features)"
run_cargo +nightly clippy --workspace --all-targets -- -D warnings

echo "Running Clippy (all features)"
run_cargo +nightly clippy --workspace --all-features --all-targets -- -D warnings

# Run on MSRV
export RV=$MSRV
get_features "$MSRV"
run_steps

# Run on stable
export RV=stable
get_features "stable"
run_steps

# Run on nightly
export RV=nightly
get_features "nightly"
run_steps

echo "CI validation completed successfully for MSRV, stable, and nightly!"
