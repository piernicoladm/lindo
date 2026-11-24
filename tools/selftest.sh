#!/bin/zsh
set -euo pipefail
here=${0:A:h:h}
export PATH="$PATH:$HOME/.cargo/bin"
export CARGO_TARGET_DIR="$HOME/Library/Developer/Lindo/rust-target"
cargo test --locked --manifest-path "$here/Cargo.toml" "$@"
