# EX228: classic ESP32 shared VHCI adapter

Base: `esp32-classic-6-analog`, `421476ba`. Builds on EX227 and the shared
HCI controller/VHCI lifecycle merged in #172 (EX200). Adds classic LX6
windowed-ABI calling glue, DRAM bounds, ELF-sized installation and function
hooks. S3's existing trampoline bytes move into the shared VHCI module;
classic and S3 reference that single constant. No second controller exists.

BLE state is appended to SocBus. Disabled BLE uses the existing empty function
hook list and adds no tick work. The controller remains the existing boxed
HciController implementation. Reboot restores substituted flash by physical
offset, even if the MMU changed, and resets controller/session state.

## Inputs and verification

The inherited fixture and ROM hashes are in [EX223](../classic-core/README.md).
Adapter tests construct instruction bytes and symbols locally, execute real
LX6 register windows, and check task arguments, receive/send-ready callbacks,
yielding, DROM callback tables, DRAM boundaries, packet limits, ELF extents,
function-boundary dispatch and physical flash restoration.

Moved trampoline: 150 bytes, SHA-256 `a1503a0de37163ce1ec858c6c32f40f9f183b5ccae13ffea97de7286a0815db4`.
The byte sequence is unchanged from the parent branch's S3 adapter.

Run the full EX223 Rust 1.99.0 check set on this branch, including both Clippy
targets, empty-HOME workspace runs, all eight WASM scenarios and evidence
privacy. Existing goldens are unchanged. JIT implementation is unchanged.

[Mutation table](mutations.json) records nine killed replacements. For each,
apply the source replacement and run `cargo +1.99.0 test -p CRATE --lib TEST`,
then restore it. Callback entry checks, instruction/cycle accounting, RAM
bounds, hook registration, ELF size and reset restoration are exercised.

Results: both Clippy targets pass with warnings denied; 781 CI-mode
workspace tests and 759 plain tests (36 ignored) pass with empty HOME.
All eight WASM scenarios and evidence privacy pass; goldens unchanged.

## CPU comparison

Not measured separately. The stack tip, PR #212 (`bcd4df7e`, which contains this change), was measured against main `2f9443a9` with the method in the classic-host receipt (user CPU, one warmup and seven alternating load-gated pairs, identical instruction counts and console hashes):

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C6 hello | 3.2784 (3.0442–3.3351) | 3.2612 (3.0412–3.3331) | -0.52% | 2/7 | 4800000000 | 5.27 |
| Pocket Tank | 35.2968 (34.6376–38.3461) | 35.0204 (34.7165–38.7101) | -0.78% | 3/7 | 10073833665 | 4.26 |
| S3 hello | 24.6510 (23.7365–32.0593) | 24.3939 (23.8923–27.6721) | -1.04% | 4/7 | 1789819657 | 4.96 |
| C3 hello | 2.3837 (2.3635–2.5350) | 2.4178 (2.3742–2.5332) | +1.43% | 5/7 | 4800000000 | 3.53 |

Static check (no timing): release `esp32sim` binaries from main `2f9443a9` and
this branch were disassembled with `llvm-objdump -d` and compared function by
function after normalising addresses, alignment `nop`s, adrp page offsets and
linker-chosen symbol aliases. The S3/C3/C6 execution path is instruction-identical
to main: `Machine::run`, `step_core`, each `SocBus` `tick` and `next_deadline`, S3 `tick_impl`,
`refresh_tick_budget` and GDMA engines, the MMIO dispatchers, `TimerGroup::tick`, and the UART,
TIMG, SHA, LEDC and RMT tick and register paths. Remaining differences: shared `Gpio::write` (`ubfx` for `lsr`, inherited from EX223); `I2c::write`, whose bounds-check panic calls share one source location in the macro
expansion, so three merge and the END arm moves; `Ledc::output`, the host PWM query, and the `Peripherals::new` constructors; drop glue for `StationLink` and `Tcp`; the analog I2C masters keep main's code
because `Regi2c` is inlined into each chip; `RtcCntl::write` places the SENS one-shot START-low block differently (five fewer
instructions; the arm runs only on ADC one-shot writes); `apply_due_script_events` and `ScriptAction` clone, format and drop gain the `TouchPad`
arm; script dispatch runs only when a scripted event is due; `Machine::reboot`, which restores substituted flash through the shared helper.

## Limits and overlap

The tests establish the adapter contract, not an unchanged classic Bluedroid
firmware boot or RF behavior. The shared controller's one-link, MTU-23 and
no-pairing limits remain. No private firmware or hardware capture is retained.
Open #196 changes C3 controller-driven BLE; classic uses the already merged
VHCI/controller path and does not duplicate that implementation.

All four chips now share flash restoration during reboot. VHCI chips use
`Ble::restore_flash`; C6 has its own NimBLE type and calls the same helper.
This changes no
per-tick path. Classic retains the physical-offset restoration regression;
shared packet-cap and trampoline-extent tests remain in S3 instead of being
copied into classic. The extent mutation runs against that S3 test.
