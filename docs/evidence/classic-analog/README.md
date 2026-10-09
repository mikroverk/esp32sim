# EX227: classic ADC, DAC and touch pads

Base: `esp32-classic-5-wifi`, `d538ea84`. Builds on EX226. Reuses main's
AnalogInputs, raw-count validation, sample observations and waveform timing.
Classic supplies two SAR register layouts, eighteen ADC pads, two DACs,
ten touch pads and RTC mux isolation. The shared script accepts
`touchpad <gpio> <0|1>` separately from board-panel `touch`.

No conversion work is added to tick. Conversion timestamps are captured only
on SENS writes. New state is appended to classic structs. The shared analog
conversion method becomes public rather than adding another sampler.

## Sources and limits

ESP-IDF v5.5.4 `components/soc/esp32/register/soc/sens_reg.h:473-516`
defines ADC pad/start/done controls; lines 708-739 define touch start controls.
`rtc_io_reg.h:777-795` defines DAC data/power/mux, and `rtc_io_periph.c`
defines RTC pad mapping. `include/soc/adc_channel.h` supplies ADC pin mapping.
`components/esp_adc/esp32/adc_cali_line_fitting.c:84-87` supplies nominal Vref
coefficients. The fixture uses IDF 5.5.4; IDF 4.4 is not validated here.

The voltage model inverts the nominal line-fitting coefficients with a
1100 mV Vref. It does not implement chip-specific two-point calibration or
high-range LUT correction. Touch counts (300 touched, 1000 released),
conversion completion and nominal DAC millivolts are inferred. No hardware
calibration, ADC DMA/continuous mode, DAC cosine generation or touch IRQ
model is claimed. Host analog and touch state survive reset; registers reset.

## Verification

Run the full [EX223 check set](../classic-core/README.md), including empty-HOME
workspace runs and all eight WASM scenarios. Inherited fixture/ROM input
hashes are recorded there. Existing goldens remain unchanged; no JIT changes.

[Mutation table](mutations.json) contains 23 killed replacements covering
start edges, controller selection, pad cardinality, power, width/attenuation,
DAC gates, touch routing, timestamp sampling, RTC GPIO isolation, reset and
script validation. Apply each row and run
`cargo +1.99.0 test -p CRATE --lib TEST`, then restore it.

Results on Rust 1.99.0: both required Clippy checks pass; 774 CI-mode
workspace tests and 752 plain tests (36 ignored) pass with empty HOME.
All eight WASM scenarios and evidence privacy pass. Goldens unchanged.

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
arm; script dispatch runs only when a scripted event is due.

## Overlap and privacy

Open PR #196 and merged #197 also touch the shared script file, for BLE connection
control and cycle limits respectively; neither provides capacitive touch-pad
input. No shared analog duplicate was found. Evidence contains public source
expressions and test outcomes, without private captures or machine identifiers.

The inline `sens_oneshot` helper shares classic/S3 START, DONE and DATA handling
without tick work; each caller retains its own low-START DATA behavior.
Pass-through RTC_IO and touch-timer accessors are removed.

Review limit: the optional Calibration variant is not adopted. Classic uses a
direct rounded line-fit inverse, while the shared API searches a forward
millivolt curve. Supporting both directions would add code and rounding rules,
so this would not meet the review's equal-size condition. Calibration remains
local and its existing raw-code assertions remain unchanged.
