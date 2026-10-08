# EX217: GPIO input feedback and C6 SPI routes

Base: upstream `017af524`. Implementation: `b58d96d884fd4e1c0dd629faa651f6874d6cbc68`. The branch ports GPIO behavior onto the existing
BoardModel API, typed IO_MUX and shared `PinRoutes` decoder. Open upstream PRs
were checked before editing; none were open. No new input channel, DMA walker,
register block or timing mode is introduced. The idle-path and shared-helper
revision follows `45e2211b6e149a2623936437cec86152a250f8eb`; `inputs.json` pins
the checked source files by SHA-256.

## Contract

GPIO input-register reads drain board edges due at the current cycle, even when
no tick elapsed after an output write. Boards retain future edges until due.
Board releases run after input edges and after attachment snapshots. C3 retains
its deadline-gated tick path; a board needing an autonomous release supplies a
deadline. Replacing a board requires `attach_board_devices`, as for UART inputs.

`Gpio::set_input` records an ideal host drive. `release_input` removes it.
Resolution prefers host drive, then an enabled GPIO output, then pulls.
Pull-up wins simultaneous pulls; an undriven pad without pulls reads high.
These are inferred digital-model choices, not electrical contention or floating
voltage claims. Open-drain and peripheral-driven pad resolution are not added. Output and pull changes use the same edge detector as host
inputs. Reboot retains host drive masks and levels, while resetting output,
pulls, interrupt status and pending input edges. A retained board may then
supply its current levels and releases.

Board input delivery and release reporting share the `esp_soc::gpio` helpers.
All chips gate delivery on `board_edges` before checking the read address;
S3 refreshes its budget only inside that gate. Input mutation returns whether
the resolved level changed, so repeated stable board levels leave IRQ caches
clean on all three chips. IO_MUX pull decoding shares `Gpio::set_pad`.

Output-driven changes enter the input queue only for pins selected by the
GPIO input matrix. Its pin mask updates on selector-register writes, outside
the tick path. Unrouted output loops leave the S3 cadence and PCNT queue idle.
Routed edges still reach PCNT, whose in-place drain retains queue capacity.
S3 output IRQ invalidation compares resolved input changes against a cached
interrupt-enabled pin mask, maintained on GPIO_PIN writes.

C6 SPI2 passes physical routes only to boards opting into `uses_spi_pins`.
The shared decoder accepts explicit CS signal IDs and native CS pin lists.
C3/S3 retain their previous routes. C6 supports all six active-low hardware
selects, low enabled software selects, mirrored outputs and native/matrix
MISO selection. Inverted routes, active-high CS and analog behavior remain
outside this digital SPI callback contract. Legacy callbacks are unchanged.

Related experiments are EX201/EX205/EX206 for pin transport and EX209 for
programmed GPIO state. The additional contract is input resolution and
same-cycle feedback; scheduler and CPU timing rules are unchanged.

## Register sources

Checked public ESP-IDF **v5.5.4**, matching the Arduino-ESP32 3.3.8 historical
sonar input. Paths below start at `components/soc/`. Header SHA-256 values are
in `inputs.json`. No silicon verification is claimed.

- `esp32s3/register/soc/gpio_reg.h`, lines 165–180: IN/IN1 offsets 0x3c/0x40;
  lines 278–301: INT_ENA at 13 and INT_TYPE at 7.
- `esp32c3/register/soc/gpio_reg.h`, lines 114–121 and 171–194: IN and interrupt
  fields. C3 has no physical upper bank; its pre-existing shared readback is
  unchanged.
- `esp32c6/register/soc/gpio_reg.h`, lines 194–219, 389–419, 4992–5023:
  IN/IN1, interrupt fields, output selector/inversion/enable and reset selector
  128. That reset value was already on main.
- Matrix input selection in `gpio_reg.h`: C3 lines 1323–1340 (enable bit 6,
  inversion bit 5, pin bits 4:0); C6 lines 2456–2476 and S3 lines 2670–2687
  (enable bit 7, inversion bit 6, pin bits 5:0).
- `esp32{c3,c6,s3}/register/soc/io_mux_reg.h`, lines 42–65: pull-down bit 7,
  pull-up bit 8, input-enable bit 9, function field at 12. Pad offsets are
  4 + 4 * GPIO number, limited to each chip's existing IO_MUX range.
- `esp32c6/include/soc/gpio_sig_map.h`, lines 115–126 and 197–209:
  CLK/Q/D 63/64/65, CS0 68, CS1–5 101–105.
- `esp32c6/register/soc/io_mux_reg.h`, lines 182–209 and 243–272:
  native function 2 on Q2/CLK6/D7/CS16–21.
- `esp32c6/register/soc/spi_reg.h`, lines 365–403 and 497–559:
  USER MOSI/MISO/address/command and MISC CS disable/polarity fields.

Also checked v4.4.8 `esp32{c3,s3}/include/soc/{gpio_reg,io_mux_reg}.h` for
Arduino-ESP32 2.x. The used field positions and input offsets agree; v5.5.4
moves these register headers to `register/soc/`. C6 is not an IDF 4.4 target.

## S3 HC-SR04 / pulseIn limitation

The retained historical negative observation is in `sonar-history.json`.
It is **not a firmware rerun on this branch**. At quantum 1 with instruction
timing, the 240 MHz S3 trigger lasted 2388 cycles, below the sensor model's
strict 2400-cycle minimum. No echo was scheduled, so `pulseIn` returned zero.
Disabling virtual quanta produced the same observation.

[Arduino 3.3.8 delayMicroseconds](https://github.com/espressif/arduino-esp32/blob/3.3.8/cores/esp32/esp32-hal-misc.c#L199-L212)
polls an integer-microsecond timer until it reaches the starting value plus
the requested delay. The retained trace crosses integer values 5706 to 5716
while only 2258 CPU cycles elapse between samples. With 130 cycles of GPIO/call
overhead, the trigger is 2388 cycles, or 9.95 microseconds. This explains the
missing echo without changing `pulseIn` or the timer frequency.
[The pulse reader](https://github.com/espressif/arduino-esp32/blob/3.3.8/cores/esp32/wiring_pulse.c)
uses cycle counts, but it cannot measure an echo that was never emitted.

The historical approximate-timing run gave a 2624-cycle trigger and 5800 us
pulse. Quantum 64 gave 2432 cycles and 5799 us. Neither is a default-timing fix.
No lowered sensor threshold, added GPIO delay, timing-mode default change or
new firmware golden is part of this PR. Default instruction timing at quantum
1 remains a sonar acceptance limitation. The raw trace is unavailable here;
its original hash and numeric summary are retained. The historical firmware
hashes identify the inputs but those binaries are not CI dependencies.

## Verification

Both Clippy checks pass. CI-policy workspace: 677 passed, none failed. Plain
workspace: 656 passed, 35 ignored, none failed. Both ran with an empty HOME.
All eight WASM demos, timing/BLE ABI, section policy, native virtual-quantum
checks and the twelve additional JS/Python CI checks pass. All 54 mutations
are killed; existing goldens are unchanged.

Reproduce the two workspace environments without changing the default toolchain:

```sh
mkdir -p target/ex217-home
TMPDIR="$PWD/target" tools/fetch-demo-assets.sh --no-linux
env -i PATH="$PATH" HOME="$PWD/target/ex217-home" CARGO_HOME="$HOME/.cargo" RUSTUP_HOME="$HOME/.rustup" ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
env -i PATH="$PATH" HOME="$PWD/target/ex217-home" CARGO_HOME="$HOME/.cargo" RUSTUP_HOME="$HOME/.rustup" cargo +1.99.0 test --release --workspace
```

CARGO_HOME and RUSTUP_HOME locate the installed toolchain and dependency cache;
no developer firmware or simulator setting is inherited. The home directory is
empty at the start of each run. The checks do not depend on its contents.

See `checks.json` for commands and results and `mutations.json` for each mutation
and the test that rejects it. GPIO and routing tests construct register/board
inputs in code and need no private firmware or home-directory state. Existing
goldens must remain byte-identical. No JIT code changes.

## Mutation results

Each row was run separately; every mutation failed a test, rather than compilation.
The script restores each source file after its run. Reproduce from the repository root:

```sh
python3 docs/evidence/gpio-input-feedback/mutate.py target/ex217-mutations.json
```

| Mutation | Test that rejects it |
| --- | --- |
| Do not remember host drive mask | `input_change_result_does_not_depend_on_irq_configuration`, `released_pad_resolves_output_pulls_and_interrupts` |
| Do not remember host low level | `input_change_result_does_not_depend_on_irq_configuration`, `released_pad_resolves_output_pulls_and_interrupts` |
| Keep released drive active | `input_change_result_does_not_depend_on_irq_configuration`, `released_pad_resolves_output_pulls_and_interrupts` |
| Ignore pull-down | `released_pad_resolves_output_pulls_and_interrupts` |
| Floating pad defaults low | `input_change_result_does_not_depend_on_irq_configuration`, `released_pad_resolves_output_pulls_and_interrupts` |
| Ignore enabled output | `released_pad_resolves_output_pulls_and_interrupts`, `output_edges_only_queue_for_selected_matrix_inputs` |
| Skip output pad resolution | `output_edges_only_queue_for_selected_matrix_inputs`, `released_pad_resolves_output_pulls_and_interrupts` |
| Do not latch GPIO edges | `released_pad_resolves_output_pulls_and_interrupts` |
| Do not guard invalid release pin | `released_pad_resolves_output_pulls_and_interrupts` |
| c3: skip same-cycle read delivery | `gpio_reads_deliver_same_cycle_feedback_and_preserve_future_edges` |
| c3: do not restore host drives | `reboot_preserves_host_drives_but_resets_pad_state` |
| c3: ignore host release | `released_inputs_resolve_pulls_outputs_and_irq` |
| c3: skip board release callbacks | `board_releases_on_attachment_and_deadline` |
| c3: ignore pull registers | `released_inputs_resolve_pulls_outputs_and_irq` |
| c6: skip same-cycle read delivery | `gpio_reads_deliver_same_cycle_feedback_and_preserve_future_edges` |
| c6: do not restore host drives | `reboot_preserves_host_drives_but_resets_pad_state` |
| c6: ignore host release | `released_inputs_resolve_pulls_outputs_and_irq` |
| c6: skip board release callbacks | `board_releases_on_attachment_and_deadline` |
| c6: ignore pull registers | `released_inputs_resolve_pulls_outputs_and_irq` |
| s3: skip same-cycle read delivery | `gpio_reads_deliver_same_cycle_feedback_and_preserve_future_edges` |
| s3: do not restore host drives | `reboot_preserves_host_drives_but_resets_pad_state` |
| s3: ignore host release | `released_inputs_resolve_pulls_outputs_and_irq` |
| s3: skip board release callbacks | `board_releases_on_attachment_and_deadline` |
| s3: ignore pull registers | `released_inputs_resolve_pulls_outputs_and_irq` |
| S3: ignore edge IRQs on output writes | `released_inputs_resolve_pulls_outputs_and_irq` |
| S3: keep quiet cadence after a host release | `bus::gp_spi_board_tests::released_and_same_cycle_inputs_restore_pcnt_cadence` |
| S3: keep quiet cadence after a same-cycle read | `bus::gp_spi_board_tests::released_and_same_cycle_inputs_restore_pcnt_cadence` |
| Reboot: drop high host drive levels | `reboot_preserves_host_drives_but_resets_pad_state` |
| Reboot: leave synthetic input changes queued | `reboot_preserves_host_drives_but_resets_pad_state` |
| C6: use consecutive CS matrix signals | `spi_additional_selects_mirrors_and_disabled_routes` |
| C6: omit native additional CS pins | `spi_additional_selects_mirrors_and_disabled_routes` |
| C6: bypass pin-aware callback | `spi_native_matrix_and_software_chip_select_routes` |
| SPI: ignore CS disable and polarity | `spi_additional_selects_mirrors_and_disabled_routes` |
| SPI: omit native CS routes | `spi_native_matrix_and_software_chip_select_routes` |
| Pull-up must win simultaneous pulls | `released_inputs_resolve_pulls_outputs_and_irq` |
| s3: omit deadline-driven release | `board_releases_on_attachment_and_deadline` |
| c3: omit deadline-driven release | `board_releases_on_attachment_and_deadline` |
| c6: omit deadline-driven release | `board_releases_on_attachment_and_deadline` |
| S3: omit upper-bank input-read delivery | `first_read_delivers_feedback_for_each_width_and_bank` |
| s3: remove idle read gate | `inactive_board_never_polls_inputs` |
| c3: remove idle read gate | `inactive_board_never_polls_inputs` |
| c6: remove idle read gate | `inactive_board_never_polls_inputs` |
| S3: refresh budget on bare GPIO reads | `bus::gp_spi_board_tests::bare_input_reads_do_not_refresh_board_deadlines` |
| Record unrouted output edges | `bus::gp_spi_board_tests::bare_gpio_toggle_loop_keeps_quiet_cadence_and_no_pcnt_work` |
| Lose PCNT queue capacity | `bus::gp_spi_board_tests::routed_output_edges_reach_pcnt_and_retain_queue_capacity` |
| Never record routed output edges | `bus::gp_spi_board_tests::routed_output_edges_reach_pcnt_and_retain_queue_capacity` |
| Ignore matrix selection enable | `output_edges_only_queue_for_selected_matrix_inputs` |
| Ignore C3 matrix selector width | `matrix_output_queue_uses_chip_selector_width` |
| Do not update interrupt-enable mask | `released_inputs_resolve_pulls_outputs_and_irq` |
| Do not clear interrupt-enable mask | `bus::gp_spi_board_tests::gpio_output_level_irqs_notify_for_both_banks_and_polarities` |
| Dirty IRQs for unchanged board inputs | `unchanged_board_levels_do_not_dirty_irqs` |
| Return IRQ rather than input change | `input_change_result_does_not_depend_on_irq_configuration` |
| Track output latch instead of resolved input for IRQs | `bus::gp_spi_board_tests::output_irq_cache_tracks_resolved_input_not_output_latch` |
| Forget other matrix routes when updating one | `output_edges_only_queue_for_selected_matrix_inputs` |

## CPU comparison

Measured by the maintainer on review of `1b3e0d97`, against main `017af524`: user CPU, 9 alternating pairs, identical instruction counts and console hashes.

| Workload | main median (range) | PR median (range) | Change | PR slower in |
| --- | ---: | ---: | ---: | ---: |
| S3 hello | 26.050 (25.864–26.556) | 25.876 (25.571–26.399) | −0.67% | 1 of 9 |
| C3 hello | 2.626 (2.589–2.667) | 2.624 (2.577–2.641) | −0.09% | 3 of 9 |

No regression on either chip. The board work is behind the cached `uses_gpio_edges()` flag, so a board that doesn't opt in pays one bool test on a path that already calls `flush_ticks()`; `bare_input_reads_do_not_refresh_board_deadlines` pins that gate.

No CPU benchmarks were run. GPIO resolution runs only on GPIO/pull/host writes.
SPI route decoding runs only for a submitted transfer on an opted-in board.
Input delivery reuses existing board tick locations and is forced inline.
Release-binary symbol inspection confirms neither shared helper remains out of line. Cached
board edge opt-in skips inactive board callbacks; added state is at the end of
its containing structs. CPU parity remains subject to the central comparison.

## Evidence privacy

The historical summary retains measurements and original artifact hashes while
omitting unrelated checks, source identities unavailable on this branch, local
paths and session details. `inputs.json` records original and curated file
hashes. This limits historical reproduction to the preserved summary and public
Arduino source. Current checks retain commands and result counts, not raw logs
or machine inventories. No private originals or backup captures are committed.
