# Arduino-ESP32 3.0.7 classic PSRAM qualification

Base: `3efe000683150fcc50fa2feb6ae5400616e49e8a` (accepted Arduino 2.0.17 qualification).
Rust test/sketch revision: `4d53a2a93234848e939e65cf0ae55c341851160d`.
Local-only qualification, no production implementation change or published API.

## Inputs and scope

The [public recipe](../../fixtures/classic-psram-arduino3/README.md) installs
unchanged Arduino-ESP32 3.0.7 / `esp32-arduino-libs idf-release_v5.1-632e0c2a`,
GCC esp-12.2.0_20230208 12.2.0, FQBN `esp32:esp32:esp32cam`. Actual guest IDF is
`v5.1.4-972-g632e0c2a9f-dirty`. No clock change, unicore substitute, patched SDK,
startup stub or intercepted guest return is used for the accepted controls.
`provenance.json` pins firmware, configuration, Arduino sources and 53 archives
represented by the actual link map. All 52 SDK archives shared with the original
P0 A30 build have identical hashes; the remaining archive is the rebuilt Arduino
core. Map membership can include discarded members; the symbol list and linked
function disassemblies independently identify retained code.

The BOOT_INIT profile explicitly executed here is the stock
`CONFIG_SPIRAM_BOOT_INIT=n`, dual core, 80 MHz, light heap poisoning configuration.
Its complete sdkconfig SHA-256 is
`cc18179bae15798fb66668c72cc246b9d596790806d54d717326fcf17ad2d311`.
Both guest tests require the compiled `boot-init=0` result; the provenance checker
requires the full config hash. This is not a BOOT_INIT=y qualification or a claim
that a compile-time flag alone can replace a matching SDK boot profile.

Only public fixture sketches are used. No binary firmware is committed or
redistributed, so these are external developer-input tests, not new CI goldens.
No camera/I2S, WASM PSRAM, other Arduino 3.x, himem API, silicon, timing or
production-recovery claim.

## Separate initialization and heap proof

The native test loads the real ECO3 mask ROM and all three firmware images,
configures the bus-owned 4 MiB device, and never writes guest mappings, caches,
heap metadata or test words. Breakpoints read the actual PRO/APP mappings and
cache low/high controls at the SDK memory test, then observe the separate
Arduino `psramAddToHeap` and SDK `esp_psram_extram_add_to_heap_allocator` entries.
Before allocator insertion all 131072 stride-32 SDK test words across both 2 MiB
banks must match. Large guest allocations independently prove heap-add completed,
not just that either function was entered or the detected flag was set.

A passive observer counts executed function-entry instructions, excluding
breakpoint observations which stop before the entry executes. In order
`psramInit`, `esp_psram_init`, `esp_psram_extram_test`, `psramAddToHeap`,
`esp_psram_extram_add_to_heap_allocator`:

- Present: `[2,2,1,1,1]`: real startup; repeated Arduino init returns true without
  repeating memory testing or heap-add; the explicit duplicate SDK init returns
  `ESP_ERR_INVALID_STATE=259` without invalidating the heap.
- Absent: `[2,1,0,2,0]`: one failed SDK probe; repeated Arduino init returns false
  from cached failure without a second physical probe. Both startup heap-add and
  an explicit failed-state heap-add reject the request, never entering SDK heap
  registration. The ID response is `ffffffff`, all getter/heap/free values are
  zero and 1 MiB allocation is unavailable, with no internal-RAM fallback.

Both paths require no stubs, zero intercepted calls and zero reboots. Successful
boots repeat with byte-identical guest console. `qualification.json` retains the
complete serial hashes and exact host instruction/exception/IRQ reports; these
are correctness controls, not CPU measurements. Instruction observation can
change scheduling relative to an unobserved CLI run; no counter equivalence to
such a run is asserted.

## Exact SDK range and allocator accounting

Physical device, Arduino getter and `heap_caps_get_total_size(MALLOC_CAP_SPIRAM)`
are each 4194304 bytes. The linked weak `esp_himem_reserved_area_size` returns
zero even though the sdkconfig enables bank switching/reserve=8. The actual
linked `esp_psram_init` writes end=start+available_size; `heap_caps_get_total_size`
sums end-start. Unlike the earlier Arduino 2 fixture, the IDF5 range does not
lose a byte, and Arduino 3's getter returns the raw total rather than payload.

The empty free payload is exactly:

    4194304 - heap_header(20) - TLSF(36 + 16*4 + 16*32*4) - poisoning(12)
    = 4192124

Both 1 MiB volatile bytewise write/read passes succeed at `3f800884`, with zero
errors and independently computed FNV-1a hashes `20c19dc5` / `9dce9dc5`. Free
before, after first free and after second free is exactly 4192124. This qualifies
free followed by reallocation, not the separate realloc API. The guest allocator
payloads use the lower bank; the real SDK memory test independently covers both.

Source anchors: Arduino 3.0.7 `cores/esp32/Esp.cpp:149-154`,
`esp32-hal-psram.c:44-105`, and the early/system-init split in
`esp32-hal-misc.c`; ESP-IDF v5.1.4 `components/esp_psram/esp_psram.c:99-114,
119-150,196-228,285-336`, `components/heap/heap_caps.c:516-535`,
`multi_heap.c:76-82,129-151`, and `multi_heap_poisoning.c:49-62`.
TLSF layout is corroborated by `espressif/tlsf` commit
`d2e28f872472ffc6a704faae65ddee1f24e2dfba`, `tlsf_common.h:90-123` and
`tlsf.c:598-614`: input 4194284 bytes has bit length 22, SL log2=5 and
alignment log2=2, so FL=22-(5+2)+1=16 and SL=32.
Source hashes are in `qualification.json`; linked disassemblies corroborate the
actual SDK weak return, range writes, duplicate guard and 20-byte header.
The public v5.1.4 source tag is corroboration, not a claim that it equals the
bundled dirty SDK revision. The SDK binary archives, full config, source hashes
and guest outcomes are pinned separately. No hidden source-version substitution.

Two fresh stock sketch builds pass identical serial and host controls after all
mutated Arduino source bytes are restored. Bootloader and partition bytes match.
The 292400-byte applications differ in 68 bytes, all within compile-time text,
embedded ELF SHA256 and trailing digest fields (`build-comparison.json`). ELF
paths/maps and full app hashes are not claimed bit-reproducible across builds.

## Checks actually run

All Rust builds use `nice -n 10 cargo +1.99.0`, four jobs and caller-owned targets.
ROM/plain suites use an empty HOME with explicit toolchain homes; the ROM suite
has only ESP32SIM_ROM_DIR among ESP32SIM variables and the plain suite has none.

- `tools/fetch-demo-assets.sh --no-linux`: PASS in this isolated worktree.
- `bash docs/fixtures/classic-psram-arduino3/recipe.sh ABS_OUTPUT`: PASS,
  including the stock P0 A30 recipe and final P6 recipe/provenance checker.
- New external tests on both fresh builds: PASS 2/2 each, present boots twice.
- Combined P4/P4b/P5/P6 native fixture tests: PASS 8/8.
- Native workspace/all-target Clippy, warnings denied: PASS.
- Release wasm32 Clippy with jit-tests, warnings denied: PASS.
- ROM workspace suite with ignored included and external skipped: PASS 1005.
- Plain workspace suite: PASS 975, 58 ignored, zero failures.
- Production `tools/wasm-build.sh`: PASS.
- `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki
  c6-contiki-net c6-rpl-net panel`: PASS all eight demos and JIT handoff.
- Evidence privacy and authored-file whitespace checks: PASS.
- All 55 existing golden blobs and all P5 fixture/test files unchanged.
- Production Rust diff against the accepted base is empty, including the six
  linked hot functions: this qualification adds no production hot-path work.
  No new hot-path disassembly or CPU-performance result is claimed.

## Semantic mutations

`mutate.py` uses a clean disposable tree. Seven of nine P6 emulator mutations
fail named CLI runtime assertions; both new external tests have semantic kills.
No compiler failures count. Denying low/high, corrupting byte loads/stores,
halving backing, aliasing pages, rejecting live QPI and zeroing absent ID are
killed. The SRAM slot-offset and page-shift probes survive this self-consistent
guest readback, just as in P5; a guest and host sharing the same mistaken mapping
are not independent physical-layout oracles. These are disclosed survivors,
not silently declared kills. A fresh target passes both restored controls.

`state_mutations.py` separately mutates only an owned Arduino installation,
builds negative guest firmware, and restores source bytes in a finally block.
Four of four semantic guest-library mutants fail their named runtime assertions:
failed init re-probes, detected duplicate returns false, failed retry returns
true, and failed heap-add returns true. Original SDK/source/input provenance is
byte-identical after restoration, and a fresh stock build then passes. These
are guest-state assertion sensitivity tests, not emulator changes or accepted
patched firmware. Their actual logs and compact results are retained locally.

Inherited sweeps pass P1 24/24, P2 11/11 and P3 30/30. P2 independently kills
its SRAM slot-index and 32 KiB shift rules, including named layout assertions;
its slot probe shifts 1152 to 1151, whereas the surviving guest probe uses 1153.
Total executed here: 76/78 (72/74 emulator probes plus 4/4 guest-state probes).
`mutations.json` retains every row, named test, exit status and the two survivors.
P2 ignore-bounds has named fault-value assertion failures and a host bounds
panic: it is not an assertion-only kill. All P6 emulator and guest-state kills
are named CLI runtime assertion failures, not compilation errors. Source
restoration is verified before removing the disposable tree.

## CPU comparison

PENDING. No benchmark, push, PR, upstream comment or other publication.
