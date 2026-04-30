#!/usr/bin/env bash
# Cross-compile dropserve for x86_64 Linux (glibc) from macOS or other hosts.
# Prerequisites:
#   rustup target add x86_64-unknown-linux-gnu
#   brew install zig          # or install Zig from https://ziglang.org
#   cargo install cargo-zigbuild
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if ! command -v zig >/dev/null; then
  echo "zig not found; install with: brew install zig" >&2
  exit 1
fi
if ! command -v cargo-zigbuild >/dev/null; then
  echo "cargo-zigbuild not found; install with: cargo install cargo-zigbuild" >&2
  exit 1
fi

rustup target add x86_64-unknown-linux-gnu

cargo zigbuild --release --bin dropserve --target x86_64-unknown-linux-gnu

echo "Built: ${root}/target/x86_64-unknown-linux-gnu/release/dropserve"
