# EX215: pin-routed audio sources and ADC streams

Builds on the local `i2s-rx-input` branch (EX214, `06c04176`), above upstream
main `2f9443a9`.
This adds host source clocks and pin routing to the existing receiver. It does
not add a second DMA walker or input transport. ADC streams use main's analog
source replacement and completed-conversion observation paths.

## Contract

Microphone sources in a dynamically sized slot vector carry stereo PCM16 at 8–96 kHz. Each has its own
sample clock and a two-second queue. Pushes drop the oldest queued frames.
RX uses zero-order resampling, with one shared timeline when two receivers hear
the same source. An empty queue yields silence. A source matches data input,
BCLK and WS matrix routes, including inversion bits. PDM sources match the
converted-PDM mode and clock/data routes. The lowest matching slot wins.
Without a bank, controller-bound EX214 input remains available. An attached
empty bank produces silence. Controller input is a `PcmSource` too; one packing
function accepts the selected producer as a closure.

Source banks are allocated on first host access, inside controller 0's lazily
allocated EX214 receiver state; every receiver of the chip borrows that one
bank for its RX interval. Their clocks advance lazily
from absolute emulated cycles at host access and before an active RX interval.
Unselected/stopped sources therefore lose elapsed samples without any new
per-tick loop. Obtain `SocBus::pcm_sources()` before pushing or inspecting its
slots. Sources, clock phase and buffered samples survive a chip reset.

`AnalogStream` attaches through `AnalogSource::Stream`. Host handles share a
bounded queue. Host timestamps are bus cycles (`bus.cycles()`). Conversions
keep main's peripheral-clock timestamp. Each attached copy records the bus
cycle at which that clock started, set on attach and on chip reset, so samples
stay aligned to bus time across resets. Attach a stream to one chip only. Voltage streams carry finite `f32` volts through the existing calibration
curve. `AnalogSource::RawStream` carries `u16` counts, accepts only `push_raw`,
and bypasses attenuation/calibration. Both use the same `ClockedQueue<T>` as
I2S for sample clocks, phase and the two-second drop-old buffer. Conversion
uses `AnalogInputs.cpu_hz`; streams store no duplicate clock frequency. Invalid pushes
leave queue and time intact. ADC underrun holds the last sample, unlike I2S
silence. Source replacement retains the existing generation accounting rule.

## Register references

ESP-IDF v5.5.5, public Arduino-ESP32 3.3.11 headers:

- `components/soc/esp32s3/include/soc/gpio_sig_map.h`, lines 56–71:
  I2S0 data/BCLK/WS 25/26/27 and I2S1 30/31/32.
- `components/soc/esp32c3/include/soc/gpio_sig_map.h`, lines 38–43:
  data/BCLK/WS 15/16/17. C6 has these at lines 34–39 in its corresponding file.
- `components/soc/esp32s3/register/soc/gpio_reg.h`, lines 2672–2678 and
  7804–7810: input selector bit 7, inversion bit 6, output inversion bit 9.
  C3 uses bits 6/5/8 at 1325–1331 and 3897–3903; C6 uses bits 7/6/8 at
  2466–2473 and 4998–5005. Output selector widths are 9/8/8 bits.

`headers.json` hashes the checked files. ADC register values and transfer
curves are unchanged from main. No IDF 4.4 or Arduino 2.x compatibility claim.
The clock/resampling/underrun policies are inferred model contracts, not
hardware measurements. Pin routing does not model pad enable, IO_MUX or
serial edges; PDM input already contains converted PCM.

## Reproduction and results

```sh
cargo +1.99.0 test -p esp32sim --test pcm_sources --test adc_stream
cargo +1.99.0 test -p esp-periph --lib
python3 docs/evidence/audio-sources-adc-stream/mutations.py
```

The source tests pin route changes, multiple rates, both S3 ports, PDM wiring,
reset, DMA stalls and exact samples. ADC tests pin exact raw counts for all
attenuations on three chips and conversion generations. Unit tests cover
phase, long stalls, source priority, replacement, validation and drop-old
queues. `mutations.json` maps 44 removal mutations to their killing tests.

`checks.json` records required native/WASM checks. Workspace tests run with an
empty `HOME`, with only `ESP32SIM_ROM_DIR` as a firmware-input variable for the
CI-policy suite and no firmware variables for the plain suite. Installed Rust
1.99.0 remains available through `CARGO_HOME` and `RUSTUP_HOME`. Demo/ROM inputs
come from `tools/fetch-demo-assets.sh --no-linux`; hashes are retained in EX214.
No new firmware fixture or golden is introduced; existing goldens are unchanged.
No JIT implementation changes are included.

## CPU comparison

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 2f9443a9 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured on `725b527e` against main `2f9443a9`; the branch was since rebased onto `fe3a9c08` (docs and web only) with no change to its own diff. User CPU seconds.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C6 hello | 3.0334 (3.0068–3.0673) | 3.0068 (2.9940–3.0870) | -0.88% | 1/7 | 4800000000 | 4.89 |
| S3 hello | 23.8686 (23.7285–24.0412) | 23.8469 (23.6671–24.9067) | -0.09% | 2/7 | 1789819657 | 3.57 |
| C3 hello | 2.3511 (2.3046–2.3642) | 2.3359 (2.2981–2.3749) | -0.64% | 5/7 | 4800000000 | 1.92 |

### C6 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.029614 | 3.157441 | 2.18/4.89/4.89 | 2.28/2.18/2.28 | accepted |
| 1 | 2 | M→B | 3.006775 | 2.995204 | 4.89/4.89/4.89 | 4.89/4.57/4.89 | accepted |
| 2 | 3 | B→M | 3.040816 | 3.006795 | 4.57/4.29/4.57 | 4.57/4.57/4.57 | accepted |
| 3 | 4 | M→B | 3.044706 | 3.008675 | 4.29/4.10/4.29 | 4.10/4.10/4.10 | accepted |
| 4 | 5 | B→M | 3.019732 | 3.009810 | 3.85/3.85/3.85 | 4.10/3.85/4.10 | accepted |
| 5 | 6 | M→B | 3.033365 | 2.993991 | 3.85/4.03/4.03 | 4.03/3.86/4.03 | accepted |
| 6 | 7 | B→M | 3.029658 | 3.004105 | 3.86/3.79/3.86 | 3.86/3.86/3.86 | accepted |
| 7 | 8 | M→B | 3.067250 | 3.087006 | 3.79/3.79/3.79 | 3.79/3.57/3.79 | accepted |

### S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 23.700089 | 23.734826 | 2.93/2.27/2.93 | 3.57/2.93/3.57 | accepted |
| 1 | 2 | M→B | 24.041194 | 24.035328 | 2.27/2.06/2.27 | 2.06/1.77/2.06 | accepted |
| 2 | 3 | B→M | 23.908291 | 23.721733 | 1.76/1.57/1.76 | 1.77/1.76/1.90 | accepted |
| 3 | 4 | M→B | 23.743918 | 23.667108 | 1.57/1.61/1.61 | 1.61/2.06/2.15 | accepted |
| 4 | 5 | B→M | 23.928048 | 23.846858 | 1.77/1.66/1.77 | 2.06/1.77/2.06 | accepted |
| 5 | 6 | M→B | 23.728536 | 23.877840 | 1.66/1.70/1.82 | 1.70/1.59/1.70 | accepted |
| 6 | 7 | B→M | 23.868581 | 24.906742 | 2.01/1.91/2.09 | 1.59/2.01/2.01 | accepted |
| 7 | 8 | M→B | 23.800235 | 23.739825 | 1.91/1.79/1.93 | 1.79/1.65/1.83 | accepted |

### C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.342261 | 2.290093 | 1.92/1.92/1.92 | 1.65/1.92/1.92 | accepted |
| 1 | 2 | M→B | 2.319549 | 2.298133 | 1.92/1.92/1.92 | 1.92/1.92/1.92 | accepted |
| 2 | 3 | B→M | 2.357739 | 2.317445 | 1.92/1.85/1.92 | 1.92/1.92/1.92 | accepted |
| 3 | 4 | M→B | 2.304637 | 2.326926 | 1.85/1.85/1.85 | 1.85/1.78/1.85 | accepted |
| 4 | 5 | B→M | 2.364159 | 2.365696 | 1.78/1.80/1.80 | 1.78/1.78/1.78 | accepted |
| 5 | 6 | M→B | 2.358388 | 2.374883 | 1.80/1.80/1.80 | 1.80/1.81/1.81 | accepted |
| 6 | 7 | B→M | 2.351075 | 2.362864 | 1.81/1.75/1.81 | 1.81/1.81/1.81 | accepted |
| 7 | 8 | M→B | 2.314663 | 2.335915 | 1.75/1.75/1.75 | 1.75/1.69/1.75 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above. Raw output files are preserved.

No source-bank or ADC-stream tick hook is installed. The bank lives in I2S0's
lazily allocated receiver state, so no bus or peripheral struct gains a field.
Its users are hosts and the already-active EX214 RX path. The C3/C6 RX pump calls one out-of-line
`i2s_receive` helper where EX214 called `rx_data`. ADC conversion time still
comes from main's per-block `pre_access` assignment. Stream time offsets are
written only on attach and on chip reset; the MMIO paths are unchanged.

Release-binary disassembly (`cargo +1.99.0 build --release --bins`, aarch64,
addresses, padding and page offsets normalised) against EX214 `06c04176`:

- Identical: C3/C6 `SocBus::tick`, `next_deadline`, `refresh_irq`,
  `periph_read`, `periph_write` and `Peripherals::read32`; S3 `tick_impl`,
  `refresh_tick_budget`, `periph_read` and `periph_write_inner`;
  `Machine::step_core` on all three chips.
- C3/C6 `pending_work` (SPI command or RX active): the RX call site changes
  from `rx_data` to `i2s_receive`, one instruction fewer. Receiver-buffer
  offsets in the boxed `RxState` also differ.
- Script-event stride: `AnalogSource` grows from 24 to 32 bytes; four variants
  no longer fit the `Arc` niche. `(u64, ScriptAction)` therefore grows from 48 to 56 bytes. `Machine::run`,
  `run_unmodeled`, `settle_modeled_time`, `after_round` and S3 `bb_quanta`
  replace an `add`+`lsl` index pair with `sub`+`lsl`. Only the `pos < len`
  pending-script branch reaches that code. Script parsing, reboot,
  constructors and drop glue are cold.

## Limits

No unmodified microphone firmware acceptance, hardware capture, RF/audio timing,
PDM filter, anti-aliasing or waveform-fidelity claim. The buffers are intended
for bounded host ingestion; ADC hosts should push between emulator runs using
that chip's current cycle count. Only reproduction commands, source/input hashes
and compact outcomes are retained, with no machine identity or private paths.
