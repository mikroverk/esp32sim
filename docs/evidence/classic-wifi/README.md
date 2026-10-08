# EX226: classic Wi-Fi on StationLink

Base: `esp32-classic-4-crypto`, `5c05d939`. Builds on EX225 and the shared
StationLink from #193. The classic adapter retains MAC/PHY registers, TSF,
DPORT gates, DMA and RX metadata. AP/network queues, pacing, Ethernet relay,
statistics and reboot handling use StationLink. Descriptor decoding reuses
the checked reader from EX224. No fourth station implementation is added.

## Sources and limits

ESP-IDF v5.5.4 `components/soc/esp32/register/soc/dport_reg.h:1031-1043`
defines the clock reset value and Wi-Fi enable mask. MAC offsets, PHY
completion and RX metadata are inferred from the Arduino-ESP32 3.3.8 libraries;
no public register-header or hardware validation is claimed for these fields.
PHY calibration completes without RF arithmetic. No RF timing claim, PSRAM
DMA, multi-descriptor Wi-Fi frames or classic firmware networking validation
is claimed by these register-level tests.

An inline DPORT gate guards Wi-Fi service; there is no separate Cargo feature.
AHB aliases normalize before the shared MMIO path, including UART/I2C FIFO
notifications and RNG reads. TX borrows DRAM without copying the frame.
The shared Regi2c helper retains each chip's analog defaults and host selection
and is inlined, so each chip keeps its own map code;
classic adds only its inferred poll-completion fields. Existing chips gain no
fields or per-tick work. New classic fields are appended.

## Verification

Use the full [EX223 command set](../classic-core/README.md) on Rust 1.99.0.
The inherited ROM/firmware hashes are in that receipt. No golden changes.
JIT implementation is unchanged.

[Mutation table](mutations.json) records 26 killed single replacements and the tests
that fail. Run each with `cargo +1.99.0 test -p CRATE --lib TEST`, restoring
the original source between mutations. Tests cover register aliases, TSF,
clock/reset, DMA rejection, RX metadata, shared station pacing and reboot.

Results on Rust 1.99.0: both required Clippy checks pass; 763 CI-mode
workspace tests and 741 plain tests (36 ignored) pass with empty HOME.
All eight WASM scenarios pass. Goldens remain byte-identical.

## CPU comparison

PENDING

Static check (no timing): release `esp32sim` binaries from main `2f9443a9` and
this branch were disassembled with `llvm-objdump -d` and compared function by
function after normalising addresses, alignment `nop`s, adrp page offsets and
linker-chosen symbol aliases. The S3/C3/C6 execution path is instruction-identical
to main: `Machine::run`, `step_core`, each `SocBus` `tick` and `next_deadline`, S3 `tick_impl`,
`refresh_tick_budget` and GDMA engines, the MMIO dispatchers, `TimerGroup::tick`, and the UART,
TIMG, SHA, LEDC and RMT tick and register paths, and RTC_CNTL. Remaining differences: shared `Gpio::write` (`ubfx` for `lsr`, inherited from EX223); `I2c::write`, whose bounds-check panic calls share one source location in the macro
expansion, so three merge and the END arm moves; `Ledc::output`, the host PWM query, and the `Peripherals::new` constructors; drop glue for `StationLink` and `Tcp`; the analog I2C masters keep main's code
because `Regi2c` is inlined into each chip.

## Privacy

No firmware disassembly, private captures or host information is retained.
Register interpretations remain explicitly inferred where no header exists.
