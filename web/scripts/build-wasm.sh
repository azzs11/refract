#!/bin/sh
# Build refract-core for the browser: web/src/wasm/ (generated, gitignored).
# Needs: rustup target add wasm32-unknown-unknown; cargo install wasm-bindgen-cli --version 0.2.130
set -eu
cd "$(dirname "$0")/../.."
cargo build -p refract-wasm --target wasm32-unknown-unknown --profile wasm
wasm-bindgen --target web --out-dir web/src/wasm --no-typescript \
  target/wasm32-unknown-unknown/wasm/refract_wasm.wasm
ls -l web/src/wasm/refract_wasm_bg.wasm
