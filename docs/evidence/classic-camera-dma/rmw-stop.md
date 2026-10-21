# RMW INLINK STOP remediation

The classic INLINK register retained START after consuming it. A later volatile
read-modify-write setting STOP therefore replayed START after stopping the channel.
The regression transports a quarter frame, performs that RMW, then waits through
the next physical frame without a new START. Before the fix it raises EOF (512);
a literal STOP already worked. This is an emulator-local reproduction, not a
production or hardware observation.

## Documented semantics

ESP32 Technical Reference Manual v5.8, section 2.6, printed page 60 states:

> I2S_OUTLINK_START bit in I2S_OUT_LINK_REG and I2S_INLINK_START bit in
> I2S_IN_LINK_REG are used for enabling the DMA Engine and are self-cleared by hardware.

Source: https://www.espressif.com/sites/default/files/documentation/esp32_technical_reference_manual_en.pdf
Downloaded PDF SHA-256: 4ba58e9fa0405ec2bf80b912a29b483f6edc8c4b2b1058201913a2fe37e582f0.
ESP-IDF v4.4.7 components/soc/esp32/include/soc/i2s_reg.h defines INLINK START
at bit 29, STOP at 28 and address at 19:0. Its `R/W` label alone does not explain
START self-clear; the TRM does. components/hal/esp32/include/hal/i2s_ll.h:494-498
starts via address plus START bitfield writes; :535-538 stops via a STOP bitfield
write. Public esp32-camera v2.1.4, commit
b817e9b81041980608d1a3d3e0b08d86e3666382, target/esp32/ll_cam.c:250-254 also
clears RX_START before writing STOP. The regression does not claim the complete
driver stop routine transfers while RX remains disabled.

START is consumed synchronously on INLINK writes, with its address preserved.
No per-tick work, field, DMA walker, memory permission or IRQ mapping is added.
STOP and RESTART readback and explicitly combined-command ordering are unchanged;
this fix does not infer their self-clearing behavior or new command priority.
Synchronous acknowledgement is the emulator approximation of the documented
hardware self-clear, not a claim about the number of peripheral clocks it takes.

## Coverage

Two C7 regressions exercise FIFO modes 1 and 3, respectively with RX left enabled
and with RX disabled before RMW STOP then re-enabled without new inlink. Both
require no further bytes, descriptor writeback, EOF descriptor change, INT_RAW
or INT_ST until a fresh START and physical VSYNC. Explicit fresh rearm (clearing
other commands first) recovers exact full-frame bytes and descriptor ownership.
The inherited literal STOP/reset, ring reuse, full QQVGA, owner, DRAM and IRQ
controls are retained. A two-port unit test, including PCM-only I2S1, verifies
START readback clears, an address-only RMW preserves active progress and later
fresh START reloads the new address and resets progress.

Reproduce the regression and semantic mutations with:

    cargo +1.99.0 test --release -p esp32 --test aithinker c7::
    cargo +1.99.0 test --release -p esp32 --lib inlink_start_self_clears
    python3 docs/evidence/classic-camera-dma/mutate.py TREE OUTPUT/c7

The additional C7 mutant restores retained START; both RMW-stop tests must kill
it with assertion failures. The original literal STOP control still independently
kills the ignored-STOP mutation.

## Codegen explanation helper

The inherited normalizer can print a page-aligned constant load as `[x8]` in one
binary but `[x8, #PAGEOFF]` in another. The explanation helper now accepts this
narrow spelling pair only after reading the actual 16-byte constants from both
supplied, hash-verified thin Mach-O binaries. It decodes the adjacent AArch64
ADRP/LDR Q instructions to locate file-backed bytes; unsupported shapes fail
closed. This does not change the original comparison or inflate its equality score.

    python3 docs/evidence/classic-camera-dma/explain-codegen.py TREE OUTPUT EXPLANATION_JSON BASE_BINARY CANDIDATE_BINARY MAIN_BINARY
    TMPDIR=SCRATCH python3 docs/evidence/classic-camera-dma/test-constant-load.py

The helper's four unit fixtures cover zero/nonzero offsets, unequal constants,
binary-hash mismatch and unsupported encoding. They are explicitly synthetic
parser tests, not emulator or hardware observations.

## Qualification

See rmw-stop-checks.json for executed commands, totals and control comparisons.
All Cargo commands use cargo +1.99.0, nice -n 10 and isolated targets. Required
workspace checks use an empty HOME with explicit Cargo/Rustup locations; only the
ROM-enabled test sets ESP32SIM_ROM_DIR. No JIT implementation is changed.

The ROM-enabled workspace passes 1,030 tests. The plain workspace passes 1,002
with 50 ignored; it was rerun after mutation restoration. All 12 C7 integration
tests and the new two-port command-readback unit test pass. Native/WASM Clippy,
WASM build, all eight demos and the evidence privacy check pass. The 55 golden
payloads and two harness files are unchanged against both controls. Seven public
S3 camera frame receipts, console hash and normalized instruction/IRQ output match
the accepted merge base.

All 96 semantic mutants are runtime-killed with no compiler-error kills: C4 23,
C5 12, C6 35 and C7 26. Each C7 mutation produces a named assertion failure.
The inherited suites also contain unwrap expectations and explicit panic guards;
`rmw-stop-mutations.json` preserves that distinction rather than labeling every
runtime failure an assertion. The retained-START mutant kills both new stop tests.

Codegen remains 10/21 equal selected bodies against each control; all 22 differing
function comparisons are explained in `rmw-stop-codegen.json`, not normalized into
equality. Tick remains
2,207 versus 2,237 static instructions, with camera dispatch behind the existing
clock/running-DMA guard. The zero/nonzero constant relocation is now independently
verified by the reproduction helper from actual binary bytes. No zero-cost CPU
measurement is claimed.

## CPU comparison

PENDING. No timing or publication. C8 remains gated on independent exact-SHA ACCEPT.
This is native register/DMA remediation, not compiled classic framebuffer acquisition,
PSRAM, JPEG, per-PCLK fidelity, product integration or silicon qualification.
