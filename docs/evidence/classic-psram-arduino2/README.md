# Stock Arduino-ESP32 2.0.17 PSRAM qualification

Local qualification on accepted low/high prerequisite
`5947b6d2a0ae6aacdf5a4340533afea0a6597210`, which descends from P4
`39caacbebf54dbc33718033dbad3975cade2cffa`. No emulator production source,
SDK, board configuration, camera code, CLI, WASM interface or golden changes.

The earlier normal-only prerequisite failure is retained in
[prerequisite-negative.md](prerequisite-negative.md), with its original
`inputs.json`, `result.json` and `checks.json`. Those files describe the
historical `05c5e57f8f0e74547da07d8e891726935a840bbc` replay, not this passing
qualification. Use that historical commit to reproduce the negative harness.
Current qualification is in `qualification.json`, `provenance.json`,
`qualification-checks.json` and `mutations.json`.

## Inputs and real guest path

Public `docs/fixtures/classic-psram-arduino2/p5_probe/p5_probe.ino`, stock
Arduino-ESP32 2.0.17, FQBN `esp32:esp32:esp32cam`, GCC 8.4.0
esp-2021r2-patch5, packaged IDF v4.4.7-dirty, real ECO3 mask ROM. The isolated
recipe installs the stock package; no SDK patches, sdkconfig overrides,
unicore substitute, clock changes, explicit sketch PSRAM init or test override.
The native harness uses the existing pre-boot PSRAM setter; classic CLI PSRAM
configuration and PSRAM-on WASM are not qualified.

`provenance.json` pins all five firmware artifacts, SDK/core source hashes,
51 linked archives, relevant sdkconfig settings, symbols and the linked weak
himem body. All 47 SDK archives also linked by P0 and the complete sdkconfig
are hash-identical to P0. Dual-core, flash 80 MHz, PSRAM 80 MHz, BOOT_INIT off
and light heap poisoning are retained. This is the 80/80 single-byte EB/38
command path documented by the prerequisite, not a new physical clock model.

The harness only observes symbols/breakpoints and reads guest state. It never
preprograms cache/MMU, writes PSRAM contents, stubs guest functions, or retries
a reboot. It asserts zero stubs, stub hits and reboots. Present boots:

1. Reach the real SDK `esp_spiram_test`, with PRO/APP low/high controls and all
   128 guest-programmed MMU entries checked.
2. Reach the real `esp_spiram_add_to_heapalloc` only after the genuine test
   succeeds. Before allocator metadata is written, verify all 131072 stride-32
   test words across both 2 MiB halves against IDF's expected address pattern.
3. Continue through actual heap registration into the unmodified Arduino
   startup and public sketch; `psramFound()` is true.
4. Allocate 1 MiB in mapped PSRAM, volatile bytewise write/read with zero
   errors and independent FNV1a `20c19dc5`, then free it.
5. Allocate another 1 MiB, write/read a distinct pattern with zero errors and
   FNV1a `9dce9dc5`, free it, and recover exactly the initial available heap.
   This qualifies free followed by allocation, not the `realloc` API.
6. Repeat from a fresh machine and require identical guest console. Retained
   host instruction/exception/interrupt/per-source totals also match exactly.

Observed serial:

    P5 idf=v4.4.7-dirty found=1 size=4192123 heap=4194303 free=4192123
    P5 pass=1 allocation=1048576 address=3f800884 errors=0 fnv1a=20c19dc5
    P5 pass=2 allocation=1048576 address=3f800884 errors=0 fnv1a=9dce9dc5
    P5 free-before=4192123 after-first=4192123 after-second=4192123
    P5 result=pass

Identical firmware with no PSRAM reports ID `ffffffff`, `found=0 size=0 heap=0
free=0`, allocation unavailable, and `result=no-psram`; it does not fall back
to internal RAM or execute a positive allocation pass.

## Exact size accounting (not a 4194304-byte getter claim)

The device has 4194304 physical bytes. The linked
`esp_himem_reserved_area_size` is the weak body returning zero, despite bank
switch configuration with reserve=8. Both external BSS/noinit ranges are
empty. P4b's linked/source-derived range accounting applies unchanged:

    start = 0x3f800000
    end = start + 4194304 - 1 = 0x3fbfffff
    heap_caps_get_total_size = end - start = 4194303

Arduino 2.0.17 `cores/esp32/Esp.cpp`, `EspClass::getPsramSize`, does NOT call
`heap_caps_get_total_size`. It calls `heap_caps_get_info` and sums free and
allocated payload bytes. On this empty external heap, IDF v4.4.7 subtracts:

    heap_t: 20 bytes (five 32-bit fields)
    TLSF control: 36 bytes
    first-level bitmap: 16 * 4 bytes
    second-level heads: 16 * 32 * 4 bytes
    light-poisoning head/tail: 8 + 4 bytes
    getter = initial available = 4194303 - 20 - 2148 - 12 = 4192123

For the 4194283-byte TLSF input, bit length 22, SL log2 5 and alignment log2 2
produce FL count 22 - 7 + 1 = 16. These are the actual 32-bit guest sizes,
not host pointer sizes. The observed getter and all three free-heap readings
are pinned to 4192123, not loosely accepted as any value near 4 MiB.

Public source anchors, ESP-IDF v4.4.7
`38eeba213aa695aabfd6d89aa9f5078dbe5a94c3`:

- `components/esp_hw_support/port/esp32/spiram.c:192-209,254-306`:
  weak reservation and inclusive-end construction.
- `components/heap/heap_caps.c:505-514`: registered range total.
- `components/heap/multi_heap.c:80-86,109-132,390-408`: heap header, TLSF
  setup, free/allocated accounting.
- `components/heap/heap_tlsf.h:62-108` and `heap_tlsf.c:473-489`:
  guest structure sizes and size-class metadata.
- `components/heap/multi_heap_poisoning.c:51-61,369-386`: 12-byte light
  poisoning overhead removed from reported payload/free bytes.
- Arduino-ESP32 tag 2.0.17 `cores/esp32/Esp.cpp:getPsramSize` and
  `esp32-hal-psram.c:psramInit/testSPIRAM`: real getter and startup chain.

Downloaded source hashes and linked weak-body disassembly are retained in
qualification/provenance JSON. No silicon, himem API or >4 MiB claim.

## Reproduction

In an isolated worktree, fetch ROM assets normally, then:

    bash docs/fixtures/classic-psram-arduino2/recipe.sh OUTPUT
    python3 docs/fixtures/classic-psram-arduino2/provenance.py OUTPUT provenance.json
    ARDUINO2_FIRMWARE_DIR=OUTPUT/fw-A2 ESP32SIM_ROM_DIR=ROMS \
      CARGO_TARGET_DIR=TARGET CARGO_BUILD_JOBS=4 \
      nice -n 10 cargo +1.99.0 test --release -j4 -p esp32sim \
      --test classic_psram_arduino2 -- --ignored --nocapture --test-threads=1

Both named external tests pass; the present test boots twice. The fixtures
are intentionally developer-input tests, ignored by plain tests and excluded
by CI's `--skip external_`. No firmware binaries or CI golden are added.

A second fresh build directory with the same stock install also passes all
three boots with identical host stop reports. Bootloader and partitions are
byte-identical. Application images differ only in the embedded ELF SHA256
(offsets 176..207) and its dependent image checksum/trailing SHA256; all other
bytes match. ELF debug/build paths and map files differ. Both exact manifests
are retained; full ELF/application hash reproducibility across different
absolute build paths is NOT claimed.

## Verification and mutation scope

`qualification-checks.json` records standard gates and `mutations.json` names
actual semantic probes/kills, including survivors. Compiler failures do not
count as kills. Mutations run in a separate clean detached worktree with an
owned target, restoring sources in finally blocks. A restored fixture control
uses a new target so cross-worktree artifact reuse cannot fake restoration.

Both new named external tests must fail at a runtime assertion under at least
one mutation of the inherited PSRAM implementation. Bytewise readback mutants
also exercise the new volatile sketch path, beyond mere startup detection.
Inherited P1-P4b mutation scripts cover independent register/MMU/cache rules;
unchanged acceptance does not imply that a self-consistent firmware readback
is an independent physical-address oracle. The fixture tests real allocator
behavior but adds no allocator implementation to the emulator.

Actual mutation receipts: P5 7/9, P1 24/24, P2 11/11, P3 30/30, P4b 17/17,
and original P4 fixture 5/7: 94/98 total. The two `sram-slot-offset` and
`sram-page-shift` probes survive both P4 and P5 (four probe survivors), because
same-layout guest/host reads can stay self-consistent. Independent P2 slot/shift
probes kill both rules. P2 `ignore-bounds` includes a host bounds panic as well
as named test failures; it is not presented as a clean assertion-only kill.
All seven new P5 kills are named CLI runtime assertion failures, including both
byte-load/store readback failures and the absent-ID failure. Fresh restored P5
control passes. No survivors or compilation failures are hidden.

Standard gates pass: native/WASM Clippy, 1005 ROM-enabled tests, 975 plain tests
with 56 ignored, production WASM/eight demos/JIT handoff, privacy/whitespace.
Final combined P5/P4b/P4 fixture run passes 6/6 tests and focused PSRAM unit
checks pass 18/18. `qualification-checks.json` preserves commands/results.

All existing 55 golden blobs remain byte-identical to accepted P4b. Production
Rust changes versus that prerequisite are empty, including camera/I2S and all
hot functions. Standard checks include empty-HOME ROM/plain workspace runs,
native/WASM Clippy with warnings denied, production WASM and the eight standard
demos plus JIT handoff, privacy and whitespace. No JIT implementation changes.
This is native PSRAM qualification only: no camera, PSRAM-on WASM, hardware,
production recovery, manual clock variants or unsupported cache-mode claims.

## CPU comparison

PENDING. No timing or publication; production emulator code is unchanged from
the independently accepted prerequisite.
