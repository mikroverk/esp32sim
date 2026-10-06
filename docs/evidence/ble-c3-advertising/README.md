# EX211: C3 controller-driven legacy advertising

Base: `a3a3102c6c7e130e20a4d5d6e4eb45a68251590c`.
Controller: `6b136e7a`. Firmware and tests: `fced1b82`.
Scope agreed in [mikroverk/esp32sim#189](https://github.com/mikroverk/esp32sim/issues/189).

`--ble full` runs the C3 guest controller through its registers and exchange
memory, without HCI substitution, ELF hooks or diagnostic pokes. The passive
observer reports emitted PDUs. The guest handles END interrupts and schedules
subsequent events, including wrap of the 16-entry event table. EX200 tests an HCI
replacement; EX211 instead tests the controller's MMIO, timer and descriptor
contract. EX210's shared StationLink remains unchanged.

## Inputs and reproducible firmware

[inputs.json](inputs.json) pins firmware, ROM and source hashes, compiler and build
tools. The committed NimBLE advertiser uses ESP-IDF v5.5.5 and its pinned submodules,
RISC-V GCC esp-14.2.0_20260121, reproducible-build configuration and app version `1`.
Two builds in different source and output directories produce identical app ELF,
app binary, bootloader and partition table bytes. The
[build recipe](../../../examples/c3-ble-advertiser/README.md) and
[binary licence notice](../../../web/wasm/fw/public/c3-ble-NOTICE.txt) are committed.
Only the three binaries are needed by CI; no application ELF is loaded by the test.

Native and production WASM each emit 19 events / 57 ADV_SCAN_IND PDUs in two
modeled seconds. Every event uses channels 37, 38 and 39 and advertises name
`esp32sim`, Battery Service UUID `180f` and flags `06`. The native golden pins every
observer line and 320,000,000 accounted instructions. Guest idle cycles are included
in that count. Native default AdvA is `60:55:f9:00:11:24`; WASM's default is
`3c:84:27:b6:a7:1e`. These are simulator defaults. The different addresses affect
the controller's random delay, so the two hosts do not share a timestamp golden.
Both use a 100 ms configured interval plus the guest's 0–10 ms advertising delay.
[wasm.json](wasm.json) records its interval range and production module hash.

## Inferred register and descriptor contract

The register layout is inferred from the C3 rev3 ROM and ESP-IDF v5.5.5
controller. The hardware comparison below identifies the readbacks and clock
rate now checked on C3 rev v0.3; other semantics remain inferred.
The code marks these inferences in `esp32c3/src/ble_lc.rs`. LC means base
`0x60031000`, also inferred from guest loads/stores. ROM function addresses refer
to the ROM hash in `inputs.json`; application functions can be located with `nm`
in the reproducible ELF whose hash is recorded there.

| Inferred field | Meaning used | Reader/writer |
| --- | --- | --- |
| LC `+0x00`, bit 31 | Reset/disable request, self-clear after 0.5 modeled µs; cancel events, alarm and pending IRQs | `r_rwip_driver_init`, `r_rwble_hw_disable` |
| LC `+0x04` | Required identity `0x09001b00`; individual subfields unknown | `r_lld_core_init` |
| LC `+0x1c/+0x20` | Bit-31 latch; 28-bit half-slot counter and fine downcount `624..0` | `r_rwip_time_get`, ROM `0x4002ee5c..0x4002eeb2` |
| LC `+0x0c/+0x10/+0x18` | Mask, masked status, W1C acknowledgement; acknowledgement reads zero | `r_rwble_isr`, ROM load `0x4002e8ee`; `r_rwble_isr_hack` |
| LC `+0xec/+0xf0`, raw bit 11 | Coarse/fine alarm pair; fine write arms timer IRQ | `r_rwip_timer_hus_set`, ROM stores `0x4002f312/0x4002f34c` |
| LC `+0x2d8` | Count `[9:5]`, source snapshot `[30:10]`, pop bit 0, reset bit 31, stored configuration `[4:1]` | `r_rwble_isr_hack` |
| LC `+0x204..0x2c0`, `+0x2e0..0x2fc` | 56 mappings with a seven-register gap; high 14 bits × 4 = logical start; low 18 bits × 4 OR `0x3fc00000` = SRAM base | `r_emi_get_mem_addr_by_offset`, ROM `0x400069e2..0x40006a90` |
| LC `+0x100` | Bit 31 kicks event index in low nibble | `r_sch_prog_ble_push_hack` |
| Event stride 16; `+2/+4`, `+6` | 28-bit half-slot time and `624 - fine_half_us` | `r_sch_prog_push`, ROM stores `0x40030f1a/0x40030f3c/0x40030f9e` |
| Event `+8` | CS pointer in halfwords | `r_sch_prog_ble_push_hack` |
| CS stride 90; `+0 & 31` | Legacy advertising format 4 | `r_lld_adv_start_set_cs`, ROM `0x400181e4..0x400181e8` |
| CS `+6..11`, `+28`, `+38[7:5]` | AdvA, first TX descriptor logical offset, channel map | `r_lld_adv_start`, ROM `0x40018684..0x40018942` |
| TX stride 14; `+0[14:0]` | Next descriptor logical offset | `r_lld_adv_start_init_evt_param`, ROM `0x40017b9e/0x40017c50` |
| TX `+2`, `+4` | LL header with length including AdvA; AD buffer logical offset | `r_lld_adv_adv_data_set`, ROM `0x40015858/0x40015864` |
| Event `+0[5:3]` | 3 = complete, 4 = aborted callback | `r_sch_prog_end_isr_handler` |
| Raw/FIFO bit 5 | END after last channel's airtime/window | `r_rwble_isr_hack`, indirect callback through `r_ip_funcs_p+0x6c0` |

The public interrupt-source definition is ESP-IDF **v5.5.5**
`components/soc/esp32c3/include/soc/interrupts.h`, `ETS_RWBLE_INTR_SOURCE = 8`,
checked against the pinned firmware's IDF checkout. No IDF 4.4 compatibility claim
is made. Time uses the existing C3 160 MHz model clock, 80 cycles per half-µs.
The hardware follow-up below adds header-defined C3 reset values and access gates.

To inspect the inferred application contract after rebuilding, with the matching
compiler on PATH:

```sh
ELF=examples/c3-ble-advertiser/build/ble_advertiser.elf
riscv32-esp-elf-nm "$ELF"
riscv32-esp-elf-objdump -d --disassemble=r_rwble_isr_hack "$ELF"
riscv32-esp-elf-objdump -d --disassemble=r_sch_prog_end_isr_handler "$ELF"
riscv32-esp-elf-objdump -d --disassemble=r_sch_prog_ble_push_hack "$ELF"
```

## Limits

No over-air timing comparison. Reset/latch latency, clock epoch, modular late-alarm
handling, one active event, FIFO kick order, coalescing identical interrupt sources,
a maximum of 16 queued kicks and nine descriptors, CS/TX snapshot at event start,
ascending channel order and a fixed 300 µs silent receive window are model choices.
Packet airtime uses 1M PHY length; RF energy, whitening and CRC bytes are not modeled.
Hardware FIFO overflow, sleep, clock gating, overlapping events and Wi-Fi coexistence
are not validated. Completion writes only the event state and END interrupt.

Malformed descriptors report an error and abort the event. Unmapped event entries
report an error without writing SRAM. Unit tests cover mapping bounds, malformed
lengths, reset cancellation, channel maps, latch/wrap, mask/W1C/FIFO and source-8
routing. Configured SCAN_RSP descriptors may be reported separately as `[ble-config]`,
never as transmitted packets. The fixture's configured response has no AD data.
There is no RX, SCAN_REQ/SCAN_RSP exchange, connection or GATT support; milestone B
follows separately. The observation queue keeps the latest 1024 entries and reports
drops. CLI full mode is C3-only and mutually exclusive with HCI `--ble`.

## Verification and off-mode CPU comparison

[checks.json](checks.json) records final commands and outcomes. Rust is selected
explicitly with `+1.99.0` or `RUSTUP_TOOLCHAIN=1.99.0`; the default is unchanged.
Existing goldens were not regenerated. Only `ble-advertiser-c3.observer.txt` and
`ble-advertiser-c3.insns` were added to pin the new fixture's behavior.

[bench.py](bench.py) runs one warmup pair, then seven alternating main/candidate
pairs for each hello demo. Every run is 30 modeled seconds with BLE off. Both
binaries are built with `cargo +1.99.0 build --release --bins`. All instruction
counts and console hashes must match; [cpu.json](cpu.json) retains every measured
round, warmups, run order, binary/input hashes, aggregate load and median child user
CPU seconds. Samples include process startup. Benchmark runs were sequential, with no concurrent builds or tests from this
comparison; unrelated host load was uncontrolled. These short measurements do
not prove mathematically zero overhead or establish browser speed.

```sh
mkdir -p target/ble-main
git archive a3a3102c | tar -x -C target/ble-main
cargo +1.99.0 build --release --bins --manifest-path target/ble-main/Cargo.toml
cargo +1.99.0 build --release --bins
python3 docs/evidence/ble-c3-advertising/bench.py \
  target/ble-main/target/release target/release web/wasm/fw > target/ble-cpu.json
```

The optional controller has no tick/deadline callback when disabled. BLE service
is behind the existing cached `work_pending` path. Timing results below quantify
the disabled build rather than inferring performance from that code structure.

Measured on Apple M5 Pro, macOS 27.0.1. C3 median user time is
2.481291 → 2.483148 s, **+0.07%**, with overlapping ranges. S3 is +1.88%
(about 5 ms), C6 +0.13%. These differences are small relative to individual run
variation; there is no measurable C3 regression in this sample and no speedup claim.
Every C3/C6 run accounts for 4,800,000,000 instructions; every S3 run for 18,788,848.
Console SHA-256 values match across both arms for each demo.

User CPU seconds, all seven measured rounds. Extra decimal places retain samples,
not measurement precision; warmups and load are in `cpu.json`.

| Round | S3 main | S3 candidate | C3 main | C3 candidate | C6 main | C6 candidate |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 0.257393 | 0.259696 | 2.481291 | 2.454482 | 3.460977 | 3.293555 |
| 2 | 0.258248 | 0.263622 | 2.447119 | 2.474360 | 3.294882 | 3.362464 |
| 3 | 0.256003 | 0.262223 | 2.444945 | 2.523605 | 3.289143 | 3.428535 |
| 4 | 0.258957 | 0.264896 | 2.478731 | 2.466368 | 3.492156 | 3.438074 |
| 5 | 0.256494 | 0.262848 | 2.566234 | 2.589667 | 3.443366 | 3.437814 |
| 6 | 0.261825 | 0.258633 | 2.617709 | 2.605095 | 3.346058 | 3.340212 |
| 7 | 0.254952 | 0.257506 | 2.596742 | 2.483148 | 3.357982 | 3.295564 |
| Median | 0.257393 | 0.262223 | 2.481291 | 2.483148 | 3.357982 | 3.362464 |

## Evidence curation

Committed evidence contains commands, hashes, counts, timings and limits. Raw
build output, console logs, event streams and process inventories are not included.
The automated-test observer golden is a fixture, not an evidence capture. The
receipt was constructed from numeric results without personal paths or machine
identifiers; no existing receipt was redacted. The privacy pattern check and manual
review cover the new text and firmware binaries. No remote artifact is required to
reproduce the claims.


## C3 rev v0.3 hardware comparison

This EX211 repeat adds a silicon oracle to the original inferred model. The same
Arduino-ESP32 3.3.11 / ESP-IDF 5.5.5 probe binary ran on a C3 rev v0.3 board with
4 MB flash and a 40 MHz crystal, and on this advertising-only PR. No probe rebuild
was needed. The earlier emulator capture used milestones A+B; this repeat checks A
on parent `3783dedf` plus this commit's changes. The original capture's emulator
revision is deliberately not cited because it is not an ancestor of this PR.

The probe takes pre-init, initialized and advertising snapshots, brackets sixteen
latch requests with timer/cycle reads, and samples programmed deadlines for two
seconds. It does not measure radio transmissions. [hardware/receipt.json](hardware/receipt.json)
retains input/capture hashes, comparison outcomes, timings and check results.
[hardware/Probe.ino](hardware/Probe.ino) and [hardware/compare.py](hardware/compare.py)
retain the measurement and comparison methods.

| Register | Classification and correction | Scope of hardware check |
| --- | --- | --- |
| LC+04 | Read-only identity, gated by SYSCON clock/reset and RTC BT power/isolation | Zero before init, `09001b00` after init; pre-init actually has BT power-down and isolation set despite enabled clocks |
| LC+08 | Read-only feature word `0f22d0b0` | Init and advertising values; individual fields and read-only behavior remain inferred |
| LC+14 | Live unmasked interrupt status; half-slot bit0 and event-start bit4 | Init `1`, advertising `11`; bit meanings and transition timing remain inferred |
| LC+48 | Inferred reset configuration `0003fff7`, retained by guest RMW | Guest ORs `100` and `f0`; initialized value checked, power-on value not directly observed |
| LC+70 | Live RF status bit1 survives the guest's zero write | Readback `2`; other RF states remain unproven |
| LC+7c | Inferred reset configuration `e400e400` | No guest write in the trace; initialized value checked |
| LC+8c | Guest configuration, mask off upper half on write | Both guest writes traced, final readback `64`; other bit widths remain inferred |
| LC+f8 | Phase-calibration configuration and success status | ROM `r_cali_phase_match_p` searches fields `[10:8]` and `[6:4]`, testing bit12. Model succeeds at phase 2/2, producing checked readback `1221`; phase choice is a board-specific model assumption, not an analog simulation |
| LC+2cc | Live exchange-memory error diagnostic, left unmodeled | Hardware `00340034`, emulator `0` during advertising; discrepancy retained, not claimed fixed |
| SYSCON+14 | Header reset `fffce030`, ordinary guest RMW | All three snapshots now `ffffffdf` |
| SYSTEM+24 | Header reset `02001001`, ordinary guest RMW | Pre `01001001`, init/advertising `04001000` |

The clock/reset masks and reset defaults come from ESP-IDF v5.5.5
`components/soc/esp32c3/register/soc/syscon_reg.h`, `system_reg.h` and
`rtc_cntl_reg.h`, matching the probe firmware. These are C3-only initial values;
the shared S3 SYSTEM implementation is unchanged. No IDF 4.4 claim is made.

The original comparator incorrectly classified LC+2cc as static configuration.
C3 ROM `r_rwble_isr`, load `0x4002e7e4`, reads its low 14 bits when LC+60 bit21 is
set and prints `EM BASE ERROR`, string at `0x3ff1b4c4`. The revised comparator
classifies it as live diagnostic state while retaining both numeric values.
There is insufficient evidence to derive its update/clear behavior from one
advertising snapshot. No constant was added to reproduce that snapshot.
The original comparator therefore still reports this one exact mismatch; the
corrected comparator reports zero static mismatches. Other asynchronous differences
remain, including event/RX bytes and device-dependent addresses.

Latch completion and the rate of 2 half-µs ticks per µs are checked on C3 rev v0.3.
The programmed event-deadline medians are 65.625 ms hardware and 65.9375 ms emulator,
within independent 0..10 ms advertising delay. These results do not validate the
80-cycle latch latency, clock epoch, atomicity, reset behavior, W1C, FIFO ordering,
RF timing, RX, connections or coexistence. Firmware uses the emulated calibration
success instead of exhausting its search; the committed advertiser observer lines and instruction golden remain identical.

Raw serial/device dumps remain local and are not committed. The receipt omits
MAC/BT addresses, raw memory/event streams and local paths, preserving hashes of
the original captures and numeric measurement summaries. No device identifier is
included in the draft PR comment. Capture hashes identify local evidence; those
private captures are not downloadable, so independent replication requires a board.

Follow-up validation passes with Rust 1.99.0: both Clippy commands with warnings
denied; 613 native tests under CI's ignored-test policy; 594 plain tests with 33
ignored; all eight WASM demos; timing/BLE ABI, VQ and ancillary CI checks; comparator
self-check and evidence privacy check. No JIT change, so the conditional JIT suite
was not rerun. All goldens remain byte-identical, including BLE advertising.

[hardware/cpu.json](hardware/cpu.json) records one warmup and seven alternating
pairs per hello demo against upstream main `cbb9edf6`, with identical instruction
counts and console hashes. Median user CPU seconds main → candidate: C3
2.590978 → 2.577355, -0.526%; S3 0.277569 → 0.275099, -0.890%; C6
3.404235 → 3.426627, +0.658%. No concurrent builds/tests from this task; unrelated
host load uncontrolled. There is no measured off-mode C3 regression or speedup claim.
