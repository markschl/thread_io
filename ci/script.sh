#!/bin/sh

set -ex

cargo build --verbose
cargo doc --verbose
cargo test --verbose
cargo test --features crossbeam_channel --verbose

if [ "$TRAVIS_RUST_VERSION" = "stable" ]; then
  rustup component add rustfmt
  cargo fmt -- --check
fi
if [ "$TRAVIS_RUST_VERSION" = "nightly" ]; then
  cargo bench --verbose --no-run
fi
