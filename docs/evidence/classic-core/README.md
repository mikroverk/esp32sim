# EX223: classic ESP32 core

Base: upstream `2f9443a9`. The classic chip uses the shared hooks merged in
#183, including the LX6-only UR 234-236 gate. No shared CPU or bus trait changes. Timer stepping and
alarm-gap logic are shared through `#[inline(always)]` `Timer` helpers; S3/C3/C6 keep 54-bit
masks and main's unsaturated deadline product, classic uses 64-bit masks and a saturating
deadline. Shared structs keep their main layout. Native and WASM front ends select the new
chip outside execution loops; the WASM chip enum appends the classic variant last.

The chip models the ECO3 memory map, PRO flash MMU, 256-byte decode-cache
invalidation, DPORT routing, UART, GPIO/IO_MUX, eFuse, flash and boot SHA-256,
RTC control and TIMG including LACT. TIMG edge sources are 58 and 62.
The APP MMU table is stored but execution currently uses the PRO mapping.
The fixed clock model is 240 MHz CPU, 80 MHz APB and 150 kHz RTC slow.
No PSRAM, DFP arithmetic or hardware timing validation is claimed. LACT sleep
stepping and timer-group watchdog execution remain outside the model.
Board callbacks and matrix peripheral signals belong to the peripherals part;
the core has no board callbacks in its tick or MMIO paths. A classic UART
wrapper adds the receive-pointer register; the shared UART is unchanged.
Direct app boot is rejected.

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
CI-mode workspace: 707 passed, zero failed. Plain workspace with no firmware
variables: 685 passed, 36 ignored, zero failed. All eight production WASM
scenarios pass. Evidence privacy check passes. Existing goldens are unchanged.
The fixture reports silicon revision v2.0 despite using the ECO3 ROM; the
model does not supply the additional revision-3 date bit. Its boot output
also retains inferred clock-calibration warnings. These are recorded limits,
not hardware equivalence claims.

## Mutation checks

[Mutation table](mutations.json): each row names the changed expression and
the test that fails. All 17 mutations are killed by assertions, not compile
errors. Reproduce each with `cargo +1.99.0 test --release -p esp32 --lib TEST`
after applying the named one-line replacement, then restore the expression.
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

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 954f2a68 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured on `274516d43c8a1505a39c8ed1db49d82aee648f24` against main `954f2a68`. The branch was since rebased onto `fe3a9c08` and gained one commit that returns shared `RtcCntl::write` to main's code; the static check below covers the current head. The whole stack is measured again at #212. User CPU seconds.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C6 hello | 3.2583 (3.2237–3.3002) | 3.2710 (3.2610–3.2895) | +0.39% | 4/7 | 4800000000 | 4.14 |
| Pocket Tank | 35.6747 (35.6081–38.3170) | 35.7083 (35.6130–35.9298) | +0.09% | 3/7 | 10073833775 | 3.03 |
| S3 hello | 25.4214 (25.0221–28.6998) | 25.3276 (24.9841–26.3929) | -0.37% | 3/7 | 1789819657 | 4.10 |
| C3 hello | 2.4209 (2.4161–2.4617) | 2.4404 (2.4134–2.4785) | +0.81% | 4/7 | 4800000000 | 3.11 |

### C6 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.315423 | 3.386911 | 4.05/4.05/4.05 | 4.14/4.05/4.14 | accepted |
| 1 | 2 | M→B | 3.300188 | 3.284723 | 4.05/3.81/4.05 | 3.81/3.58/3.81 | accepted |
| 2 | 3 | B→M | 3.243717 | 3.270954 | 3.58/3.53/3.58 | 3.58/3.58/3.58 | accepted |
| 3 | 4 | M→B | 3.253785 | 3.260999 | 3.53/3.33/3.53 | 3.33/3.33/3.33 | accepted |
| 4 | 5 | B→M | 3.288584 | 3.261825 | 3.14/2.97/3.14 | 3.33/3.14/3.33 | accepted |
| 5 | 6 | M→B | 3.258280 | 3.284469 | 2.97/2.97/2.97 | 2.97/3.05/3.05 | accepted |
| 6 | 7 | B→M | 3.296547 | 3.265093 | 3.13/3.13/3.13 | 3.05/3.13/3.13 | accepted |
| 7 | 8 | M→B | 3.223713 | 3.289536 | 3.13/3.12/3.13 | 3.12/3.03/3.12 | accepted |

### Pocket Tank

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 35.732971 | 35.459644 | 2.50/2.74/2.74 | 3.03/2.50/3.03 | accepted |
| 1 | 2 | M→B | 35.674723 | 35.708295 | 2.74/2.08/2.74 | 2.08/1.91/2.08 | accepted |
| 2 | 3 | B→M | 35.608068 | 35.929791 | 1.51/1.75/1.82 | 1.91/1.51/1.91 | accepted |
| 3 | 4 | M→B | 35.879542 | 35.619119 | 1.75/2.48/2.56 | 2.48/1.96/2.48 | accepted |
| 4 | 5 | B→M | 35.633072 | 35.683283 | 1.58/1.77/1.77 | 1.96/1.58/1.96 | accepted |
| 5 | 6 | M→B | 38.317014 | 35.864055 | 1.77/1.94/2.12 | 1.94/1.91/1.95 | accepted |
| 6 | 7 | B→M | 35.648992 | 35.613042 | 1.58/1.46/1.58 | 1.91/1.58/1.91 | accepted |
| 7 | 8 | M→B | 35.932761 | 35.879612 | 1.46/1.46/1.50 | 1.46/1.40/1.56 | accepted |

### S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 25.249511 | 25.158672 | 1.40/1.46/1.50 | 1.40/1.40/1.47 | accepted |
| 1 | 2 | M→B | 25.038804 | 25.533096 | 1.46/1.42/1.55 | 1.42/1.63/1.68 | accepted |
| 2 | 3 | B→M | 26.068775 | 26.392894 | 2.45/3.14/3.28 | 1.63/2.45/2.45 | accepted |
| 3 | 4 | M→B | 25.514731 | 25.327616 | 3.14/2.56/3.14 | 2.56/2.16/2.56 | accepted |
| 4 | 5 | B→M | 25.022109 | 25.116522 | 1.76/1.95/2.12 | 2.16/1.76/2.16 | accepted |
| 5 | 6 | M→B | 25.399362 | 25.116221 | 1.95/2.37/2.53 | 2.37/2.49/2.49 | accepted |
| 6 | 7 | B→M | 28.699832 | 25.734873 | 4.10/3.56/4.10 | 2.49/4.10/4.10 | accepted |
| 7 | 8 | M→B | 25.421370 | 24.984144 | 3.56/3.57/3.57 | 3.57/3.11/3.57 | accepted |

### C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.462578 | 2.526983 | 3.02/3.02/3.02 | 3.11/3.02/3.11 | accepted |
| 1 | 2 | M→B | 2.461655 | 2.429068 | 3.02/2.86/3.02 | 2.86/2.86/2.86 | accepted |
| 2 | 3 | B→M | 2.416063 | 2.455406 | 2.71/2.71/2.71 | 2.86/2.71/2.86 | accepted |
| 3 | 4 | M→B | 2.419689 | 2.457303 | 2.71/2.57/2.71 | 2.57/2.57/2.57 | accepted |
| 4 | 5 | B→M | 2.419744 | 2.478514 | 2.53/2.53/2.53 | 2.57/2.53/2.57 | accepted |
| 5 | 6 | M→B | 2.420874 | 2.413361 | 2.53/2.56/2.56 | 2.56/2.56/2.56 | accepted |
| 6 | 7 | B→M | 2.440300 | 2.440420 | 2.52/2.52/2.52 | 2.56/2.52/2.56 | accepted |
| 7 | 8 | M→B | 2.442235 | 2.434022 | 2.52/2.48/2.52 | 2.48/2.48/2.48 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above. Raw output files are preserved.

Static check (no timing): release `esp32sim` binaries from main `2f9443a9`
and this branch were disassembled with `llvm-objdump -d` and compared function
by function after normalising addresses, alignment `nop`s, adrp page offsets
and linker-chosen symbol aliases. The S3/C3/C6 execution path is
instruction-identical to main, including `Machine::run`, `step_core`, each
`SocBus` `tick` and `next_deadline`, `TimerGroup::tick` and the UART, TIMG,
RTC_CNTL and SHA register paths. The classic RTC adapter masks its translated
offset to the 4 KiB block so the shared `RtcCntl::write` keeps main's code.
Shared `Gpio::write`, which the classic GPIO adapter also calls, uses `ubfx`
instead of `lsr` for one register index, with the same instruction count.
Other differences are in CLI setup and reporting, `Cpu::dump`, a coverage
report and the opt-in register-trace observer.

## Privacy

No private captures are retained. Firmware reproducible-build settings map
source paths to generic labels. Input records contain hashes and public tool
versions; link-map paths are omitted. License notices retain upstream legal
attribution. No speed conclusion is drawn from these correctness runs.
