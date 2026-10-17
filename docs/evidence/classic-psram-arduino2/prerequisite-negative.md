# Stock Arduino-ESP32 2.0.17 PSRAM qualification: BLOCKED

Base: 39caacbebf54dbc33718033dbad3975cade2cffa (standalone IDF P4).
This is a negative prerequisite receipt, not completed Arduino qualification or
an emulator bug claim. No emulator implementation, SDK, sketch, camera code,
golden, CLI option or WASM interface is changed.

## Actual replay

Reuse the public P0 `p0_probe` build from
`docs/fixtures/classic-psram/recipe.sh A2 OUTPUT`. Arduino-ESP32 2.0.17,
stock esp32:esp32:esp32cam, IDF v4.4.7-dirty, dual-core, 80 MHz PSRAM,
BOOT_INIT disabled. Exact retained inputs are in `inputs.json`. This replay
uses the existing public sketch without rebuilding its firmware; archive,
sdkconfig and linked-symbol provenance are the P0 inputs, not a fresh SDK audit.
No unicore replacement, symbol substitution, SDK modification or reboot retry.

The CLI currently rejects `--psram-mb` on classic ESP32 (even 0). Therefore the
external native test uses the accepted bus setter before real ROM boot. This
is explicitly not a CLI-configuration qualification or a new CLI implementation.

Present device:

1. Real `psramInit -> esp_spiram_init -> psram_enable` accepts the device.
2. Real `esp_spiram_init_cache` calls `cache_sram_mmu_set` for CPU 0 and CPU 1.
   Immediately before `esp_spiram_test`, all 128 PRO and APP entries are equal
   to physical pages 0..127, asserted by the harness.
3. At that breakpoint PRO CACHE_CTRL is 0x00010028, APP CACHE_CTRL 0x00004028;
   CTRL1 is 0x00000006 / 0x00000000. Cache-enable bit 3 is set. Low/high mode
   bits PRO 16 and APP 14 are set. The host read at 0x3f800000 returns Prohibited.
   The read is diagnostic only; it never writes external memory or controls.
4. Continuing unchanged firmware enters its real memory test and requests
   `esp_himem_reserved_area_size`, then panics StoreProhibited on its first
   store: PC 0x400dce17, EXCVADDR 0x3f800000, EXCCAUSE 0x1d. It requests reset;
   the harness does not reboot. Heap registration and sketch setup are not reached.
5. `external_arduino2_psram_startup_and_readback` FAILS its named assertion
   "stock Arduino startup must detect PSRAM". This deliberately failing external
   acceptance probe is ignored by plain tests and excluded from CI's include-ignored
   suite via the required `external_` prefix. It must not be presented as green.

Absent device:

- The same firmware correctly rejects ID 0xffffffff and reaches sketch setup:
  `P0 found=0 heap=0 free=0`, allocation unavailable, result no-psram.
- `external_arduino2_no_device_is_rejected` passes without stubs or reboots.

Serial, function milestones, register snapshots, deterministic stop instruction,
exception and interrupt totals were reproduced with and without the breakpoint.
The breakpoint causes one duplicate function-entry trace, not extra retired work.
No wall time is reported as CPU evidence.

## Explanation and source boundary

The source contract deliberately excludes low/high mode:
`esp32/src/psram.rs` at the base SHA, `psram_cache_gate`, lines 30-46. It rejects
PRO/APP low/high bits even when cache enable and MMU entries are valid.
The observed guest controls therefore explain a concrete unsupported-mode
prerequisite; mapping success alone cannot qualify stock Arduino.

Public IDF v4.4.7 (38eeba213aa695aabfd6d89aa9f5078dbe5a94c3),
`components/esp_hw_support/port/esp32/spiram_psram.c:1090-1098`, sets exactly
these controls for PSRAM_VADDR_MODE_LOWHIGH. Its `spiram.c` selects low/high for
dual-core, normal for unicore. The installed A2 sdkconfig has unicore disabled,
SPIRAM_SPEED_80M enabled and BOOT_INIT off. These are source-derived semantics,
not a hardware measurement or an assertion of general dual-core correctness.

Himem bank-switch config is enabled with reserve=8. A linked reserved-area-size
function was entered, but this run never reaches heap insertion. Do not infer
an actual reservation, available heap size, ESP.getPsramSize, successful memory
test, or successful allocation from that config/function entry.

## Reproduction

From an isolated checkout of this branch, fetch ROM assets normally. Build A2
using the unchanged P0 recipe, or supply retained P0 artifacts matching inputs.json.
Then, with a dedicated target outside the control checkout:

    P5_FIRMWARE_DIR=OUTPUT/fw-A2 ESP32SIM_ROM_DIR=web/wasm/fw \
      CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=TARGET \
      nice -n 10 cargo +1.99.0 test --release -p esp32sim \
      --test classic_psram_arduino2 -- --include-ignored --nocapture --test-threads=1

Expected current result: exit 101, one absent-device PASS, one present-device FAIL.
The test adds symbols for observation only and asserts the stub list is empty,
stub hits zero and reboot count zero. It never preprograms cache/MMU registers.

## Remaining qualification, not performed

A separately scoped, independently reviewed low/high cache/translation contract
is required before P5 can proceed. Merely deleting the rejection or replacing
the stock SDK with unicore is not acceptable. Once unblocked, P5 still needs a
new fixture exercising ESP.getPsramSize, exact physical/available/registered heap
accounting, 1 MiB bytewise write/read, free/reallocate, no-device behavior and
semantic mutation kills. The reused P0 sketch does not cover free/reallocate or
the Arduino size getter. Those acceptance items and successful P5 mutations are
NOT claimed. No passing PSRAM CI firmware/golden is added here. No P6/P7, camera,
WASM PSRAM, hardware or production-recovery claim.

## No-regression checks

`checks.json` records actual commands and exits. All builds use Rust 1.99.0,
`nice -n 10`, four Cargo jobs and a dedicated target. ROM and plain workspace
runs use empty HOME, explicit toolchain homes, and respectively only the ROM
directory variable or no ESP32SIM variables.

- Native and WASM Clippy with warnings denied: PASS.
- ROM-enabled include-ignored suite excluding external_: 1001 PASS / 0 FAIL.
- Plain workspace suite: 971 PASS / 0 FAIL / 54 ignored (two new external probes).
- Production WASM build and all eight standard demos plus JIT handoff: PASS.
- Privacy and whitespace checks: PASS. All 55 existing golden blobs unchanged.
- Five retained A2 firmware artifact hashes match original P0 provenance.
- No JIT implementation changes; the additional JIT-specific script is inapplicable.
- No semantic emulator rules were added, and the successful-startup prerequisite
  fails. P5 semantic mutations remain NOT RUN, not zero survivors or a passing gate.

These checks do not override the failing stock-Arduino present-device probe.

## CPU comparison

PENDING. No timing run; no production/hot-path source changes.
