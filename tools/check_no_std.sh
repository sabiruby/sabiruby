#!/bin/bash
# Enforces the no_std rule: the libraries that say `#![no_std]` must build for a target that has
# no `std`. That is the VM (the arguments, e.g. `--features regexp`, are its features) and
# sabiruby-serde without its default `json` (serde_json's `preserve_order` brings `std`; the
# conversion layer is `no_std` + alloc like the VM, `serde/Cargo.toml`).
#   rustup target add thumbv7em-none-eabi   (once)
set -eu
cd "$(dirname "$0")/.."
cargo build --lib --no-default-features --target thumbv7em-none-eabi "$@"
cargo build -p sabiruby-serde --lib --no-default-features --target thumbv7em-none-eabi
if grep -rn "std::" src serde/src --include=*.rs | grep -vE "^[^:]+:[0-9]+:\s*//"; then
  echo "error: std:: path in a no_std library (use core::/alloc::)" >&2; exit 1
fi
echo "no_std OK"
