# Classic PSRAM cache/coherence (P3)

Local-only new feature on the conflict-free merge of P1
97e22dbbbaad5a94d6180778331ebbc740c5a54d and P2
250969d7c908b0fda1fca0b917f1860c283ae0b0. Neither input is rewritten.
The merge keeps both accepted test suites and backing/mapping rules. No camera
files, tick scheduling, host/WASM ABI, allocator or guest function interception.

## Functional contract

Sources: ESP-IDF v5.1.4 (d7b0a45ddbddbac53afb4fc28168f9f9259dbb79),
components/soc/esp32/include/soc/spi_reg.h:903-1016,
components/soc/esp32/include/soc/dport_reg.h:178-314,418-542 and
components/esp_psram/esp32/esp_psram_impl_quad.c:1068-1125. These are public
source-derived register transactions, NOT a trace from a successful PSRAM boot.
P0's captured probes did not reach successful cache setup.

The classic adapter retains offsets 0x54..0x7c in the existing controller
register RAM, bypassing the shared engine's FSM/FIFO interpretation. Actual
classic FIFO words at 0x80..0xbc still use the shared FIFO. SPI0 and SPI1 remain
separate. SRAM_CMD is retained register state, not an implemented automatic
SPI0 command/reset sequencer. Device mode is controlled by the accepted SPI1
command path; legacy rev0/delay-clock encodings are outside qualification.

Exactly one bus-owned allocation serves SPI1 commands and CPU mappings. The
command dispatcher now supplies that allocation and honors DirtyMem::Flash
versus DirtyMem::Psram: PSRAM writes cannot invalidate flash aliases. Coherence
is synchronous write-through. There are no cache lines, delayed writeback,
cache latency or hit-rate claims, and no new translated/fast memory pointers.
Cache invalidation completes synchronously without clearing payload bytes;
subsequent helper-based accesses translate and check current registers again.

Access ordering:

1. Absent backing, invalid/unmapped table, divergent live tables or physical
   overflow gives Fault::Unmapped, with no partial cross-page writes.
2. A present valid mapping must have PRO cache enabled (CACHE_CTRL bit 3),
   DRAM1/OPSDRAM unmasked (CTRL1 bits 3/5), 32 KiB pages (CTRL1 6..8 zero),
   and normal addressing (HL/split clear). A live APP must satisfy the same
   restrictions; held/stalled/reset APP does not gate PRO-only startup.
3. SPI0 CS1 must be enabled; CACHE_SCTRL must select QIO not DIO, 24-bit
   addressing, read dummy enabled and the relevant user read/write command
   enabled. The corresponding command must be eight-bit EB/38. Dummy count
   and burst length are retained but not timed. This conservative functional
   subset requires the read-dummy control for both directions. It does not
   qualify DCLK/double-byte opcodes, HL/even-odd mappings or other page sizes.
4. The shared SPI1 device must be in QPI. Unsupported cache/device states give
   Fault::Prohibited and preserve bytes until restored. SPI1 commands themselves
   continue to operate with CPU cache disabled, as required for initialization.

No cache status is polled periodically. Gates execute only inside the existing
cold external-RAM access helpers. The P2 unconditional external-window guards
remain an explicitly disclosed memory-access cost; this is not an idle-cost-free
or behavior-preserving speedup claim.

P0 lifecycle is preserved: host presence/capacity and allocation identity survive
reboot; all bytes become zero, SRAM mappings become invalid, and replacement
peripherals clear guest QPI/reset/cache register state. Presence is not silently
removed at reboot. Host absence remains absent. Device reset via 66/99 retains
payload, unlike the deterministic emulator machine power-cycle policy.

## Named tests and mutations

Four new native tests in esp32/src/psram_cache_tests.rs exercise public bus MMIO:

- cache_trace_registers_never_alias_fifo_or_other_controller
- cache_command_cpu_coherence_remap_and_flash_dirty_isolation
- cache_gates_fault_and_restore_without_losing_bytes
- cache_reboot_resets_controls_device_mappings_and_bytes_in_place

P1's five tests remain unchanged. P2's five tests retain their assertions and
now explicitly program the cache/device preconditions rather than depending on
an always-enabled mapping. Both table banks, 128 reversed pages, page edges,
remapping and rejected DMA/fetch behavior remain covered.

Run with cargo +1.99.0 and a caller-owned CARGO_TARGET_DIR:

    nice -n 10 cargo +1.99.0 test --release -p esp32 --lib psram
    python3 docs/evidence/classic-psram-cache/mutate.py MUTANT_TREE OUTPUT
    python3 docs/evidence/classic-psram-spi/mutations.py MUTANT_TREE OUTPUT
    python3 docs/evidence/classic-psram-mmu/mutate.py --repo MUTANT_TREE --out OUTPUT

Mutation scripts require an idle disposable source tree, restore in finally and
require test failure rather than counting compiler errors as kills. The P3
script also requires a panic and its named FAILED test line. Reproduction of
full native/WASM gates, no-PSRAM controls and linked disassembly is recorded in
the result files accompanying this receipt.

## Limits

No successful IDF/Arduino PSRAM boot, guest memory test, guest heap registration,
camera allocation or physical-hardware qualification. No actual PSRAM-on WASM
interpreter/JIT fixture claim: constructor and mapped-memory parity remain P7.
No classic DMA expansion. The unchanged no-PSRAM WASM suite is a regression gate,
not evidence for the future PSRAM guest milestone.

## Verification

Frozen Rust implementation: 1f7a77df860513bf7432553a13d1a415bf06794e.
checks.json records commands and zero exit codes. All Cargo commands use
nice -n 10, cargo +1.99.0 and an out-of-tree CARGO_TARGET_DIR. Native tests use
an empty HOME, explicit RUSTUP_HOME/CARGO_HOME and only ESP32SIM_ROM_DIR in the
ROM-enabled run; the plain run has no ESP32SIM_* variables.

- Native workspace/all-targets Clippy, warnings denied: PASS.
- Release wasm32 esp32sim-wasm jit-tests Clippy, warnings denied: PASS.
- Release workspace CI-policy tests: 999 passed, zero failed.
- Plain release workspace: 971 passed, zero failed, 50 ignored.
- Production WASM build: PASS. All eight standard demos and JIT handoff: PASS.
- Evidence privacy and whitespace checks: PASS. No JIT implementation changes;
  the additional wasm-jit-test.sh gate is not applicable.
- Targeted inherited/new PSRAM tests: 14 passed, including the restored-source
  rerun after mutations. Disposable mutant tree has an empty tracked diff.
- Semantic mutations: P3 30/30, inherited P1 24/24 and inherited P2 11/11 killed
  (65/65 total). mutation-summary.json and the three mutation tables retain
  each rule/test outcome; P3/P1 require named runtime panics, not compile errors.
  P2's bounds-removal mutant intentionally exposes a host bounds panic, as in
  its accepted receipt; other P2 kills are named runtime failures.
- All 56 tracked golden blobs match P0/main exactly (goldens.json).

No-PSRAM ROM controls (controls.json) use freshly fetched public demo assets and
the retained stock P0 A2/A30 fixture binaries with hashed inputs. Classic hello
has identical console SHA-256 ddb22f96bb04830ff8abaf2305bf193d3b333d1bfa2e383cf0709f05ecbe9c5c
and identical stop pins: core0 4337816 + core1 426561 instructions, 1458 exceptions,
187 interrupts. A2/A30 still report found=0, heap=0, free=0, result=no-psram.
Their serial differences from P0 are exactly the accepted P1 absent-ID correction
00164020 -> ffffffff; controls.json retains the corresponding changed stop pins.
They are negative controls, not successful PSRAM boot evidence.

Normally linked release CLI disassembly is retained against both P0/main (linked/)
and the conflict-free P1+P2 merge ae62bfd4 (merge-linked/). Reproduce with:

    nice -n 10 cargo +1.99.0 build --release -p esp32sim --bin esp32sim
    python3 docs/evidence/classic-psram-mmu/linked.py --base BASE_BINARY --head HEAD_BINARY --out OUTPUT

Static disassembled instruction counts, including alignment padding:

| Function | P0/main | P1+P2 merge | P3 |
|---|---:|---:|---:|
| tick | 2207 | 2207 | 2207 |
| fetch | 122 | 133 | 133 |
| read8 | 106 | 114 | 113 |
| read16 | 125 | 133 | 133 |
| read32 | 171 | 188 | 188 |
| shared store | 2736 | 2750 | 2752 |

Tick has the same mnemonic sequence and source body on all three revisions.
Versus P0, P1/P2 change field offsets (+16 for affected peripheral fields, +40
for affected bus-tail fields); code/static data are relocated. P3 adds no fields
or tick checks. Its merge-to-head tick differences are static-data/import-stub
relocations, not cache polling. The raw disassembly and tick-operands.json make
these differences inspectable; byte identity is not claimed.

The P2 external-window checks still execute on memory access even with absent
PSRAM. Fetch/read16/read32 have unchanged mnemonic sequences versus the merged
base. read8's one-instruction decrease is only removed trailing UDF alignment
padding (read8-normalized.diff), not an executed-path optimization. Store's +2
instructions and changed loop layout come from supplying the PSRAM slice and
discriminating dirty-memory records in the inlined command-dispatch arm
(store-relocation-stripped.diff); this is event-time work, not per-tick polling.
No performance conclusion is drawn from static counts.

## CPU comparison

PENDING. No timing performed; central night-only scheduling owns measurement.
