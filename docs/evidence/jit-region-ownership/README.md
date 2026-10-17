# Retained JIT region ownership

## Result

Reproduced on main `142805d5399e0c3cf25995fe21ba524e75a2b3cb`, with only
regression test `3c395f5652480f1d2e71c4ebb117c8ec631999fc` added.
`RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-jit-test.sh` exits 1:

```text
panicked at xtensa-lx7/src/jit/wasm.rs:186:9:
assertion `left == right` failed
  left: (1142, 1, 2, 0)
 right: (1143, 1, 2, 0)
```

The synthetic test explicitly seeds two overlapping compiled regions and a live
covering hint, runs the same successor twice, then changes region generations and
resets the cache to force index permutation. This isolates cache ownership; it is
not a firmware reboot or a hardware observation. No firmware input is needed.
This backend and its test are WASM-only; native AArch64 uses a different cache.

## Explanation and fix

The coverage map is a preference for new lookups, not exclusive region ownership.
The slow lookup may retain a live overlapping region in `covered_by`. Hot validation
must compare with that selected owner and chunk, not a new map lookup. All existing
Hot field comparisons remain, and the selected chunk's PC is checked explicitly.

Separately, reset sorts and compacts retained blocks. Old owner indices can then
name another valid overlapping region, passing the slow path's live-PC check.
Discard the hints when rebuilding the maps, so the next lookup selects from the
rebuilt map. This adds work only to cache turnover, not normal dispatch.

## Checks

All builds used Rust 1.99.0 and `nice -n 10`, with one separate target directory.
These commands all exit 0 on the fixed source:

```sh
cargo +1.99.0 clippy --workspace --all-targets -- -D warnings
cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
cargo +1.99.0 test --release --workspace
RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh
node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel
RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-jit-test.sh
node tools/check-evidence-privacy.mjs
git diff --check
```

Assets were fetched with `tools/fetch-demo-assets.sh --no-linux`. Both native test
commands ran with an empty HOME, explicit toolchain locations, and no ESP32SIM
settings except ROM_DIR for the first run: 985 passed with ROMs; 957 passed and 50
ignored without ROM_DIR. Totals include doc tests. WASM: 111126 differential cases,
99679 compiled modules released; 24 waveform cases, 738 modules released. All eight
requested demo scenarios and the JIT handoff check pass; no goldens changed.

## Mutation checks

Each mutation was applied independently to fixed source, rebuilt with `jit-tests`,
and killed by calling `esp32sim_test_region_ownership` through the normal JIT host:

| Mutation | Outcome |
| --- | --- |
| Validate the map's preferred owner rather than the selected hint | Exit 1 at Hot field comparison on second overlapping-owner dispatch |
| Omit clearing `covered_by` in reset | Exit 1: `reset must discard owner indices before rebuilding coverage`; actual `(1, 1)`, expected `(4294967295, 0)` |

The same regression is part of `tools/wasm-jit-test.sh`. Restoring both rules and
rerunning the focused export passes: one ownership case, two compiled modules,
two released, zero failed compilations.

## Production code generation

Identical production build settings, with `jit-tests` disabled. The baseline is
main plus the regression test, which is excluded from production by its feature
gate. Of 2113 named function bodies, 2112 are byte-identical; no functions added or
removed. Only `CodeCache::reset` changes, from 6502 to 6514 bytes. Both chip-specific
`run_inner` functions and their two named closures have identical WASM disassembly.
The extra reset instructions discard the old hints; selected-owner validation is
compiled out of production release builds. Native AArch64 source is unchanged.

| Artifact | SHA-256 |
| --- | --- |
| Baseline production WASM | `948526030c6bffee4e6389c520fdba52bf2cd992b6dce9d03e12bffd13a3ad60` |
| Fixed production WASM | `e9b09a413a28036fc9609e67cb8e8d035e64bb4f6eb9aafbfd448b7203b49d1c` |

## CPU comparison

PENDING

No CPU timing comparison was run and no speedup is claimed.
