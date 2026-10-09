# EX215: pin-routed audio sources and ADC streams

Builds on the local `i2s-rx-input` branch, EX214, above upstream `017af524`.
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
bounded queue; conversion samples it at the bus cycle. Voltage streams carry finite `f32` volts through the existing calibration
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
queues. `mutations.json` maps 34 removal mutations to their killing tests.

`checks.json` records required native/WASM checks. Workspace tests run with an
empty `HOME`, with only `ESP32SIM_ROM_DIR` as a firmware-input variable for the
CI-policy suite and no firmware variables for the plain suite. Installed Rust
1.99.0 remains available through `CARGO_HOME` and `RUSTUP_HOME`. Demo/ROM inputs
come from `tools/fetch-demo-assets.sh --no-linux`; hashes are retained in EX214.
No new firmware fixture or golden is introduced; existing goldens are unchanged.
No JIT implementation changes are included.

## CPU comparison

PENDING

No source-bank or ADC-stream tick hook is installed. The bank lives in I2S0's
lazily allocated receiver state, so no bus or peripheral struct gains a field;
hosts and the already-active EX214 RX path are its only users. Analog source
variants hold one shared handle and do not enlarge `AnalogSource`. ADC time is
set in the bus write path for the converter block, moving the existing block
check out of the per-access hook (reads included) on `954f2a68`. Idle
structure of the EX214 base is described in its receipt.

Release-binary disassembly (same method as EX214) against the EX214 commit on
this branch: C3/C6 `SocBus::tick`, S3 `tick_impl` and `refresh_tick_budget`,
and C3/C6 `next_deadline` are instruction-identical. C6 `periph_write` goes
1656 → 1651 and S3 `periph_write_inner` 1664 → 1632 instructions.

## Limits

No unmodified microphone firmware acceptance, hardware capture, RF/audio timing,
PDM filter, anti-aliasing or waveform-fidelity claim. The buffers are intended
for bounded host ingestion; ADC hosts should push between emulator runs using
that chip's current cycle count. Only reproduction commands, source/input hashes
and compact outcomes are retained, with no machine identity or private paths.
