# EX223: classic ESP32 core

Base: upstream `017af524`. The classic chip uses the shared hooks merged in
#183, including the LX6-only UR 234-236 gate. No shared CPU, bus trait or
existing chip tick implementation changes. Native and WASM front ends select
the new chip outside execution loops.

The chip models the ECO3 memory map, PRO flash MMU, 256-byte decode-cache
invalidation, DPORT routing, UART, GPIO/IO_MUX, eFuse, flash and boot SHA-256,
RTC control and TIMG including LACT. TIMG edge sources are 58 and 62.
The APP MMU table is stored but execution currently uses the PRO mapping.
The fixed clock model is 240 MHz CPU, 80 MHz APB and 150 kHz RTC slow.
No PSRAM, DFP arithmetic or hardware timing validation is claimed. T0/T1 use
the shared 54-bit counter; classic hardware has 64-bit counters. LACT sleep
stepping and per-core watchdog resets remain outside the model.

## Inputs and reproduction

[Input hashes](inputs.json) identify the committed firmware. The
[recipe](../../../examples/hello_world-classic/README.md) pins pioarduino
55.03.38-1, ESP-IDF 5.5.4 and GCC 14.2.0_20260121. Two clean builds in different
source/output directories produced identical bootloader, partition and app
bytes. The ROM fetcher pins esp-rom-elfs 20260528 and the ECO3 ELF hash.

```sh
tools/fetch-demo-assets.sh --no-linux
cargo +1.99.0 clippy --workspace --all-targets -- -D warnings
cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
cargo +1.99.0 test --release --workspace
RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh
node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel
node tools/check-evidence-privacy.mjs
```

The workspace test runs use an empty HOME, with RUSTUP_HOME pointing to the
installed toolchain. ESP32SIM_ROM_DIR is the only firmware input environment
variable for the ignored-test run; the plain run has no firmware variables.
The classic golden pins console, exceptions and interrupt totals/per-source
counts. It does not pin cycle-derived instruction counts. Existing goldens
remain unchanged. JIT implementation files are unchanged.

## Results

Rust 1.99.0: native and WASM Clippy pass with warnings denied. Empty-HOME
CI-mode workspace: 664 passed, zero failed. Plain workspace with no firmware
variables: 642 passed, 36 ignored, zero failed. All eight production WASM
scenarios pass. Evidence privacy check passes. Existing goldens are unchanged.
The fixture reports silicon revision v2.0 despite using the ECO3 ROM; the
model does not supply the additional revision-3 date bit. Its boot output
also retains inferred clock-calibration warnings. These are recorded limits,
not hardware equivalence claims.

## Mutation checks

[Mutation table](mutations.json): each row names the changed expression and
the test that fails. All 13 mutations are killed by assertions, not compile
errors. Reproduce each with `cargo +1.99.0 test -p esp32 --lib TEST` after
applying the named one-line replacement, then restore the expression.
The GPIO test drives an undriven pad low with a pull-down before enabling
its pull-up; starting high alone cannot detect missing pull handling.

## Register sources

The fixture uses IDF 5.5.4. Source comments cite its
`components/soc/esp32/register/soc/*_reg.h` definitions and
`components/soc/esp32/include/soc/{soc,interrupts,gpio_sig_map}.h`.
The interrupt enum's lines 75 and 79 give edge sources 58 and 62.
`timer_group_reg.h` lines 42-53 give T0 edge/level bits 12/11;
397-438 give LACT enable/direction/reload/divider/alarm bits.
`io_mux_reg.h` lines 41-66 give pulls, input enable and function selection.
Completion timing and ROM compatibility behavior are inferred, not measured
on hardware. This fixture does not establish an IDF 4.4 firmware contract.

## CPU comparison

PENDING

## Privacy

No private captures are retained. Firmware reproducible-build settings map
source paths to generic labels. Input records contain hashes and public tool
versions; link-map paths are omitted. License notices retain upstream legal
attribution. No speed conclusion is drawn from these correctness runs.
