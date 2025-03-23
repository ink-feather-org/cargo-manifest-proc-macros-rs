# Useful commands

```bash
# run the benchmarks
cargo bench --package cargo-manifest-proc-macros --lib --features nightly --no-default-features -- toml_strip::benches::strip_and_parse toml_strip::benches::parse_only --exact --show-output
cargo bench --no-default-features --features "nightly" --bench single_threaded_random_lookups --bench multi_threaded_random_lookups
# benchmark inline benchmarks
cargo bench --package cargo-manifest-proc-macros --lib --features nightly --no-default-features -- cargo_manifest::benches::get_mtime_from_path cargo_manifest::benches::get_mtime_from_path_cached --exact --show-output
# compute a flamegraph for a benchmark
RUSTFLAGS="-Clink-arg=-fuse-ld=lld -Clink-arg=-Wl,--no-rosegment" CARGO_PROFILE_RELEASE_DEBUG=true cargo +nightly flamegraph --no-default-features --features "nightly" --bench single_threaded_random_lookups
RUSTFLAGS="-Clink-arg=-fuse-ld=lld -Clink-arg=-Wl,--no-rosegment" CARGO_PROFILE_RELEASE_DEBUG=true cargo +nightly flamegraph --no-default-features --features "nightly" --bench fixed_single_threaded_random_lookups
# run the ci
./tools/ci.sh
```

## Using samply

```bash
cargo install samply
echo '1' | sudo tee /proc/sys/kernel/perf_event_paranoid
RUSTFLAGS="-Clink-arg=-fuse-ld=lld -Clink-arg=-Wl,--no-rosegment" CARGO_PROFILE_RELEASE_DEBUG=true cargo +nightly build --release --no-default-features --features "nightly" --bench single_threaded_random_lookups
RUSTFLAGS="-Clink-arg=-fuse-ld=lld -Clink-arg=-Wl,--no-rosegment" CARGO_PROFILE_RELEASE_DEBUG=true cargo +nightly build --release --no-default-features --features "nightly" --message-format json -q --bench single_threaded_random_lookups
```

## Run fs tests

```bash
cargo +nightly test --no-default-features --features "nightly"
```
