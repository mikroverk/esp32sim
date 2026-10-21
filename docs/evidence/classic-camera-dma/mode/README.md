# Classic I2S0 camera register adapter

This is a register-only prerequisite, based on
`eae1ba46bb7c19fcb457120fc66766acfcd52bdd`. It does not implement camera
capture and does not make an ESP32-CAM sketch work yet.

## Public source contract

`verify-sources.py` downloads immutable public files and checks their SHA-256,
ten register offsets, nineteen bits and three shifts. `sources.json` pins:

- ESP-IDF v4.4.7, commit `38eeba213aa695aabfd6d89aa9f5078dbe5a94c3`,
  `components/soc/esp32/include/soc/i2s_reg.h` and `i2s_struct.h`.
- esp32-camera v2.1.4, commit
  `b817e9b81041980608d1a3d3e0b08d86e3666382`, `target/esp32/ll_cam.c`.

These are header/driver-derived semantics, not a hardware qualification or a
claim about which camera library version any application uses. IDF v4.4.7 is
pinned for this trace; existing audio code keeps its IDF v5.5.4 references.

The driver's config/start/stop writes are replayed as volatile-style RMW
operations through the classic bus. CONF2 is at 0xa8 (LCD_EN bit 5,
CAMERA_EN bit 0), not the shared S3 I2S layout. CONF RX_RESET and RX_FIFO_RESET
are bits 1 and 3, not TX bits 0 and 2. LC_CONF input/AHB FIFO/AHB resets are
bits 0, 2 and 3, not the output-reset bit 1. Reset bits remain plain readable
R/W bits. The adapter exposes reset-held/start-request state; future FIFO and
DMA integrations must use reset writes to discard their own pending work.

Only controller 0 recognizes both camera-mode bits. Its parallel receiver
configuration exposes FIFO mode, descriptor and forced-mode enables, slave,
start and reset state independently of PCM clocks. RX_BITS_MOD=0 is valid
for this path. The audio producer is stopped and its phase cleared on camera
register synchronization, even if the old audio clocks/width would otherwise
be valid. Disabling camera mode resynchronizes PCM from the retained audio
registers. Controller 1 continues to interpret its registers as audio.

## Tests and semantic mutations

Four camera-specific tests supplement the six existing classic I2S tests:

- `camera_driver_config_start_stop_trace`: config and capture-arm/stop trace
  for FIFO modes 1 and 3, zero RX_BITS_MOD, EOF enable/disable and word-count
  readback. It asserts decoded configuration and no active PCM producer.
- `camera_requires_both_mode_bits_on_port_zero_only`: both bits are required;
  actual bus-created controller 1 never selects camera mode.
- `camera_reset_bits_and_fifo_fields_are_live`: all five RX-relevant reset
  bits gate start, TX-only resets do not, reset bits read back, FIFO fields
  and slave mode track live writes.
- `camera_bypasses_audio_and_mode_exit_restores_pcm`: camera stays configured
  with invalid audio width/dividers; valid audio settings cannot emit PCM in
  camera mode; returning to audio starts with fresh phase. Controller 1's
  rate, fractional phase and exact PCM bytes are preserved.

`mutate.py REPO OUTPUT` runs a passing baseline and one semantic edit at a
time, restores the source in `finally`, and requires named assertion failures
rather than compile failures. `mutations.json` contains the replacements and
actual killing tests. FIFO mode validity/packing is deliberately left to the
future packer: decoding a mode here does not assert support for transporting it.

## Reproduction

Use an isolated worktree and a caller-owned CARGO_TARGET_DIR. All Cargo
commands below use `nice -n 10 cargo +1.99.0`:

    tools/fetch-demo-assets.sh --no-linux
    cargo +1.99.0 clippy --workspace --all-targets -- -D warnings
    cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings
    cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
    cargo +1.99.0 test --release --workspace
    RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh
    node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel
    python3 docs/evidence/classic-camera-dma/mode/verify-sources.py
    python3 docs/evidence/classic-camera-dma/mode/mutate.py REPO OUTPUT
    node tools/check-evidence-privacy.mjs
    git diff --check

Both workspace test commands run with an empty HOME and explicit CARGO_HOME /
RUSTUP_HOME for tooling only. The first sets ESP32SIM_ROM_DIR to the fetched
firmware directory; the plain run unsets it. `wasm-build.sh` copies from a
literal `target/` path, so link the worktree's `target` to the same isolated
CARGO_TARGET_DIR before running it. No JIT source was changed.

`checks.json` records actual outcomes and test totals. No non-camera golden
file is changed; the ROM-enabled golden suite and all eight WASM demos pass.
No golden regeneration is used.

## Idle path and hot code

No camera checks were added to `data`, `i2s_step`, bus `tick`, or machine
scheduler loops. Camera mode is recognized during MMIO synchronization only;
the read-only configuration decoder is for the later camera integration.
The controller-identity flag adds no field-offset or array-stride changes in
the measured hot code; the comparison preserves those offsets and immediates.

Build `esp32sim` with identical release settings in this tree and in a detached
base worktree, using OUTPUT/target and OUTPUT/base-target respectively, then:

    python3 docs/evidence/classic-camera-dma/mode/codegen.py REPO OUTPUT

The script reuses the existing classic-core address/page/call-alias normalizer.
`codegen.json` records binary/disassembly hashes and the compared functions.
The classic I2S step is inlined in `SocBus::tick`, not a separate symbol.
All five selected classic tick/machine function bodies are identical, including
2207 instructions in tick, 84 in after_round, 3445 in run_modeled, 1708 in
run_unmodeled and 838 in step_core. This is selected native codegen equivalence,
not whole-program equivalence or a CPU-time measurement.

## CPU comparison

PENDING. No timing or benchmark run is performed on this card.

## Limits

No sensor, board, pixel FIFO packing, camera DMA delivery, GPIO/VSYNC, PSRAM,
JPEG, firmware fixture or product-support gate is implemented. The driver
trace is a native register test, not an actual compiled guest camera driver.
LC_CONF reset state is decoded here, but resetting in-flight descriptors and
partial pixel words belongs to the later DMA/packing integration. The existing
audio DMA walker and interrupt mapping are unchanged. No new dependency,
publication, upstream issue or pull request is introduced.
