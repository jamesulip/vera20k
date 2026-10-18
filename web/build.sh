#!/bin/sh
# Build the browser version of VERA20k into web/pkg.
# Needs: rustup target add wasm32-unknown-unknown
#        cargo install wasm-bindgen-cli --version 0.2.114 --locked  (must match Cargo.lock)
# Serve the web/ folder over HTTP afterwards, for example:
#        python3 -m http.server 8080 --directory web
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
cargo build --release --target wasm32-unknown-unknown --bin vera20k
wasm-bindgen --target web --no-typescript --out-dir web/pkg \
    target/wasm32-unknown-unknown/release/vera20k.wasm
echo "Built web/pkg. Serve web/ and open index.html in a WebGPU browser."
