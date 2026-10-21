C7.1 configuration-horizon capture cancellation

Base: 213b1adda7910923ba6ac65ad6e2446c36d47d6a, preserving the accepted
C7 ancestor a3315ab85be28cd5318f2ab381a59f9b4b9c6b7a. Local only.

Contract and mechanism

A qualified receiver must discard its capture when its source becomes invalid,
even if software restores it before the next DMA tick. Checking only the route
at that later tick misses a real loss. Reconciliation now runs after a peripheral
write has applied its GPIO, clock and board effects. Only an existing qualified
capture requests the routed source. If unavailable, the existing native abort
operation drops the packed frame, stops DMA, resets receive progress and raises
the existing descriptor error. `IN_DSCR_ERR` on route loss is the emulator's
intentional partial-frame cancellation policy, not a hardware behavior claim.
It does not write guest memory or manufacture EOF.

Restoring the source, clearing the error, and benign receiver writes cannot
restart DMA. A fresh explicit INLINK and a full physical pulse recover. A start
on an initially unavailable route remains unqualified and waits, as before.
Valid XCLK inversion, unrelated pads and non-routing bits do not abort. Matrix
and IO_MUX/RTC opportunity invalidation, consumed-edge no-replay, STOP/reset/start,
the scheduled waveform, public driver, FIFO packing and DMA walker are unchanged.
The late-tick source check remains a defensive fallback. No tick poll or fields
in a hot structure were added: the only new query is on the configuration path,
behind the boxed capture's existing qualification bit.

Regression evidence

The supplied partial-frame repro was applied unchanged to the base and run with
REVIEW_CAUSE=0/1/2. Each compiled and assertion-failed: EOF was 512, expected 0.
They pass on the changed code. The environment-selected review module is not a
permanent test; the new cancellation module replaces it with explicit cases.

Ten permanent tests exercise both FIFO modes for matrix, IO_MUX, RTC ownership,
power, XCLK, SCCB streaming, I2S and LEDC gates, plus qualified-but-not-yet-packed
capture. They assert synchronous error, unchanged bytes/descriptor/EOF address
and no EOF over two later full pulses, then exact bytes/descriptor/EOF after
explicit fresh INLINK. A positive control preserves active capture across
unrelated pad/RTC bits, pull-up and valid XCLK inversion. All inherited 26 camera
bodies and the eight arming-window bodies are byte-identical.

    cargo +1.99.0 test --release -p esp32 --test aithinker
    python3 docs/evidence/classic-camera-dma/mutate-cancellation.py TREE OUTPUT/new
    python3 docs/evidence/classic-camera-dma/mutate-arming.py TREE OUTPUT/arming
    python3 docs/evidence/classic-camera-dma/mutate.py TREE OUTPUT/c7
    python3 docs/evidence/classic-camera-dma/routing/mutate.py TREE OUTPUT/c6
    python3 docs/evidence/classic-camera-dma/fifo/mutate.py TREE OUTPUT/c5
    python3 docs/evidence/classic-camera-dma/mode/mutate.py TREE OUTPUT/c4

Mutation results are retained in cancellation-mutations-*.json. Compile errors
or timeouts do not count as kills. The old tick-only terminal-loss mutant
survives because configuration cancellation already stops that capture before
its next tick; the C7 harness therefore mutates the shared terminal abort
operation instead, retaining the semantic rule and testing both callers.

Native retained driver

All four fixture manifest sizes and SHA-256 hashes were verified before running
the unchanged retained public esp32-camera driver. The runner is an untracked
example, not a new driver or host framebuffer injection. See arming-window.md
for the retained fixture's public versions, format derivation and source paths.
The current execution prints C8 PASS with two 160x120 RGB565 frames of 38400
bytes. Independent RGB888-to-RGB565 construction, without importing emulator
code or reading a captured framebuffer, matches both frame SHA-256 values:

c67913e0afba1703e8c9c47e02b1ae2ea62e47596a8440d83f1ebba14aed6cf1

Host-only tracing at effective capture start measures 1632 cycles for each of
three starts. ARM_WINDOW remains 240000 cycles; its quarter-window is 60000.
No guest instructions/cycles were added. The trace source was restored before
clean verification. The exact edge/start pairs and input hashes are retained
in cancellation-checks.json. This is emulator execution, not hardware timing.

    cargo +1.99.0 build --release -p esp32 --example camera_capture
    "$CARGO_TARGET_DIR/release/examples/camera_capture" "$FIXTURE" "$ROM"
    python3 docs/evidence/classic-camera-dma/expected-frame.py

Checks and code generation

The required gates pass: 44 camera tests, 1048 ROM-enabled workspace tests,
1020 plain workspace tests (50 ignored), native/WASM Clippy, WASM build and all
eight demos. All 60 tracked golden paths are byte-identical. There are 112
runtime-killed semantic mutations, of which 100 are explicit assertion failures;
all seven new cancellation mutations are assertion-killed. No compiler error or
timeout is counted. The selected native bodies are 21/21 strictly normalized
equal to the parent, including classic tick at 2236 instructions on each side.

Commands, outcomes, source hashes, mutation totals and selected-body comparison
are in cancellation-checks.json and cancellation-codegen.json. Native/WASM
Clippy deny warnings; both workspace test modes use empty HOME, explicit Cargo
and Rustup locations, and only the ROM-enabled mode sets ESP32SIM_ROM_DIR.
One card-owned CARGO_TARGET_DIR is shared by all builds and removed afterward.
All Cargo commands use cargo +1.99.0 and nice -n 10. There are no JIT changes.

    cargo +1.99.0 clippy --workspace --all-targets -- -D warnings
    cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings
    ESP32SIM_ROM_DIR="$TREE/web/wasm/fw" cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
    cargo +1.99.0 test --release --workspace
    RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh
    node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel
    node tools/check-evidence-privacy.mjs
    cargo +1.99.0 build --release --bin esp32sim
    python3 docs/evidence/classic-camera-dma/codegen.py TREE BASE_BINARY CANDIDATE_BINARY OUTPUT/codegen 213b1adda7910923ba6ac65ad6e2446c36d47d6a

The comparison isolates this cancellation change against its exact parent.
It does not replace inherited comparisons against pinned main or claim current
main equivalence. No CPU performance measurement is inferred from assembly.

## CPU comparison

PENDING. No timing benchmark was run.

Limits

No hardware validation, per-PCLK accuracy, arbitrary control polarity, JPEG,
PSRAM, browser camera integration or production-user claim. No public driver
changes, new DMA path, push, PR, issue, Docker operation or deployment.
