# OV2640 SCCB model qualification

Base: `eae1ba46bb7c19fcb457120fc66766acfcd52bdd` (shared OV5640 extraction),
on upstream main `142805d5399e0c3cf25995fe21ba524e75a2b3cb`.
This adds a chip-independent OV2640 SCCB model, not classic camera capture.

## Contract and public source

`esp_soc::camera::{Ov2640, Ov2640State}` exposes a banked 8-bit sensor and shared
output dimensions, raw DSP IMAGE_MODE and configured-stream state. OV5640's
existing state, frame conversion and S3 behavior are untouched. No board attaches
this model yet; no classic-to-S3 dependency is introduced. Classic's workspace
dependency closure remains esp32 / esp-soc / esp-periph / emu-core / xtensa-lx7.
No bus tick, board layout, capture peripheral or DMA path changes.

Register facts come from public espressif/esp32-camera v2.1.4, commit
`b817e9b81041980608d1a3d3e0b08d86e3666382`:

- `sensors/private_include/ov2640_regs.h`: DSP/sensor bank selector 0xff,
  ZMOW/ZMOH/ZMHH, IMAGE_MODE, COM7_SRST, COM2_STDBY and RESET_DVP.
- `sensors/ov2640.c`: detect (548–563), reset (101–108), formats (110–135),
  output-size packing (137–208) and framesize/mode selection (211–246).
- `sensors/private_include/ov2640_settings.h`: complete CIF initialization,
  resolution-mode and RGB565/YUV422/JPEG tables; 4:3 ratio 1600x1200.
- `driver/sccb.c`: 8-bit writes and STOP-separated pointer/read transactions.
- `driver/sensor.c`: selected QQVGA/VGA/UXGA resolution entries.

Source hashes/URLs and generated numeric transaction fixtures are committed under
`esp-soc/tests/fixtures/ov2640/`. The fixture README explains host stubs, source
extraction and licensing. The generator compiles selected unchanged driver
routines; it does not execute ESP-IDF guest firmware. Six generated traces include
probe-ID reads, complete reset initialization, QQVGA and VGA RGB565, QQVGA YUV422,
VGA JPEG and UXGA RGB565. UXGA exercises both high geometry fields; QQVGA/VGA alone
cannot catch their removal. Format/size changes also run without software reset.

## Checks and commands

All Cargo commands use `+1.99.0`, `nice -n 10`, and an isolated `CARGO_TARGET_DIR`.
`target` is a task-worktree symlink to that directory because wasm-build.sh copies
from a literal target path. Rustup/cargo homes are supplied explicitly when HOME
is empty. No JIT code changed, so the separate wasm-jit-test script is inapplicable.

| Command | Result |
| --- | --- |
| `python3 tools/ov2640-driver-trace.py --scratch <owned-scratch> --check` | six traces and public source hashes byte-identical |
| `cargo +1.99.0 test --release -p esp-soc --test ov2640` | 7 passed, including classic-controller replay |
| `CARGO_TARGET_DIR=<owned-target> python3 tools/ov2640-mutations.py --out <owned-results>` | 17/17 semantic mutations killed by assertions, no compilation-error kills |
| `tools/fetch-demo-assets.sh --no-linux` | pass; own-worktree downloads, no copied ROM directory |
| `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings` | pass |
| `HOME=<empty> ESP32SIM_ROM_DIR=<own-worktree>/web/wasm/fw cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_` | 993 passed, 0 failed, 22 filtered |
| `HOME=<empty> cargo +1.99.0 test --release --workspace` (ROM variable unset) | 965 passed, 0 failed, 50 ignored |
| `RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh` | pass |
| `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel` | all 8 demos pass |
| `node tools/check-evidence-privacy.mjs` | pass; also run after staging new evidence |
| `git diff --check` | pass |
| `git diff --exit-code eae1ba46 -- tests/golden esp32/src esp32s3/src esp-periph/src Cargo.toml Cargo.lock` | no differences |

`mutations.json` records every actual mutation/test/result. The rules covered are
bank selection/storage, both geometry high fields, low geometry, format, reset
and stale-state clearing, ID preservation, pointer retention, per-write pointer
phase, standby, DVP reset, DSP bypass and bank-specific reset effects. Each mutant
returns a named test assertion failure, followed by automatic source restoration.

## Limits

- State is a bounded driver-facing approximation, inferred from public sources,
  not compared with silicon. Fresh/reset geometry is zero/unconfigured, not a
  claim about all physical sensor defaults. The selected sensor bank survives
  software reset so the driver's cached bank selection remains consistent.
- `streaming` means nonzero configured geometry, sensor out of standby, DVP out
  of reset and DSP not bypassed. Withholding converted DSP output during bypass
  is conservative model policy, not a measured statement that physical DVP stops.
  External power, XCLK, pin routing and frame cadence belong to later board work.
- Unknown ISP registers retain writes; indirect tables and full ISP controls are
  not modeled. Only single-register SCCB transactions are qualified; no burst
  auto-increment contract. The identification revision is modeled as 0x2642 with
  manufacturer 0x7fa2, not discovered by driver execution on hardware.
- Grayscale uses YUV422 on the sensor wire, as the public driver does. JPEG state
  is decoded, but no JPEG encoding/delivery is added. No image capture, board
  support, PSRAM, esp_camera_init success or production-error resolution claim.
- Local-only implementation. No push, PR, issue, deployment or timing run.

## CPU comparison

PENDING

The new unattached sensor has no work on camera-free tick paths. This is a
structural idle-cost statement, not a CPU measurement.
