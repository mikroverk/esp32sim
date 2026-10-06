# EX212: Live camera input and idle cost

Base `cbb9edf607a09be78cb199c49c2e4cc0bd27d0dc`. The review retains the public
OV5640 SCCB state and the still-image path. RGB24 streaming uses one pending
picture, without a web server. FIFO opening happens on the reader thread.
This is a live source: regular files are consumed immediately, not paced or
deterministic. Browser RGBA uses the existing type-3 binary input. Both live
interfaces enforce the same 8 MiB message-equivalent limit.

Sensor output supports RGB565 MSB first, YUYV and grayscale. The shared picture
code accepts a format enum; only the board maps OV5640 register values.
Nearest-neighbour crop/scale omits ISP offsets, mirror/flip and binning. Typical
driver settings can differ by about 2.5% horizontally and 1.6% vertically.
JPEG, RAW and odd-width YUYV log an unsupported-output diagnostic and emit no
frame. Sensor reset and board reattachment reset shared geometry.

## Validation

The [driver receipt](../camera-start-2026-10-02/README.md) covers the independent
VSYNC fix. The final live-source driver run checks four RGB565 and three YUYV
frames, including deinit/init and reboot. Unit tests additionally cover a
mid-frame VS_EOF start, multiple byte-count EOFs on a ring, owner checks, EOF
counter resets, blanking, unsupported capture modes, clock gating and idle return.

`tools/camera-cli-test.mjs` boots and exits with a FIFO that has no writer.
`tools/camera-ui-test.mjs` exercises the page's actual webcam handlers, including
stop, failed boot, disconnect, delayed permission and insecure-origin errors.
The WASM ABI test sends type-3 input to S3, C3 and C6 and checks board rejection.
Parser tests include malformed RGBA dimensions and crop windows.
No physical webcam or silicon timing comparison is claimed.

```sh
cargo +1.99.0 clippy --workspace --all-targets -- -D warnings
cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
cargo +1.99.0 test --release --workspace
RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh
node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel
node tools/camera-cli-test.mjs
node tools/camera-ui-test.mjs
node tools/check-evidence-privacy.mjs
```

## CPU comparison

Build each source with `cargo +1.99.0 build --release --bins`. No builds or test
jobs run during measurement. S3 hello uses hello_world, `--board none` and 3000
modeled seconds. Pocket Tank, C3 hello and C6 hello use 30 seconds. One warmup
pair precedes seven alternating pairs per workload. Child user CPU is measured
with getrusage. Both core instruction counts and console hashes must match.

```sh
python3 docs/evidence/camera-live-2026-10-05/speed.py \
  "$MAIN_BIN" "$CANDIDATE_BIN" --base-revision "$MAIN_SHA" \
  --candidate-revision "$CANDIDATE_SHA" --output "$OUTPUT"
```

Results include every round, executable/input hashes and exact CLI arguments.
Interpret median differences together with their ranges; a small median delta
is not a sub-percent precision claim. The review's M5 Max results motivated
moving camera state to the end of LCD_CAM, placing EOF progress on the receive
channel, and checking capture state before pixel work. The comparison here uses
an Apple M5 Pro, macOS arm64, Rust 1.99.0. The default toolchain is unchanged.
