# AI-Thinker OV2640 board and host frame source

Local-only C3 feature slice, based on C2 `935da1221bce40e60581895a9e11f0f268c122ad`.
No classic guest camera capture is claimed. No publication or CPU timing was performed.

## Contract and implementation

- `ai-thinker-cam` / `esp32cam` select the fixed AI-Thinker board. Its only attached
  I2C device is OV2640, controller 0 address 0x30, physically qualified SDA26/SCL27.
- PWDN32 must explicitly be output-enabled and low. High or released PWDN withholds
  frames and SCCB ACK. This floating-pin policy is conservative, not a measured pull default.
  There is no reset wire (`Option::None`, corresponding to public pin -1), never GPIO0/255.
- The existing shared OV2640 register state controls dimensions, stream enable and
  raw RGB565/YUV422 output. Grayscale requests remain two-byte YU/YV on the wire.
  JPEG/raw/alternate byte-order IMAGE_MODE values are rejected rather than fabricated.
- `HostFrameSource` is extracted from the existing OV5640 camera, so both boards use
  one validated Picture/preview/conversion/cache implementation. Its cache key includes
  dimensions, format and optional sensor-array window; valid host replacement invalidates
  it, invalid host input preserves the current image. Black fallback remains available.
- Classic `SocBus::camera_frame()` lazily supplies the existing routed LEDC PWM query
  through `BoardModel::camera_frame_with_clock`. A finite positive frequency and a
  nonconstant duty on GPIO0 are required. Routing, IO_MUX, timer pause, output-enable
  and DPORT clock gating remain owned by existing GPIO/LEDC models, not duplicated.
  Inverted PWM is still a clock. No per-PCLK stepping, tick polling or hot struct fields
  are introduced. The default clock-aware board hook preserves existing boards and is
  appended after the existing methods to preserve hot vtable call offsets.

## Public sources

`sources.json` pins immutable URLs, SHA-256 and lengths; `sources.py OUTPUT` fetched them.

- Arduino-ESP32 2.0.17, commit `5e19e086c43d0fa5e5a596497ff8f11a0a43f6c2`,
  `libraries/ESP32/examples/Camera/CameraWebServer/camera_pins.h:139-156`:
  AI-Thinker SCCB/PWDN/XCLK and no-reset wiring.
- esp32-camera v2.1.4, commit `b817e9b81041980608d1a3d3e0b08d86e3666382`,
  `sensors/private_include/ov2640_settings.h:399-416`: YUV422/RGB565 register sequences;
  `target/esp32/ll_cam.c:501-537`: OV2640 grayscale consumes YU/YV, not sensor Y8.
- MMIO setup in the board tests uses the already qualified ESP-IDF v5.5.4 classic
  GPIO/IO_MUX/LEDC models and cites their headers in the test. No newly decoded register
  fields are added. No hardware verification or sensor ISP fidelity claim.

## Checks actually run

All Cargo commands use `+1.99.0`, all executions use `nice -n 10`, and build outputs are
isolated from the control checkout. `qualify.py TREE OUTPUT` records exact check commands.
It uses an empty HOME with explicit toolchain/cache locations: the ROM run has only
ESP32SIM_ROM_DIR supplied as emulator input; the plain run has no ROM environment.

| Command | Result |
| --- | --- |
| `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings` | PASS |
| `ESP32SIM_ROM_DIR=<tree>/web/wasm/fw cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_` | 1000 passed, 0 failed, 22 filtered |
| `cargo +1.99.0 test --release --workspace` | 972 passed, 0 failed, 50 ignored |
| `RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh` | PASS |
| `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel` | Eight demos and built-in JIT handoff PASS |
| `node tools/check-evidence-privacy.mjs` | PASS, including staged new evidence |
| `cargo +1.99.0 build --release --bins` on base and candidate | PASS |
| `git diff --check` | PASS |

No JIT source changed, so the separate JIT differential suite is inapplicable.
`tools/fetch-demo-assets.sh --no-linux` fetched assets in this worktree; none were copied
from another checkout. All 55 tracked golden files remain byte-identical (`goldens.json`),
and the ROM-enabled native golden tests pass without regeneration.

Seven new board integration tests exercise selection, pin-qualified SCCB transactions,
cache reuse/replacement/invalid input, register updates, grayscale wire bytes, PWDN,
standby/DVP reset/bypass, no-reset wiring and actual routed 10 MHz LEDC XCLK. These are
native model tests, not an esp_camera_init/guest frame-buffer acceptance fixture.

## Mutation table

`CARGO_TARGET_DIR=<output>/target python3 docs/evidence/classic-camera-dma/board/mutations.py TREE OUTPUT`
produces 21/21 named assertion kills. Each run selects exactly one test; compile failures
or missing/empty test runs do not count. Exact replacements and results are in `mutations.json`.
Names below omit the common `aithinker_` test prefix.

| Mutation | Test that fails |
| --- | --- |
| board-selection | selection_and_only_sensor_attachment |
| no-reset-sentinel | selection_and_only_sensor_attachment |
| sccb-sda | sccb_requires_both_physical_wires |
| sccb-scl | sccb_requires_both_physical_wires |
| sccb-unqualified | sccb_requires_both_physical_wires |
| sccb-power | pwdn_stop_and_reset_withhold_frames |
| frame-power | pwdn_stop_and_reset_withhold_frames |
| pwdn-release | pwdn_stop_and_reset_withhold_frames |
| stream-stop | pwdn_stop_and_reset_withhold_frames |
| xclk-route | xclk_requires_live_routed_pwm_not_gpio_level |
| xclk-pin | xclk_requires_live_routed_pwm_not_gpio_level |
| xclk-finite | xclk_rejects_constant_or_invalid_clock |
| xclk-positive | xclk_rejects_constant_or_invalid_clock |
| xclk-duty-zero | xclk_rejects_constant_or_invalid_clock |
| xclk-duty-full | xclk_rejects_constant_or_invalid_clock |
| grayscale-wire | grayscale_requests_keep_yuv_wire_bytes |
| rgb565-wire | host_and_register_updates_invalidate_shared_cache |
| unsupported-jpeg | host_and_register_updates_invalidate_shared_cache |
| picture-validation | host_and_register_updates_invalidate_shared_cache |
| picture-invalidation | host_and_register_updates_invalidate_shared_cache |
| register-invalidation | host_and_register_updates_invalidate_shared_cache |

`python3 docs/evidence/classic-camera-dma/shared-state/mutate.py TREE OUTPUT` also passes its baseline
and kills all 11 historical shared-camera/S3 mutations (`s3-mutations.json`). Its cache
mutation snippets are updated for the shared host-source extraction; no historical
receipt is relabeled as a fresh result.

## S3 control and idle code generation

`python3 docs/evidence/classic-camera-dma/shared-state/fixture.py TREE OUTPUT FIRMWARE` runs the existing
external public OV5640 fixture against base and candidate. Firmware/ROM hashes are checked
against the existing public manifest. Seven frames match: four RGB565 `7bf49dc5`, three
YUYV `8c2afdc5`. Serial SHA-256 is
`b0011b04b138834fd6d6697ed6a9cfd328065f5af87b1a68d7eb146f6ba01bd2` on both.
Stderr also matches after path and wall-time/throughput normalization only; instruction,
cycle, exception and interrupt counts remain intact (`fixture-comparison.json`).

`python3 docs/evidence/classic-camera-dma/board/codegen.py TREE BASE_BINARY CANDIDATE_BINARY OUTPUT`
compares release classic hot functions in the main CLI binary using the existing EX230
normalizer. All 21 selected instances match C2, with zero differences or missing symbols.
The selector, normalized instruction counts and artifact/disassembly hashes are recorded
in `codegen.json`. This normalizes addresses/page relocations/equivalent call aliases,
not instruction/register/field/vtable offsets. It is not entire-binary equivalence.

## Limits

This board/source slice does not implement I2S camera packing, DMA, DVP input routing,
VSYNC scheduling, PSRAM, JPEG encoding, capture cadence, product support gates or a guest
camera API. SCCB does not require XCLK in this approximation; frame delivery does. The
clock check establishes enable/routing, not an exact sensor clock tolerance. Board PWDN
uses the existing GPIO output callback; arbitrary alternative board wiring is outside
this fixed-board slice. Lifecycle and transport qualification belong to subsequent cards.

## CPU comparison

PENDING

No timing was requested or run.
