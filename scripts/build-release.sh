#!/bin/sh
set -eu

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_root"

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release --locked -p atlasfs-cli
sha256sum target/release/atlas >target/release/atlas.sha256
printf 'Release artifact: %s\n' "$project_root/target/release/atlas"
