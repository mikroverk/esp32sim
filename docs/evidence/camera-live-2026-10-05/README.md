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
The [mutation results](mutations.json) include the exact isolated substitutions
and command; all ten cause test failures. [Check results](checks.json) include
the synthetic Chrome webcam run. No physical webcam or silicon timing comparison
is claimed.

Rebuild the firmware with the driver receipt’s helper, then run the live commit:

```sh
python3 docs/evidence/camera-start-2026-10-02/external_camera.py \
  --emulator target/release/esp32sim --firmware "$FIRMWARE" \
  --rom web/wasm/fw/esp32s3_rev0_rom.elf --output "$OUTPUT" --expect frames
```

[Driver results](driver.json) record the seven captures and executable hash.

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

User CPU seconds; seven measured pairs after one warmup. Main `cbb9edf607a09be78cb199c49c2e4cc0bd27d0dc`; measured code `58ab02b85eed9267c99bd26da2977b67bda55e97`.

| Workload | Main median (range) | Branch median (range) | Median change | Instructions |
| --- | ---: | ---: | ---: | ---: |
| S3 hello | 26.147 (25.328–31.347) | 25.844 (25.215–27.421) | -1.2% | 1,789,819,657 |
| Pocket Tank | 36.562 (36.176–37.286) | 36.576 (35.938–37.069) | +0.0% | 10,073,833,775 |
| C3 hello | 2.509 (2.479–2.532) | 2.514 (2.502–2.541) | +0.2% | 4,800,000,000 |
| C6 hello | 3.338 (3.290–3.467) | 3.357 (3.291–3.465) | +0.6% | 4,800,000,000 |

Every round below is main / branch, in user CPU seconds. Warmup is excluded from medians. Order alternates B→M for warmup, then M→B, B→M; M = main, B = branch.

| Pair | Order | S3 hello | Pocket Tank | C3 hello | C6 hello |
| --- | --- | ---: | ---: | ---: | ---: |
| Warmup | B→M | 25.330 / 25.196 | 37.683 / 37.586 | 2.566 / 2.587 | 3.324 / 3.330 |
| 1 | M→B | 25.328 / 25.215 | 36.176 / 37.069 | 2.508 / 2.541 | 3.290 / 3.324 |
| 2 | B→M | 25.558 / 25.782 | 36.562 / 36.492 | 2.509 / 2.502 | 3.329 / 3.291 |
| 3 | M→B | 31.347 / 27.421 | 37.286 / 36.720 | 2.489 / 2.534 | 3.352 / 3.369 |
| 4 | B→M | 27.529 / 26.599 | 36.421 / 36.563 | 2.522 / 2.513 | 3.338 / 3.357 |
| 5 | M→B | 27.078 / 25.844 | 37.107 / 36.746 | 2.479 / 2.503 | 3.364 / 3.438 |
| 6 | B→M | 26.147 / 25.974 | 37.214 / 36.576 | 2.521 / 2.517 | 3.337 / 3.332 |
| 7 | M→B | 25.919 / 25.650 | 36.465 / 35.938 | 2.532 / 2.514 | 3.467 / 3.465 |

Instruction counts (including each S3 core) and console hashes are identical in every run. The ranges overlap; these samples do not resolve sub-percent costs or establish a precise speedup. They do not reproduce the reviewer’s separated +3–4% S3 hello ranges on an M5 Max. Raw samples and input/executable hashes are in [speed.json](speed.json).

[Receive completion and overflow follow-up](review-2.md) records exact-fit
interrupt checks, the slow-clock reproduction, still-image limits and fresh
S3 hello/Pocket Tank measurements.
