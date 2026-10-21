# Classic ESP32 camera: scheduled VSYNC and physical DVP routing

Local-only C6 feature slice. Base `de9f3978df0a25af2e4fbcafc337ca22c75146af`
is the clean merge of accepted board/source C3
`6d80410460fd81a46cdb3277717ef2e7385c056a` and register-adapter C4
`42d5c9dbe29827bfe2271777b74e7af914559c2f`. It does not include the separate
C5 FIFO packer. This is not end-to-end classic camera support.

## Behavior and integration contract

The AI-Thinker sensor now drives physical GPIO25 through the existing board
input-edge, deadline, GPIO-level and GPIO-interrupt machinery. It does not use
LCD_CAM registers or I2S interrupts for VSYNC. Capture can be unarmed, explicitly
stopped or in reset while the independently powered/clocked sensor runs.

Timing is deliberately approximate: 10 frames/second, with a 1ms active-high
VSYNC blanking pulse. The 240MHz stable bus-time domain is independent of the
selected guest CPU clock. GPIO level remains high at the public driver's delayed
1us ISR check. No per-PCLK stepping, exposure timing or sensor timing fidelity
is claimed. Large advances retain every due edge's original timestamp.

PWDN must be explicitly driven low, the OV2640 must be streaming, and GPIO0
must have a live routed PWM clock (finite positive frequency and nonconstant
duty). Stopping any of these cancels the deadline and immediately lowers an
asserted VSYNC at the same bus horizon. Restoring them schedules a new frame,
not a backlog of retroactive frames. Inverted XCLK remains a valid clock.
Clock/matrix/SCCB writes reconcile inputs even if no GPIO output edge occurs.
A latched PWM update is observed on the following existing board-input advance;
this is not a bit-level PWM-to-sensor clock model.

Future classic transport must call `SocBus::camera_routed_frame()`, not the
C3 raw host-source `camera_frame()` hook. The routed hook requires I2S0 clock
and reset gates, camera mode, slave RX start, descriptor enable, forced FIFO
mode 1 or 3, and the fixed board's actual D0..D7/PCLK/HREF/VSYNC matrix inputs.
The driver routes H_ENABLE to constant high. Disabled pad inputs, wrong pad
functions, wrong physical sources and disconnected routes cannot supply valid
capture bytes. Every request requalifies the current routing.

Data inversion applies an XOR mask to wire bytes without modifying the cached
host frame. Inverted PCLK/HREF/VSYNC polarities are explicitly unsupported and
return no frame rather than silently producing a normal-polarity capture.
This is conservative fixed-board support, not sampling arbitrary wiring.
The helper returns source bytes only: it performs no FIFO packing, DMA writes,
descriptor completion, EOF interrupt or guest framebuffer delivery.

The board's clock-aware input-advance method is appended at the trait tail.
Other boards retain their existing `advance_to` behavior and do not query a
camera clock. The existing `pins_active` guard still excludes a camera-free
bare board from both deadline and producer calls. Tests use panic sentinels
for those forbidden calls, plus an active non-camera board whose clock callback
must never run. No camera field is added to the bus or peripheral hot layout.

## Public source contract

`sources.py OUTPUT` fetches five immutable public sources, hashes their actual
bytes and checks matrix IDs and driver routing/ISR behavior. `sources.json`
retains URLs, lengths, SHA-256 and exact selected source lines.

- ESP-IDF v4.4.7, `38eeba213aa695aabfd6d89aa9f5078dbe5a94c3`:
  `components/soc/esp32/include/soc/{gpio_sig_map,gpio_reg,io_mux_reg,dport_reg}.h`.
  Matrix source bits 0..5, inversion bit 6 and selection bit 7;
  WS/PCLK input 28, data inputs 140..147, H/V_SYNC 190/191, H_ENABLE 192.
- esp32-camera v2.1.4, `b817e9b81041980608d1a3d3e0b08d86e3666382`,
  `target/esp32/ll_cam.c`: physical routing, constant-high enable, GPIO ISR and
  its 1us delayed VSYNC check. C4's register/clock source receipt is inherited.
- Fixed pinout inherits C3's public Arduino-ESP32 2.0.17 AI-Thinker provenance.

These contracts are source-derived, not checked on physical hardware.

## Qualification

`checks.json`, `test-totals.json`, `goldens.json` and the mutation receipts retain
actual command outcomes. Final results:

- Native and WASM Clippy with warnings denied: PASS.
- Empty-HOME ROM-enabled workspace: 1011 passed, 0 failed, 22 filtered.
- Empty-HOME plain workspace: 983 passed, 0 failed, 50 ignored.
- WASM build and all eight named demos, including the harness JIT handoff: PASS.
- Assertion mutation kills: C6 35/35, inherited C3 21/21 and C4 23/23;
  no compile-error kills. The integration test has seven new C6 tests.
- Non-camera goldens: 55 byte-identical files. Public S3 fixture: seven unchanged
  frames and matching serial/normalized IRQ receipt. Classic hot bodies: 21/21
  identical against the C3+C4 merge base.
- Evidence privacy and `git diff --check`: PASS.

Run with Rust 1.99.0, `nice -n 10`, an isolated target
and empty HOME (explicit Cargo/rustup homes retained only to find the toolchain):

```
tools/fetch-demo-assets.sh --no-linux
python3 docs/evidence/classic-camera-dma/routing/qualify.py TREE OUTPUT
CARGO_TARGET_DIR=OUTPUT/target python3 docs/evidence/classic-camera-dma/routing/mutate.py TREE OUTPUT/mutations
CARGO_TARGET_DIR=OUTPUT/target python3 docs/evidence/classic-camera-dma/board/mutations.py TREE OUTPUT/c3-mutations
python3 docs/evidence/classic-camera-dma/mode/mutate.py TREE OUTPUT/c4-mutations
python3 docs/evidence/classic-camera-dma/routing/summarize.py TREE OUTPUT
```

Do not run mutations concurrently with builds/checks in the same source tree.
The WASM build script needs a temporary `target` symlink to the isolated target
because it copies from a conventional relative path. No JIT implementation is
changed, so the separate JIT differential suite is not applicable.

The seven new C6 tests cover unarmed/stopped GPIO interrupts and the delayed
level check, no DMA writes, power/clock/sensor cancellation and restart, every
physical wire with another enabled pad as the negative control, byte inversion
and restored routing, receiver/gate controls, timestamped multi-edge advances,
and camera-free idle deadlines/steps. `mutations.md` is the mutation-to-test table;
`mutations.json` records each executable replacement and exact command that
kills it. Compilation errors are
not accepted as kills. Inherited C3 and C4 mutation tables are rerun separately.

## Unchanged S3 fixture and non-camera controls

The unchanged public `camera-start-2026-10-02` fixture is run on both merge base
and candidate using the inherited `camera-shared-state/fixture.py` harness.
Its pinned ROM and three flash input hashes are checked before execution.
`fixture-comparison.json` records seven identical guest frames: four RGB565
`7bf49dc5`, three YUYV `8c2afdc5`. Serial SHA-256 is
`b0011b04b138834fd6d6697ed6a9cfd328065f5af87b1a68d7eb146f6ba01bd2` on both.
Instruction, exception, interrupt and per-source interrupt receipts also match;
only image/ROM paths and host wall time/throughput are normalized from stderr.
No existing golden or firmware input is changed.

## Hot-path code generation

`codegen.py TREE BASE_BINARY CANDIDATE_BINARY OUTPUT` uses the inherited EX230
address/page-relocation and equivalent-call-alias normalizer. All 21 selected
classic hot functions match the C3+C4 merge base, including bus tick, read32,
fetch, next_deadline and machine run/step/IRQ functions. `codegen.json` retains
the selector, instruction counts and binary/disassembly hashes.

This is not whole-binary equality or CPU timing: the out-of-line input delivery
helper and peripheral-write reconciliation intentionally change to deliver the
new physical inputs. The ordinary bare-board guard is unchanged; active GPIO
boards retain default advance semantics without camera-clock polling.

## CPU comparison

PENDING. No timing run was performed.

## Limits

No compiled guest `esp_camera_init`/framebuffer capture, FIFO/DMA transport,
PSRAM, JPEG, product integration, arbitrary control polarity or hardware
qualification. Lifecycle across a guest reboot is deferred to the separate
camera lifecycle qualification slice; C6 schedules within a monotonic bus
horizon. No push, PR or publication is part of this local receipt.
