# Classic camera evidence

Current integrated-head checks and review fixes: [PR243 review qualification](review.md).

[Curated receipt index and archive provenance](CURATION.md) covers the board,
shared sensor state, SCCB, receiver mode, FIFO, routing, DMA and lifecycle receipts
now consolidated here. Historical results below were not rerun by this curation.

Supported guest-capture scope is raw RGB565/YUV422 in internal DRAM. JPEG and
adjustable FPS are parked; FIFO mode 3 is unit-tested, not guest-driver-qualified.
CPU comparison remains pending for classic ESP32; no CPU timing was run here.
Route-loss `IN_DSCR_ERR` is an intentional partial-frame cancellation policy,
not a claim about what the hardware reports for a disconnected route.

## Feed classic camera frames through native I2S RX DMA

RMW INLINK STOP qualification is extended by [the remediation receipt](rmw-stop.md).
The original qualification below predates that additional regression; use
`rmw-stop-checks.json` for the remediation's executed checks.

The scheduled falling-edge start window and retained native-driver qualification
are documented in [arming-window.md](arming-window.md).

The reproducible public-driver fixture and independent frame hash are recorded
in [fixture.md](fixture.md); this adds CI inputs, not a new transport path.

# Scope and base

Local-only C7, a new feature, not a reproduced production bug. The merge base is
8f3a9eaa4ab2e4ba0d58721c0abe9fcf7428ecd6, merging accepted C6
bfb0c4a28d291d1d35e7e286a43f1a7e4b1e25b4 with accepted C5
f0027bfbbe6a0552e23b193b27f94bedfe32c920. Pinned main for codegen is
142805d5399e0c3cf25995fe21ba524e75a2b3cb. Implementation head:
8ac344b77038631660f87025f76b272ec3a87b4f.

# Contract

Physical VSYNC runs independently of capture, as in C6. Once I2S0 camera RX and
its native inlink are armed, capture waits for a physical VSYNC pulse and latches
a route-qualified source at its falling edge. C5 packs each row into complete
mode-1 or mode-3 FIFO words. Delivery is approximated uniformly across the existing
10 Hz active interval; the next rising VSYNC flushes the last whole words. It is
not a per-PCLK or silicon timing model. Finite guest service intervals observe
native chunk EOF interrupts, not a fabricated camera-frame-complete flag.

The only transport is the existing bounded GdmaInCh receive/scatter path through
DramDma. There are no new DMA registers, descriptor readers, memory aliases or
PSRAM permissions. The one dma_range rule still excludes the CPU-only ROM
integrity prefix, external RAM, MMIO, IRAM, RTC memory and crossing DRAM ends.
Native CHECK_OWNER semantics are unchanged: bad-owner rejection is qualified with
that bit enabled; disabling it intentionally retains the hardware's native ring
semantics, as for PCM. EOF count remains RXEOF_NUM times four DMA bytes.

Stop, inlink restart and RX reset discard capture state; RX stop/reset requires
an explicit new inlink before transport resumes. The existing DPORT write loop
also discards camera state on peripheral reset. Route/power loss during capture
stops the native channel with descriptor error, so re-enabling pins cannot splice
an old partial frame into a new one. Earlier successful chunk EOF bits can remain
latched until guest INT_CLR, as on the existing channel; they are not whole-frame
success and are not retroactively erased when a later chunk faults.

Register provenance is unchanged from C4/C6 and the native I2S adapter: ESP-IDF
v4.4.7 components/soc/esp32/include/soc/i2s_reg.h (CONF, FIFO_CONF, CONF2 and
LC_CONF), gpio_sig_map.h (parallel routes), and ESP-IDF v5.5.4
hal/esp32/include/hal/i2s_ll.h:639-643 (RXEOF_NUM word unit),
register/soc/i2s_reg.h (classic IRQ mapping) and register/soc/dport_reg.h:947-1000
(clock/reset bits). The pinned public esp32-camera v2.1.4 ll_cam.c source and
actual extraction filters are reproduced by the inherited C5 verifier. These are
header/driver-derived facts, not silicon validation.

# Measurable acceptance

The QQVGA test sends a deterministic four-color 160x120 RGB565 image: 38,400
sensor bytes become 76,800 mode-1 DMA bytes across 20 descriptors and ten EOF
chunks. Guest service is once per tenth active frame interval. Every descriptor's
length, returned owner, EOF bit, EOF descriptor register and classic INT_RAW/INT_ST
are checked, as are every packed byte and the untouched next interval's bytes.
A smaller exact-vector test covers both modes, three descriptors per EOF and two
EOF chunks, including EOF-shortened descriptors and guest interrupt clearing.

Ten C7 integration tests additionally cover bad owner, forbidden descriptor/buffer
memory and crossing DRAM ends, ring starvation and finite service/refill, inlink
stop, RX/FIFO/AHBM resets, DPORT reset, fresh restart, partial tick splits, late
arming, route/power loss, two different images on a continuously reused ring, and
camera-free active PCM without board-camera polling. Existing PCM tests are not
changed. C6's camera-free scheduler/producer assertions remain in force.

C7's 25 semantic mutants are assertion-killed, not compile-error kills; see
[the table](mutations.md) and [summary](c7-mutations.json). The baseline passes. Reproduction:

    python3 docs/evidence/classic-camera-dma/mutate.py TREE OUTPUT/c7
    python3 docs/evidence/classic-camera-dma/fifo/mutate.py TREE OUTPUT/c5
    python3 docs/evidence/classic-camera-dma/fifo/verify-driver.py TREE OUTPUT/driver
    python3 docs/evidence/classic-camera-dma/routing/mutate.py TREE OUTPUT/c6

All cargo invocations use cargo +1.99.0, nice -n 10 and isolated targets.

# Historical C7 qualification

Final qualification passes: 1,027 ROM-enabled tests and 999 plain tests (50
ignored); native/WASM Clippy, WASM build and all eight demos pass. All 95 semantic
mutants are killed: C7 25, C6 35, C5 12 and C4 23, with no compilation-error kills.
The actual public C filters recover 13 vectors exactly, retaining three known
partial-group negatives. Seven S3 fixture frames, serial hash and normalized
instruction/IRQ receipt match the merge base. C5/C6 rules were rerun on the first
implementation commit; their mutated source files are unchanged by the final
cached-dispatch commit. C4/C7 were rerun on the final implementation head.

Final commands and numeric outcomes are in checks.json. The required commands
(all Cargo steps under nice -n 10 and cargo +1.99.0) are:

    cargo +1.99.0 clippy --workspace --all-targets -- -D warnings
    cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings
    ESP32SIM_ROM_DIR=<fresh-assets> cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
    cargo +1.99.0 test --release --workspace
    RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh
    node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel
    node tools/check-evidence-privacy.mjs
    python3 docs/evidence/classic-camera-dma/shared-state/fixture.py TREE OUTPUT PINNED_PUBLIC_FIRMWARE
    python3 docs/evidence/classic-camera-dma/codegen.py TREE BASE_BINARY CANDIDATE_BINARY OUTPUT/codegen-base 8f3a9eaa4ab2e4ba0d58721c0abe9fcf7428ecd6
    python3 docs/evidence/classic-camera-dma/codegen.py TREE MAIN_BINARY CANDIDATE_BINARY OUTPUT/codegen-main 142805d5399e0c3cf25995fe21ba524e75a2b3cb
    python3 docs/evidence/classic-camera-dma/explain-codegen.py TREE OUTPUT EXPLANATION_JSON

Native and WASM checks
use an empty HOME, with only explicit Cargo/Rustup tool locations preserved;
ROM-enabled tests additionally set ESP32SIM_ROM_DIR to freshly fetched assets.
No external-person data or user projects are used. The 55 golden payloads plus
the two golden harness files are byte-identical to the merge base and pinned
main (goldens.json). No JIT source is changed.

# Hot-path code generation

The inherited address/call-alias normalizer compares selected classic machine,
bus and core functions against both the merge base and pinned main. The compact
comparisons retain binary/disassembly hashes, function instruction counts and
all mismatches; they do not claim whole-binary equivalence. See codegen results
and the explanation recorded alongside them.

Against each control, 10/21 selected bodies are identical under the inherited
normalizer. Ten further bodies have identical operation/register/order sequences:
every changed immediate is a +16-byte field displacement from the two appended
optional capture pointers (verified by explain-codegen.py). The remaining tick
body changes from 2,207 to 2,237 static instructions. Its new camera dispatch is
behind the existing clock/running-DMA checks; the rest of its differences include
field displacements and register/stack allocation. codegen-explanation.json keeps
the call-site guard window and opcode deltas. These layout differences are not
silently normalized into the 10/21 equality score. They are explained, not claimed
as byte-identical or zero measured CPU impact.

Camera transport is entered only behind the existing clock-enabled/running-DMA
gate. Dispatch is cached on CONF2 writes, so an active PCM-only channel does not
call the camera helper or poll camera registers. Camera-off boards allocate no
Capture. Board hooks are not polled by an active PCM-only channel; the atomic-count
regression test proves that condition. The active camera branch and the optional
boxed capture state change native code/layout as documented above. This is not
represented as a CPU speed measurement.

## CPU comparison

PENDING. No CPU timing or benchmark was run.

# Limits

This receipt proves native register/descriptor integration, not an actual compiled
classic guest esp_camera_init or esp_camera_fb_get. Those belong to C8. No claim
of full classic camera/product support, JPEG, PSRAM, sensor ISP fidelity,
bit-accurate PCLK, arbitrary control polarity, machine reboot lifecycle or browser
product integration is made. No push, PR, issue, deployment or publication.
