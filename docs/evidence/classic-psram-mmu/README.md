# Classic ESP32 SRAM mapping (P2)

Local-only feature slice on P0 `9908529f323e5c7e6b72bd348debecf1b4cf0b1d`
(upstream base `142805d5399e0c3cf25995fe21ba524e75a2b3cb`). Emulator code
revision: `f5def98ce4b519ca6ad339c96bbafd29f7f5560e`.
No publication, timing, hardware verification or successful guest PSRAM boot is claimed.

## Scope and limits

One optional bus-owned 4 MiB allocation; the existing size setter accepts exactly
0 or 4194304 bytes and rejects other sizes without changing bytes, tables or
allocation. Default remains absent. Existing native/WASM CPU bus helpers resolve
external data accesses using the existing MMU arrays, not a flat memory fallback.
There is no classic fast_mem backing pointer or new ABI layout.

ESP-IDF v4.4.7 and v5.1.4 `components/esp_hw_support/port/esp32/cache_sram_mmu.c`
define the PID 0/1 slot offset 1152, 128 entries, 32 KiB shift and data window.
Other PIDs/page modes are not qualified here. Flash retains its 64 KiB translator
and separate bytes. Instruction fetch is prohibited; DMA remains internal-only.

PRO-only startup is allowed while APP is held. Once APP is running (clock enabled,
reset and stall cleared), divergent live entries fault instead of silently selecting
PRO. This is the ratified emulator restriction, not a hardware MMU claim. Mapping
writes and core release take effect on the next access because translation is
recomputed synchronously; no cached mappings or polling is added.

Each CPU access validates every byte before writing, including a crossing into a
noncontiguous or invalid page. Existing CPU alignment checks are unchanged; the
bus already permits unaligned data helpers, exercised by the page-crossing tests.
Absent, invalid and physical overflow fault Unmapped. Guest reboot clears SRAM
tables and bytes but preserves capacity and allocation identity. This is a
deterministic emulator power-cycle policy, not DRAM retention behavior.

P1's SPI adapter is not included. P3 must connect that adapter to this allocation,
qualify command/mapped coherence, and add cache/device/mask/page-mode gates before
claiming functional PSRAM. These gates are deliberately not claimed by P2. P7 must
qualify actual mapped guest execution in WASM interpreter/JIT and constructor
argument propagation. No camera, allocator, firmware init, or add-on claim.

## Native rules and semantic negatives

Five `psram::tests::sram_*` tests exercise both table banks through MMIO, all 128
pages in reverse physical order, byte/halfword/word widths at each page's edges,
noncontiguous crossings, physical aliases, remap/invalid/absent/overflow faults,
full-width write atomicity, APP release/divergence, flash separation, fetch/DMA
exclusion, loader routing, rejected capacities, reboot and independent creation.

`mutations.json` records eleven rebuilt semantic negatives and their named killing
tests. Compilation failure is not a kill. In a disposable worktree of the code
revision, run:

    python3 docs/evidence/classic-psram-mmu/mutate.py --repo MUTANT --out OUTPUT

The harness restores each edited file in a finally block and puts its target under
OUTPUT. The required SRAM slot-offset and 32 KiB-shift negatives are included.
Bounds-removal is killed by the expected fault becoming a host panic; partial-store
is killed by changed bytes after a failing cross-page write.

## Hot-path comparison

The base P0 Rust/build inputs are identical to underlying upstream main. Commands
use cargo +1.99.0, nice -n 10, independent targets and no CPU timing. Toolchain:
rustc 1.99.0 (b940084d7 2026-09-28), cargo 1.99.0 (5f94df478 2026-08-27),
AArch64 macOS; Apple clang 21.0.0 (clang-2100.3.34.2).

Normally linked release CLI binaries (fat LTO, workspace defaults):

    cargo +1.99.0 build --release -p esp32sim --bin esp32sim
    python3 docs/evidence/classic-psram-mmu/linked.py --base BASE_BINARY --head HEAD_BINARY --out OUTPUT

`linked.json` pins both binary SHA-256 values. `linked/*.disasm.gz` retains actual
hot-function instructions and resolved call targets, without source paths.
Static instruction totals (not executed-instruction counts or timings):

| Function | Base | P2 |
|---|---:|---:|
| tick | 2207 | 2207 |
| fetch | 122 | 133 |
| read8 | 106 | 114 |
| read16 | 125 | 133 |
| read32 | 171 | 188 |
| shared store | 2736 | 2750 |

The tick mnemonic sequence is unchanged: no PSRAM polling or new tick calls.
Bytes differ due to shifted bus-field offsets (+24 for affected scalar/tail fields)
and linked code/static-data relocation. `tick-operands.json` exposes changed
non-branch operands. Existing cache/MMIO/clock/peripheral operations remain.

Memory functions are NOT byte-identical: load/store/fetch add a window guard,
branch to the cold translator/fetch prohibition, and compiler block/register-layout
changes. For example read32 starts with lsr #22, cmp #0xfe, b.eq before the original
MMIO check; this guard executes even when PSRAM is absent. This is a disclosed
new-feature cost, not a behavior-preserving speedup or an idle-cost-free claim.
The shared store remains one helper, not three duplicated width-specific buses.

Optional pre-LTO comparison:

    python3 docs/evidence/classic-psram-mmu/disasm.py --base BASE_TREE --head HEAD_TREE --out OUTPUT

This emits release assembly, assembles it with clang and disassembles it with
llvm-objdump. `pre-lto/` and `disassembly.json` preserve its separately labeled
results. Pre-link opcode equality does not prove relocated call-target equality.

## Verification

`checks.json` records the frozen-code replay. All builds use nice -n 10,
cargo +1.99.0 and a caller-owned CARGO_TARGET_DIR outside the source tree.
Native tests use an empty HOME with explicit toolchain/cache locations, and no
ESP32SIM_* settings except ESP32SIM_ROM_DIR for the ROM-enabled run.

- `tools/fetch-demo-assets.sh --no-linux`: PASS (fetched in this worktree).
- `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings`: PASS.
- `cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings`: PASS.
- `ESP32SIM_ROM_DIR=ROM_DIR cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_`: 990 passed, 0 failed.
- `cargo +1.99.0 test --release --workspace`: 962 passed, 0 failed, 50 ignored.
- `RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh`: PASS.
- `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel`: eight demos and JIT handoff PASS.
- `node tools/check-evidence-privacy.mjs`: PASS; retained disassembly is symbols and instructions only.
- `git diff --check`: PASS. All 56 tracked paths containing /golden retain identical blob IDs against P0; no golden updates.
- `mutate.py`: 11/11 semantic mutants killed by failing tests, not compilation errors.
- No JIT code changes; the additional wasm-jit-test.sh gate is not applicable.

WASM build scripts expect a source-tree target path when copying their output;
the run used a symlink to its own out-of-tree target, not another checkout's assets.
The linked head binary hash was reverified after the final full-check replay.

## CPU comparison

PENDING. No CPU timing run was performed.
