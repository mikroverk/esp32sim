#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
. ./tools/wasm-rustflags.sh
RUSTC="$(rustup which rustc)"; export RUSTC
DYLD_FALLBACK_LIBRARY_PATH="$(dirname "$(dirname "$RUSTC")")/lib${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"; export DYLD_FALLBACK_LIBRARY_PATH
"$(rustup which cargo)" build --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests
node tools/wasm-jit-test.mjs

# Isolate generated-module metadata from the full differential suite.
node tools/wasm-jit-test.mjs gpio-waveform
