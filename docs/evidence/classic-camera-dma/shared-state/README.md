# Shared camera sensor and frame-state extraction

Base: `142805d5399e0c3cf25995fe21ba524e75a2b3cb`.

The OV5640 sensor, shared SCCB configuration, host picture, preview and converted-frame
cache now live in `esp-soc::camera`. The S3 board delegates to that model and retains
its wiring, GPIO count, preview-dirty flag and successful-frame count. The previous
`esp32s3::i2c::{Ov5640, SensorState}` paths remain re-exports of the same types.

This is a chip-independent extraction, not a sensor-neutral protocol: the register
format bytes, crop array and SCCB behavior still belong to OV5640. No OV2640, classic
ESP32 capture support, new board, JPEG support, PSRAM support or timing behavior is
introduced. LCD_CAM, GDMA, the board trait, bus layouts and step loops are unchanged.
There is no new crate dependency. The workspace dependency graph is acyclic and
classic ESP32 does not depend directly or transitively on S3.

## Preservation checks

`source-equivalence.json` records a byte-identical moved sensor implementation.
The three moved frame methods are identical after removing the board-owned counter
and preview-dirty update and making their visibility public. The board increments
its counter only for a returned frame, as the original successful path did. Picture
validation, black fallback, cache reuse/invalidation, streaming gates, unsupported
format diagnostics and reset behavior are preserved.

The standalone `camera_shared_model_needs_no_soc_and_preserves_frame_cache` test
constructs the shared model using only esp-soc and SCCB transactions. It pins black
fallback, exact RGB565/grayscale bytes, preview bytes, Arc cache identity, invalid
input preservation, equal-size host replacement, stop/resume, JPEG rejection and
sensor ID retention after reset. Existing S3 camera/DMA/input/reboot tests also pass.

Final required checks, with Rust 1.99.0 and `nice -n 10`:

- `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings`: PASS.
- `cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings`: PASS.
- Empty HOME, only ROM discovery supplied by `ESP32SIM_ROM_DIR`,
  `cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_`:
  986 passed, 0 failed, 22 filtered. Cargo/rustup homes retained explicitly for tools.
- Empty HOME, no `ESP32SIM_ROM_DIR`, `cargo +1.99.0 test --release --workspace`:
  958 passed, 0 failed, 50 ignored. The new shared-model test passes in both suites.
- `RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh` and
  `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel`:
  all eight demos and the harness's JIT handoff check PASS.
- `node tools/check-evidence-privacy.mjs`: PASS; `git diff --check`: PASS.
- Native release bins built on base and candidate. No JIT implementation changed;
  the separate JIT differential suite is not applicable.

The worktree's `target` was a temporary symlink to the caller-owned output directory
because `wasm-build.sh` copies from that relative path even with CARGO_TARGET_DIR set.
Demo assets were fetched in the candidate worktree with
`tools/fetch-demo-assets.sh --no-linux`; the base build needs no firmware inputs.
No firmware directory was copied from another checkout. No tracked golden
or firmware artifact is changed by this extraction.

## Mutations

All 11 independent mutations are assertion-killed, not compile failures. The first
two replay the historical camera-live SCCB reset/window cases at their new shared
path. The other nine remove configuration/picture cache invalidation, stream gating,
JPEG rejection, RGB565/YUYV/grayscale selection, black fallback or preview pixels.
`mutations.json` contains exact replacements and the tests that kill each one.
LCD_CAM/GDMA mutations are not repeated: those implementations are untouched.

Reproduce in a disposable worktree, with output outside the source tree:

```
python3 docs/evidence/classic-camera-dma/shared-state/mutate.py "$PWD" "$OUTPUT"
```

The helper always restores the source, even on a failed assertion. Its baseline
runs `cargo +1.99.0 test -p esp-soc -p esp32s3 --lib camera` before the mutations.

## Existing S3 firmware fixture

The unchanged public `camera-start-2026-10-02` fixture and its pinned input manifest
were used; all three supplied flash inputs and the ROM match that manifest's SHA-256.
The existing `external_camera.py` drives 50 simulated seconds, including RGB565,
YUYV and reinitialization. Both revisions return the expected seven 96x96 frames:
four RGB565 frames with FNV-1a `7bf49dc5`, three YUYV frames with `8c2afdc5`.
These are guest-computed frame hashes checked against independently generated
solid-red bytes by the inherited harness, not an image rendered by the host UI.

Both serial streams are byte-identical, SHA-256
`b0011b04b138834fd6d6697ed6a9cfd328065f5af87b1a68d7eb146f6ba01bd2`.
Core instruction counts are 94,647,530 and 165,914,655; the 12,000,000,000-cycle stop
has 1,006,137 exceptions and 150,684 interrupts on both revisions. Stderr is identical
only after normalizing image/ROM paths and host wall time/throughput; instruction,
cycle, exception, total interrupt and per-source interrupt counts are retained.
`fixture-comparison.json` records inputs, hashes, serial lines and normalization.

Build base bins into `$OUTPUT/base-target` and candidate bins into `$OUTPUT/target`
with `cargo +1.99.0 build --release --bins`. With the already-built fixture inputs:

```
python3 docs/evidence/classic-camera-dma/shared-state/fixture.py "$PWD" "$OUTPUT" "$FIRMWARE"
```

`FIRMWARE` supplies the three CameraFixture flash files from the pinned manifest;
the recipe and limitations remain in `camera-start-2026-10-02`. This is an external
qualification fixture, not a newly committed CI firmware fixture or hardware proof.

## Hot-path code generation

`codegen.json` compares the three release CLI binaries using the existing EX230
`classic-core/asmcmp.py` normalizer. All 216 selected functions are identical
(72 per binary, including classic, S3, C3 and C6 machine run loops and selected
bus/core functions). Addresses, page relocations, alignment and equivalent call
aliases are normalized; register/stack/field offsets and instruction order remain
compared. This is a selected-function check, not whole-binary equivalence.
No new polling, cadence, per-tick field or step-loop call is introduced.

```
python3 docs/evidence/classic-camera-dma/shared-state/codegen.py "$PWD" "$OUTPUT"
```

The helper records binary/disassembly hashes and the precise selection regex.
Large binaries and raw disassemblies are disposable; compact receipts and helpers
are retained here. No performance conclusion is inferred from fixture wall time.

## CPU comparison

PENDING. No formal CPU timing table was run for this refactor.
