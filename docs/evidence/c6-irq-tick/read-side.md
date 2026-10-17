# C6 read-driven interrupt correction (EX090)

Base: `142805d5399e0c3cf25995fe21ba524e75a2b3cb` (the pinned upstream/main
used for this experiment, still upstream/main when this correction was tested).
The earlier gate at `a41db5cd14ac94d6d6918145952d524c51e633c1` misses USB
FIFO read-driven interrupts. Its old timing samples are historical only.

## Regression and correction

Queue 65 bytes, consume 63, clear RECV_PKT and synchronize interrupts. The
first packet has one byte left and the second packet is queued. A real guest
executes `lw t0,0(a0); wfi`, with a handler that increments s0. That load drains
the first packet and presents the second; USB's IRQ rises before the next
dispatcher tick. Comparing sources only before/after that tick misses the
transition. Before the correction the new guest test fails with mcause=0,
expected 0x80000003. With the correction, the handler executes.

Only reads of the USB FIFO word invalidate the gate, including supported byte
and halfword lane aliases. This adds one address comparison/OR on the generic
C6 peripheral-read path, not a source scan on every read, a shared dispatcher
change, or unconditional round refresh. It conservatively invalidates even if
a FIFO read presents no new packet. Existing dirty state cannot be cleared by
a later unrelated read. No struct layout, instruction timing, quantum, deadline,
idle chunk, or other chip's source is changed.

Register anchors: ESP-IDF v5.5.4
`components/soc/esp32c6/register/soc/usb_serial_jtag_reg.h`, EP1_REG at +0,
INT_ENA at +0x10, INT_CLR at +0x14, SERIAL_OUT_RECV_PKT bit 2. The queued-packet
presentation behavior is the existing emulator model, not a new silicon claim.
No hardware validation is claimed.

## Read-side audit

Audited the C6 `device_set!` entries, adapters, `DeviceSet::pre_access`, bus
reads and subword write read-modify-write path at this source tree:

| Read path | Source effect / invalidation |
| --- | --- |
| USB EP1 FIFO | Pops a byte, may present the next packet and set raw bit 2. The correction invalidates for the aligned FIFO word. |
| Other USB registers | Status/config/raw reads do not mutate IRQ state. |
| UART0/1 FIFO | Pops a byte only; the current model leaves raw FULL/TOUT flags unchanged on read. Clear/threshold writes recompute and already invalidate. No new UART behavior is introduced. |
| I2C FIFO | Pops receive data; raw/enable fields remain unchanged. Optional-source cache is not changed by reads. |
| GPIO IN/IN1 board delivery | Existing `deliver_board_inputs` result is ORed into dirty before the read. GPIO register reads themselves do not clear/recompute status. |
| Systimer, TIMG, LEDC, MCPWM, RMT | Read stored/latching values, timer phase or status without changing IRQ raw/enable fields. RMT compact adapter delegates to the same read-only implementation. |
| I2S, PARLIO, GDMA | Read status/config/descriptor state without changing raw/enable. PARLIO may allocate its cold TX object, whose initial IRQ is zero. C6 GDMA remapping delegates to read-only shared GDMA. |
| AES, SHA, RSA, ECC, SARADC | Result/status reads do not execute operations or acknowledge interrupts. Completion/clear paths are writes/ticks/DMA with existing invalidation. SHA/SARADC sources are not mapped in this C6 table. |
| Radio / Wi-Fi MAC | Status reads do not clear events or complete work; unknown-access/debug logging does not change IRQ state. |
| Matrix, INTPRI, PLIC claim | Pure reads. In this model PLIC claim reports pending without clearing it. Controller/routing writes already invalidate. |
| SPI memory/cache, LP, PCR, analog master, efuse, IO mux, RNG, assist debug, register RAM | No read-driven mapped IRQ transition. RNG mutates its generator state, not a source. `pre_access` updates I2C clock config, ADC/RNG time and radio logging only. |
| Byte/halfword writes | May read a peripheral to merge lanes, but every dispatched write already sets dirty, including a read of USB FIFO during that merge. |

This audit covers currently modeled C6 paths, not hypothetical clear-on-read
hardware behavior or future device additions. Direct host mutation of public
peripheral fields still needs `sync_irq()` as before.

## Tests and mutations

`esp32c6/tests/irq_read.rs` adds the real-guest regression, seven supported
width/lane cases preserving dirty state across an unrelated read and quiet tick,
and a control for clean unrelated status/controller/GPIO reads. The three
original tick regressions remain unchanged.

Commands (all Cargo runs use nice 10, toolchain 1.99.0 and external target):

    cargo +1.99.0 test --release -p esp32c6 --test irq_read --test irq_tick
    python3 docs/evidence/c6-irq-tick/mutate.py "$OUT/mutations-tick"
    python3 docs/evidence/c6-irq-tick/mutate_read.py "$OUT/mutations-read"

Fresh results: the upstream-base guest test passes; the original gate fails
with mcause=0; the corrected tree passes all six IRQ tests. All eight mutations
produce assertion failures (exit 101, not compile errors), and restored tests
pass again. `read-mutations.json` and `read-tick-mutations.json` retain results.

| Additional mutation | Killing test |
| --- | --- |
| Drop FIFO read invalidation | next_usb_packet_irq_is_presented_after_guest_fifo_read |
| Match only the exact address, missing lane aliases | fifo_read_widths_preserve_invalidation_until_sync |
| Invalidate every peripheral read | unrelated_reads_do_not_invalidate_a_clean_irq_gate |
| Assign dirty instead of ORing, losing a pending change on a later read | fifo_read_widths_preserve_invalidation_until_sync |

## Fresh required checks

`read-checks.json` records commands and exit codes, `read-test-totals.json` the
parsed totals. Empty HOME is paired with explicit CARGO_HOME/RUSTUP_HOME so the
pinned toolchain is usable, and the CI run gets only the fetched ROM directory
as simulator input configuration. The plain run has no ROM environment.

- `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings`: PASS.
- `cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings`: PASS.
- `ESP32SIM_ROM_DIR=... cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_`, empty HOME: 991 passed, 0 failed/ignored.
- `cargo +1.99.0 test --release --workspace`, empty HOME, no ROM environment: 963 passed, 0 failed, 50 ignored.
- `RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh`: PASS (checkout target symlinked to external target for its literal copy path).
- `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel`: eight PASS.
- `node tools/check-evidence-privacy.mjs`: PASS, including final tracked evidence.
- `cargo +1.99.0 build --release --bins`: PASS on both arms.

No goldens or JIT code changed; wasm-jit-test is not applicable.

## Fresh exact execution

Apply the same `retired-probe.patch` to both arms, build release C6 binaries,
then restore the patch. Run without collecting timing:

    python3 docs/evidence/c6-irq-tick/exact_work.py "$OUT/exact-work" "$BASE_PROBE" "$FIX_PROBE" "$ROM_DIR" "$PUBLIC_STARTER_DIR"

One 30-guest-second run per workload per arm (four runs total), retaining full
console SHA256, actual architectural retirement and per-line/source interrupts.
The inputs match `inputs.json`; no firmware was rebuilt or substituted.
`read-exact-work.json` contains all results and binary hashes.

| Workload | Retired per arm | Console bytes | Interrupts | Console SHA256 |
| --- | ---: | ---: | ---: | --- |
| hello delay | 10,313,837 | 9,735 | 3,029 | bd69fbb9ac5c677337a293c1ec0ff7941306eec82c499290917a7c42360f86ad |
| public display starter | 1,126,728,507 | 336 | 31,160 | c2935031fcffb102bb9f3c720adfc300db2e16b57ddd47f55537338f947e7929 |

Both arms have zero exceptions, and identical per-line/source interrupt totals
for each workload. This is an execution/output check, not a CPU speed claim.

## Fresh static cost

Run `codegen.py` on both uninstrumented release builds for all three CLI
binaries, and separately compare each C6 binary to its retirement-probe binary.
The five `read-codegen-*.json` summaries use the existing EX230 normalizer.
For each CLI binary, 106 functions selected, 102 identical; all 90 non-C6
selected functions are identical. Four C6 differences versus pinned main:

| Function | Base normalized instructions | Correction | Reason |
| --- | ---: | ---: | --- |
| bus tick | 1,983 | 2,135 | Existing source-change comparisons are now live. |
| optional tick | 277 | 322 | Existing optional-source change result propagates. |
| after_round | 92 | 100 | Tests the dirty/source-change result. |
| periph_read | 225 | 233 | FIFO-word address test and dirty-byte OR/store; existing GPIO helper base-register allocation moves. |

The first three differences are the original optimization. The added read
check is eight static ARM64 instructions on this build's generic peripheral
path: mask, constant materialization, compare/set, dirty-byte load, OR and store.
There is no new IRQ scan on unrelated reads or RAM/fetch path. The probe leaves
all 106 selected hot functions identical on each arm separately. This is scoped
static evidence, not whole-binary identity or an absence-of-cost timing claim.

## CPU comparison

PENDING

No new timing result is collected or claimed for this correction. Exact-work
runs check retirement, console and interrupts only. The original loaded-machine
smoke did not prove correctness and is not a formal CPU table.
