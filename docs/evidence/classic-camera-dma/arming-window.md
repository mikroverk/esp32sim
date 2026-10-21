C7.1 scheduled falling-edge arming window

Base: a3315ab85be28cd5318f2ab381a59f9b4b9c6b7a (accepted C7 RMW STOP fix).
This is local-only emulator qualification, not silicon validation.

Contract

The AI-Thinker waveform is unchanged: 1 ms high, 99 ms low, 10 Hz.
ARM_WINDOW = VSYNC_CYCLES is an approximate frame-start back porch, not an
OV2640/PCLK timing claim. A scheduled falling edge opens one opportunity; power,
XCLK and streaming forced-low edges never do. Physical DVP matrix, IO_MUX and
RTC ownership changes clear that opportunity; unrelated pins and non-routing
bits do not. The I2S adapter records the effective start on MMIO writes,
not on the first subsequent DMA tick. A start in the inclusive window can latch
the current frame; otherwise it waits for the next full pulse. Both paths use
absolute elapsed bus cycles from the scheduled edge, including catchup on the
first post-start tick. A claimed edge is consumed in the board, outside the
resettable I2S capture, so a second start cannot replay any part of that frame.
Recording the start alone does not make an initially unavailable route a
terminal error: the original C7 behavior waits until a valid source has been
observed, after which route loss is terminal until explicit rearm.

Pixels still pass through CameraFifo, native I2S INLINK descriptor DMA and the
existing bounded internal-DRAM adapter. There is no guest framebuffer injection,
public-driver modification, sensor waveform change, or PSRAM change. New state
is confined to the boxed capture and board. New callbacks are appended to the
board vtable. The existing clock/running-DMA and cached camera dispatch gate is
unchanged. Configuration bookkeeping is on MMIO writes; idle ticks and active
PCM do not poll the camera. The board updates its timestamp while processing an
already scheduled edge, not by adding a tick poll.

At a coarse first post-start tick exactly on the next rising edge, the previous
unclaimed edge is retained long enough to deliver its remaining whole words.
The producer then marks the new high pulse observed, preserving the existing
continuous-ring path and split invariance. The board's next deadline still
slices guest execution at the original waveform edges.

Native retained fixture

Public esp32-camera b817e9b8, IDF 5.4.3, unchanged retained binary/ELF and runner.
No IDF rebuild. The C8 owner retains the full fixture manifest and recipe; its
runner is copied only as an untracked example, not committed by this change.
All four manifest sizes and SHA-256 hashes were verified before execution:

classic_camera.bin 247456 685ce513255eff2247eb9ff12d60ff5773529522703d26db5866898a1d3483db
classic_camera.elf 4346440 80ac10d1744ee029439782abc8b5d362e7190026d87d29843f343a04963f2204
bootloader/bootloader.bin 25584 9fe22a0bd805d8830602afa2d92f99312b7f567503db22808e3f7cc903f68850
partition_table/partition-table.bin 3072 7f00b6c042a89b15b0cac534f82ed988caf29278ff5700b0c511eb1b5bb7c820
esp32_rev300_rom.elf SHA-256 920b70635440517866aab2230964a570d2cf2b676658d93c52fbac108c1cca31

Observed: esp_camera_init=ESP_OK, two frames, each width=160 height=120
len=38400 RGB565, then C8 PASS. Each observed framebuffer SHA-256:
c67913e0afba1703e8c9c47e02b1ae2ea62e47596a8440d83f1ebba14aed6cf1
Independent expected SHA-256, from expected-frame.py:
c67913e0afba1703e8c9c47e02b1ae2ea62e47596a8440d83f1ebba14aed6cf1

For pixel index p, host RGB888 is (p % 256, (p / 160) % 256, (p * 7) % 256),
with integer division. Quantize to ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3).
Sensor bytes are MSB-first RGB565; mode-1 FIFO [LSB,0,MSB,0] and the public
ll_cam_dma_filter_yuyv sample1/sample2 extraction restore MSB,LSB (public
target/esp32/ll_cam.c:62-70,163-182,524-537 selects this filter for RGB565 at
10 MHz XCLK). The expected
script independently constructs these 38400 bytes without importing emulator
code or reading the captured framebuffer.

Latency measurement: temporary host-only tracing immediately after effective
capture start in camera_dma_written queries the board's unclaimed edge and
prints (edge, bus cycles, bus cycles minus edge). Public ll_cam.c:269-291
resets RX and the native channel, writes INLINK START at 289, then RX_START at
290; this final write is the measured effective start. No guest changes or added
emulated cycles. Source tracing was removed before the clean verification.
Three starts in the retained 300000000-instruction run:

edge       start      latency cycles
52770195   52771827   1632
100770195  100771827  1632
148770195  148771827  1632

At the 240 MHz bus domain this is 6.8 microseconds. ARM_WINDOW is 240000 cycles
(1000 microseconds); the required quarter-window is 60000 cycles (250
microseconds). Every measured latency is within it; no window enlargement.

Reproduction (caller supplies retained fixture and ROM paths):

    cargo +1.99.0 build --release -p esp32 --example camera_capture
    "$CARGO_TARGET_DIR/release/examples/camera_capture" "$FIXTURE" "$ROM"
    python3 docs/evidence/classic-camera-dma/expected-frame.py

Tests and mutations

The inherited 26 aithinker/C6/C7 test bodies are unchanged. New tests cover
falling-edge-only starts in both FIFO modes with full frame bytes/descriptors/
EOF, 1/7/257 tick splits, inclusive window boundary and boundary+1, exact first
catchup word count, driver STOP/reset/START, duplicate start after partial
word delivery, forced-low power/clock/stream stops, and same-cycle loss/restoration
of matrix, IO_MUX, RTC ownership, power and XCLK. Separate controls pin unrelated
pad/RTC bit writes and initially unavailable routes that must wait, not abort.
The boundary+1 test verifies recovery at the next full pulse.

    cargo +1.99.0 test --release -p esp32 --test aithinker
    python3 docs/evidence/classic-camera-dma/mutate-arming.py TREE OUTPUT/arming
    python3 docs/evidence/classic-camera-dma/mutate.py TREE OUTPUT/c7
    python3 docs/evidence/classic-camera-dma/routing/mutate.py TREE OUTPUT/c6
    python3 docs/evidence/classic-camera-dma/fifo/mutate.py TREE OUTPUT/c5
    python3 docs/evidence/classic-camera-dma/mode/mutate.py TREE OUTPUT/c4

The C7 mutation script's three elapsed/arming anchors are updated for the new
absolute-edge implementation, retaining those semantic rules. Every mutation
must be killed by a named assertion failure, not compilation failure or timeout.
See arming-mutations.json and arming-checks.json for executed results.

Hot-path disassembly

Compared native release CLI binaries against the exact accepted C7 base with
the inherited EX230 address/page/call-alias normalizer: 20/21 selected bodies
are identical. Tick is 2237 versus 2236 static instructions, not byte-identical.
The camera helper now uses the absolute edge time rather than its cycles
argument, so the compiler removes one `ldr x2` argument load at that call.
That call remains behind the existing clock-enabled, running-DMA and cached
camera-mode branches. Every other tick difference is an SP-relative spill slot
assignment. explain-arming-codegen.py verifies the full normalized sequences
match after this one exact call-site removal and SP-slot relabeling; it does
not inflate the strict 20/21 equality score. Raw deltas, binary/disassembly
hashes and both guard windows are retained in arming-codegen.json.

This isolates C7.1 against its accepted base. The inherited C7 receipt's
comparison with pinned main and its original camera dispatch/layout delta are
not replaced by a claim of equivalence with current main. No new tick poll,
field before a hot struct, gate or idle-path operation is added here. MMIO
route/start configuration work and scheduled board-edge bookkeeping are
separate from the ordinary idle tick.

    cargo +1.99.0 build --release --bin esp32sim
    python3 docs/evidence/classic-camera-dma/codegen.py TREE BASE_BINARY CANDIDATE_BINARY OUTPUT/codegen a3315ab85be28cd5318f2ab381a59f9b4b9c6b7a
    python3 docs/evidence/classic-camera-dma/explain-arming-codegen.py TREE OUTPUT/codegen OUTPUT/arming-codegen.json

Qualification

All required checks pass: 1038 ROM-enabled tests, 1010 plain tests (50 ignored),
native/WASM Clippy with warnings denied, WASM build, all eight demos and evidence
privacy. There are 34 camera integration tests: all inherited 26 unchanged and
eight new tests. All 105 semantic mutants are runtime-killed: C4 23, C5 12,
C6 35, C7 26 and C7.1 nine. The nine new mutants and all 26 C7 mutants are
explicitly assertion-killed; inherited suites also contain panic/unwrap guards,
classified individually in arming-inherited-mutations.json rather than called
assertions indiscriminately. The final full check run follows mutation restoration.
The clean native driver rerun again prints C8 PASS and both hashes match.
All 60 tracked paths matching the golden glob are byte-identical to the base.
Exact commands, inputs, source hashes, frame hash pair and counts are retained
in arming-checks.json. Normalized command paths use caller-supplied TREE, OUTPUT
and FIXTURE directories. The standalone arming baseline and all mutation suites
are reproducible with the commands above.

    cargo +1.99.0 clippy --workspace --all-targets -- -D warnings
    cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings
    ESP32SIM_ROM_DIR="$TREE/web/wasm/fw" cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
    cargo +1.99.0 test --release --workspace
    RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh
    node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel
    node tools/check-evidence-privacy.mjs

All Cargo builds use cargo +1.99.0,
nice -n 10 and a single card-owned target. Native/WASM checks use empty HOME,
explicit Cargo/Rustup locations, and only the ROM-enabled run sets ROM input.
No JIT code changes; no goldens intentionally changed. After verification, the
single target was deleted: 1836841496 unique logical file bytes and 1945608192
allocated bytes reported by file metadata. The detached base build worktree
was removed too. Cleanup is recorded in arming-checks.json.

## CPU comparison

PENDING. No timing benchmark was run.

Limits

This verifies native classic camera transport with the retained public driver,
not silicon, JPEG, arbitrary sensor polarity, PSRAM capture, sensor ISP fidelity,
per-PCLK accuracy, reboot lifecycle, browser camera integration or production
user behavior. No push, PR, issue, Docker operation or deployment.
