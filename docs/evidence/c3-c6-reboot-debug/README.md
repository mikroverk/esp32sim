# EX223: C3/C6 debug areas across guest reboot

Base: upstream/main `954f2a68`, which includes #198 and EX222 for S3.
The source is the commit containing this receipt. This extends the same reboot
contract to C3/C6. Rebuilding their digital peripherals now reapplies `self.debug`
through the same `Dispatch::debug` loop and MMIO flag assignment as `set_debug`.
The two Wi-Fi log copies are removed because that dispatch covers them.

Only reboot handling changes. There are no new fields, scheduler branches,
per-tick work, input channels, register meanings or firmware fixtures.
Directly setting a peripheral's log field is not persistent host configuration;
use `set_debug` to keep an area enabled across reboot.

## Verification

The per-chip `reboot_preserves_debug_areas` test requires no ROM or firmware.
It constructs a machine without reading environment settings, selects each of
quiet, `spi`, `wifi`, `mmio` and their combined selection, and checks SPI/Wi-Fi/MMIO log flags
and the stored debug areas before and after two successive reboots.
Both tests fail on the base at `SPI: area=spi, boot=1`.

Focused command:
`cargo +1.99.0 test --release -p esp32c3 -p esp32c6 --test reboot`.

Fetch public verification inputs with `tools/fetch-demo-assets.sh --no-linux`.
[Input SHA-256 hashes](inputs.json) identify the ROMs and demo assets.
[Required checks](checks.json) and [additional CI checks](extra-checks.json)
all pass, including both Clippy targets, the virtual scheduler suite, all eight
production WASM demos and evidence privacy. Full checks use Rust 1.99.0 on Darwin arm64 and Node v22.23.1. Workspace tests
run with an empty HOME, installed CARGO_HOME/RUSTUP_HOME, and all ESP32SIM_*/
ESP_EMU_* variables removed; only the CI-policy run sets ESP32SIM_ROM_DIR to
an absolute path to `web/wasm/fw`. CI-policy workspace tests: 644 passed, zero ignored. Plain workspace tests:
623 passed, 35 ignored. No goldens are regenerated. No JIT code changes.

## Mutation table

All four mutations compiled and failed the named test. Each was applied alone
and restored before the next run. Both restored tests pass.

| Mutation | Test that kills it |
| --- | --- |
| Remove C3 debug-area dispatch on reboot | C3 `reboot_preserves_debug_areas` |
| Remove C3 MMIO log reapply on reboot | C3 `reboot_preserves_debug_areas` |
| Remove C6 debug-area dispatch on reboot | C6 `reboot_preserves_debug_areas` |
| Remove C6 MMIO log reapply on reboot | C6 `reboot_preserves_debug_areas` |

## CPU comparison

PENDING

No CPU benchmark was run. The change executes only when rebuilding peripherals
on reboot; idle execution is structurally unchanged. No performance or hardware
measurement claim. No private captures or machine identifiers are retained.

Static code comparison: `cargo +1.99.0 build --release -p esp32sim --bin esp32sim`
(aarch64-apple-darwin, the workspace's fat-LTO release profile) at `954f2a68` and
at `eb1f2bfc`, disassembled per function with `objdump -d`, with absolute
addresses, branch targets and page offsets normalised. 10 of 2056 functions differ.
Six are the C3/C6 `Machine::reboot`, `SocBus::set_debug` and
`Peripherals::Dispatch::debug`, now out of line and shared by both callers.
The other four (C3 `Machine::peek` and `Bus::fetch`, one C6 `RawVec::grow_one`
instance and one S3 `Debug` impl) differ only in trailing alignment `nop`s. C6 `Machine::run`,
`step_core`, `Bus::tick`, `periph_write`, `refresh_irq` and
every other C6 function outside reboot and `set_debug` are instruction-identical.
Struct layouts are unchanged; only function addresses move.
