# EX225: classic ESP32 crypto adapters

Base: `esp32-classic-3-peripherals`, `e9b177c8`. Builds on EX223/EX224.
Adds classic AES and RSA registers and expands the boot SHA-256 adapter to
SHA-1/256/384/512, sharing AES, SHA and big-integer arithmetic with other chips.
The classic RSA Montgomery operation uses the programmed M-prime value;
plain multiply and modular exponentiation use the shared Rsa register operations.
The adapter translates operand widths and retains only classic commands and
Montgomery reduction. SHA counts come from the three shared engines, with
SHA-384/512 sharing a counter as well as state. DPORT owns command gating;
reset replaces each device with its initial state, including diagnostic counts.
DPORT clocks, coupled resets, RSA power-down and source 51 route completion.

## Sources and limits

ESP-IDF v5.5.4 `components/soc/esp32/include/soc/hwcrypto_reg.h:12-64`
defines operand memories and commands. In `register/soc/dport_reg.h`,
lines 98-104 define clock/reset bits, 1632-1638 and 2184-2190 define PRO/APP
RSA interrupt routing. Completion is synchronous and inferred. No timing,
hardware or IDF 4.4 validation is claimed. No new fields precede existing
peripheral fields; no crypto processing is added to the tick path.

The inherited IDF 5.5.4 fixture, ROM sources and input hashes are recorded in
[EX223](../classic-core/README.md). Tests use FIPS-197 AES vectors, SHA vectors
and independent integer arithmetic, including carry across every supported
Montgomery width. SHA-384/512 share state; SHA-1/256 remain independent.

## Verification

Run the full [EX223 command set](../classic-core/README.md) with Rust 1.99.0:
both Clippy targets with warnings denied, empty-HOME CI-mode and plain release
workspace tests, production WASM build and all eight required scenarios,
and evidence privacy. Existing goldens are not regenerated. JIT code is unchanged.

[Mutation table](mutations.json): 20 single replacements killed by assertion
failures. For each row, apply the replacement, run
`cargo +1.99.0 test -p esp32 --lib TEST`, then restore the source.

Results: both Clippy checks pass; 754 CI-mode tests and 732 plain tests
(36 ignored) pass with empty HOME. All eight WASM scenarios pass.
Existing goldens remain byte-identical. Evidence privacy passes.

## CPU comparison

PENDING

Static check (no timing): release `esp32sim` binaries from main `2f9443a9` and
this branch were disassembled with `llvm-objdump -d` and compared function by
function after normalising addresses, alignment `nop`s, adrp page offsets and
linker-chosen symbol aliases. The S3/C3/C6 execution path is instruction-identical
to main: `Machine::run`, `step_core`, each `SocBus` `tick` and `next_deadline`, S3 `tick_impl`,
`refresh_tick_budget` and GDMA engines, the MMIO dispatchers, `TimerGroup::tick`, and the UART,
TIMG, SHA, LEDC and RMT tick and register paths, and RTC_CNTL. Remaining differences: shared `Gpio::write` (`ubfx` for `lsr`, inherited from EX223); `I2c::write`, whose bounds-check panic calls share one source location in the macro
expansion, so three merge and the END arm moves; `Ledc::output`, the host PWM query, and the `Peripherals::new` constructors.

## Privacy

Only public source expressions, test names and outcomes are retained.
No private captures or machine identifiers are included.
