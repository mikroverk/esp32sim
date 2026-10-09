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

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 954f2a68 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured on `eb1f2bfcf1c9325512751446d0d1b1f89e826b8a` against main `954f2a68`; the branch was later rebased onto `2f9443a9` with no change to its own diff. User CPU seconds.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C3 hello | 2.5581 (2.5188–2.6286) | 2.5323 (2.5135–2.6085) | -1.01% | 2/7 | 4800000000 | 3.62 |
| C6 hello | 3.2995 (3.2788–3.3382) | 3.3333 (3.3078–3.3440) | +1.03% | 6/7 | 4800000000 | 4.46 |

### C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.571468 | 2.575992 | 3.60/3.47/3.60 | 3.60/3.60/3.60 | accepted |
| 1 | 2 | M→B | 2.558150 | 2.527604 | 3.47/3.43/3.47 | 3.43/3.43/3.43 | accepted |
| 2 | 3 | B→M | 2.554602 | 2.532189 | 3.40/3.40/3.40 | 3.43/3.40/3.43 | accepted |
| 3 | 4 | M→B | 2.518787 | 2.592218 | 3.40/3.45/3.45 | 3.45/3.45/3.45 | accepted |
| 4 | 5 | B→M | 2.564130 | 2.550232 | 3.41/3.41/3.41 | 3.45/3.41/3.45 | accepted |
| 5 | 6 | M→B | 2.628572 | 2.513475 | 3.41/3.62/3.62 | 3.62/3.62/3.62 | accepted |
| 6 | 7 | B→M | 2.564391 | 2.608539 | 3.57/3.57/3.57 | 3.62/3.57/3.62 | accepted |
| 7 | 8 | M→B | 2.541359 | 2.532267 | 3.57/3.60/3.60 | 3.60/3.60/3.60 | accepted |

### C6 hello

Status: PASS. Flags: slower ≥6/7.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.345667 | 3.383490 | 3.55/3.51/3.55 | 3.60/3.55/3.60 | accepted |
| 1 | 2 | M→B | 3.297015 | 3.341907 | 3.51/3.51/3.51 | 3.51/3.55/3.55 | accepted |
| 2 | 3 | B→M | 3.318104 | 3.339821 | 3.74/3.74/3.74 | 3.55/3.74/3.74 | accepted |
| 3 | 4 | M→B | 3.338156 | 3.327408 | 3.74/3.76/3.76 | 3.76/4.02/4.02 | accepted |
| 4 | 5 | B→M | 3.278846 | 3.318484 | 4.02/4.34/4.34 | 4.02/4.02/4.02 | accepted |
| 5 | 6 | M→B | 3.322660 | 3.333324 | 4.34/4.15/4.34 | 4.15/4.15/4.15 | accepted |
| 6 | 7 | B→M | 3.299489 | 3.307779 | 4.06/3.98/4.06 | 4.15/4.06/4.15 | accepted |
| 7 | 8 | M→B | 3.293798 | 3.343998 | 3.98/3.98/3.98 | 3.98/4.46/4.46 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above. Raw output files are preserved.

 The change executes only when rebuilding peripherals
on reboot; idle execution is structurally unchanged. No hardware
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
Struct layouts are unchanged; only function addresses move. The measured C6 hello
difference above therefore comes from code placement, not added work.

