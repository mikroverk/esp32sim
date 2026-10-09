# EX229: classic host input and I2S receive

Base: `esp32-classic-ble`
([#211](https://github.com/mikroverk/esp32sim/pull/211), `50178766`), with public
prerequisites [#201](https://github.com/mikroverk/esp32sim/pull/201) (`06c04176`) and
[#202](https://github.com/mikroverk/esp32sim/pull/202) (`725b527e`) merged by
`22d8db41`; both sides are on upstream main `2f9443a9`. The classic branch
retains its local predecessor as first parent.
The host PR must wait for those shared prerequisites; this is local ancestry,
not native GitHub stack registration.

Classic UART APB/AHB routing, matrix input, GPIO feedback/release, physical SPI,
raw ADC observations and Ethernet relay are already supplied by EX223-EX228.
This part adds I2S0/1 RX using the shared PCM packer, source bank, clocked queue
and receive scatter from EX214/EX215. The routing descriptor now gives BCLK/WS
indices explicitly because classic's data and clock signals are not adjacent.
All existing chip callers retain their previous indices.

The merge keeps one definition of the descriptor word/fault types
(`esp_periph::dma`) and one bounded descriptor walk, the `dma_descriptor_reader!`
macro: classic's bus expands it for its memory-only reader, and the bus-generic
expansion in `esp_periph::gdma` serves GDMA receive and S3's GDMA engines as in #202. Classic maps native register/interrupt values onto the
shared receive channel. Ownership, memory-only buffers and failed writeback
cannot publish successful EOF. A null next descriptor stops a completed
transfer; surplus samples report the shared descriptor-error condition.

## Idle structure and inputs

Classic fields are appended. Receiver state and the PCM bank use #202's boxed
per-controller state (the bank on controller 0), allocated on host access;
the shared queue advances lazily on host/RX access, without a per-tick source
scan. There is no separate receive Cargo feature. An inline clock/channel
gate guards receive work; the data/completion/fault helpers are inline too.
No per-tick work is added to S3/C3/C6. No CPU measurements were run.

Static check: fat-LTO release builds (`cargo +1.99.0 build --release -p esp32sim
--bins`) of this branch, #202 (`725b527e`) and main (`2f9443a9`) were disassembled
and compared per function after normalising addresses, padding and page offsets.
Against #202, every S3/C3/C6 hot function (`Machine::run`/`run_modeled`/
`run_unmodeled`/`step_core`, bus `tick`/`tick_impl`/`next_deadline`/
`refresh_tick_budget`/`periph_read`/`periph_write`, GDMA engines, `refresh_irq`)
is instruction-identical except: the RX-only helpers (C3/C6 `i2s_receive`, S3
`dma_i2s_port`), which store the two BCLK/WS indices and run only while RX is
active; and `RtcCntl::write`, `I2c::write`, `Gpio::write` and
`apply_due_script_events`, which differ from main exactly as on #211. Against
main, the remaining differences are those #202 already has. Classic's adapter
writes the shared I2S model through `Device::write`, like the other chips'
dispatch, so the shared inherent writer keeps one caller.

The inherited classic fixture/ROM hashes are in [EX223](../classic-core/README.md).
I2S tests generate their PCM samples and DMA descriptors in memory; there is no
additional firmware or external-input requirement. ADC stream tests use EX215's
shared typed raw stream and prove clocking, attenuation bypass and reset state.

## Sources and limits

Classic register definitions: ESP-IDF v5.5.4
`components/soc/esp32/register/soc/i2s_reg.h:93-98,738-756,1373-1390`.
`include/soc/gpio_sig_map.h:69-72,299,309-312,343` supplies I2S0/1 input data
and BCLK/WS indices. `include/soc/interrupts.h:50-51` supplies sources 32/33.
`register/soc/dport_reg.h:947,965` supplies I2S clock gates;
`i2s_reg.h:149-178` supplies descriptor-error and successful-EOF bits.
EX214/EX215 cite IDF v5.5.5 for their S3/C3/C6 layouts; classic uses the
v5.5.4 headers matching its inherited fixture. No IDF 4.4 validation is claimed.

The classic adapter supports standard 16/24/32-bit RX with the PLL clock.
APLL, external slave clocking, ADC-to-I2S capture, serial edge timing and classic
I2S TX are not implemented here. One shared DMA call uses RXEOF_NUM * 4,
including EOF across descriptors. IDF v5.5.4 `i2s_reg.h:676-682` defines
RXEOF_NUM at +0x24 with reset value 64; `components/hal/esp32/include/hal/i2s_ll.h:633-642`
converts bytes to register words. Word counts above u32::MAX/4 cannot fit the
shared DMA byte counter and report a descriptor error instead of wrapping.
Physical routing follows the shared matrix contract; it does not validate
IO_MUX/pad enable. No hardware microphone or audio-fidelity claim is made.
An attached empty source bank produces silence; per-controller input is used
only when no bank has been attached. Host source and queued PCM state survive
reset. No private application or firmware capture is part of these checks.

## Verification

Run the full [EX223 Rust 1.99.0 command set](../classic-core/README.md) on this
branch, including both Clippy targets, empty-HOME CI-mode and plain release
workspace tests, production WASM and all eight scenarios, and evidence privacy.

[Mutation table](mutations.json) records 19 replacements killed by assertion
failures. Run `cargo +1.99.0 test -p esp32 --test TARGET TEST` for integration
rows or `--lib TEST` otherwise, restoring source after each mutation. EX214/EX215
retain their own shared-model contracts and mutation receipts.

Results on Rust 1.99.0: both required Clippy targets pass; 816 CI-mode
workspace tests and 794 plain tests (36 ignored) pass with empty HOME.
All eight WASM scenarios pass.
Evidence privacy passes and existing goldens remain byte-identical. No JIT
implementation files changed.

The EOF regression covers a threshold spanning two descriptors, word-to-byte
scaling, reset value, zero/overflow counts and exhausted destinations. The PCM
packing test now runs exactly two frames for its two-frame buffer; overflow is
asserted separately. No firmware golden changes are needed. Shared RX tests
reuse one signal constant instead of repeated routing literals.

## CPU comparison

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 2f9443a9 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured on `bcd4df7e` against main `2f9443a9`; the branch was since rebased onto `fe3a9c08` (docs and web only) and its prerequisite branches gained docs-only receipt commits. User CPU seconds.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C6 hello | 3.2784 (3.0442–3.3351) | 3.2612 (3.0412–3.3331) | -0.52% | 2/7 | 4800000000 | 5.27 |
| Pocket Tank | 35.2968 (34.6376–38.3461) | 35.0204 (34.7165–38.7101) | -0.78% | 3/7 | 10073833665 | 4.26 |
| S3 hello | 24.6510 (23.7365–32.0593) | 24.3939 (23.8923–27.6721) | -1.04% | 4/7 | 1789819657 | 4.96 |
| C3 hello | 2.3837 (2.3635–2.5350) | 2.4178 (2.3742–2.5332) | +1.43% | 5/7 | 4800000000 | 3.53 |

### C6 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.311011 | 3.280495 | 4.96/4.96/4.96 | 4.78/4.96/4.96 | accepted |
| 1 | 2 | M→B | 3.335100 | 3.309226 | 4.96/4.80/4.96 | 4.80/4.89/4.89 | accepted |
| 2 | 3 | B→M | 3.326745 | 3.261224 | 4.89/5.06/5.06 | 4.89/4.89/4.89 | accepted |
| 3 | 4 | M→B | 3.278430 | 3.333128 | 4.73/4.51/4.73 | 4.51/4.51/4.51 | accepted |
| 4 | 5 | B→M | 3.300833 | 3.297490 | 5.27/5.01/5.27 | 4.51/5.27/5.27 | accepted |
| 5 | 6 | M→B | 3.243410 | 3.127133 | 4.40/4.40/4.40 | 4.40/4.45/4.45 | accepted |
| 6 | 7 | B→M | 3.044205 | 3.072730 | 4.41/4.41/4.41 | 4.45/4.41/4.45 | accepted |
| 7 | 8 | M→B | 3.064612 | 3.041217 | 4.41/4.54/4.54 | 4.54/4.26/4.54 | accepted |

### Pocket Tank

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 34.873879 | 35.176084 | 3.07/2.66/3.07 | 4.26/3.07/4.26 | accepted |
| 1 | 2 | M→B | 35.670774 | 34.777742 | 2.66/3.12/3.24 | 3.12/3.13/3.57 | accepted |
| 2 | 3 | B→M | 34.967871 | 34.716506 | 2.39/2.89/2.89 | 3.13/2.39/3.13 | accepted |
| 3 | 4 | M→B | 34.960377 | 34.809702 | 2.89/3.09/3.19 | 3.09/3.12/3.22 | accepted |
| 4 | 5 | B→M | 34.637622 | 35.020398 | 3.22/3.12/3.22 | 3.12/3.22/3.32 | accepted |
| 5 | 6 | M→B | 35.296837 | 38.710090 | 3.12/3.14/3.17 | 3.14/3.14/3.35 | accepted |
| 6 | 7 | B→M | 35.409490 | 38.432319 | 3.33/3.56/3.56 | 3.14/3.33/3.53 | accepted |
| 7 | 8 | M→B | 38.346102 | 35.122780 | 3.56/3.14/3.63 | 3.14/2.68/3.14 | accepted |

### S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 24.392411 | 23.938263 | 2.33/2.64/2.64 | 2.68/2.33/2.68 | accepted |
| 1 | 2 | M→B | 24.650971 | 27.672066 | 2.64/2.75/2.84 | 2.75/3.50/3.58 | accepted |
| 2 | 3 | B→M | 23.921096 | 24.166101 | 3.45/3.25/3.69 | 3.50/3.45/3.50 | accepted |
| 3 | 4 | M→B | 27.041111 | 25.220509 | 3.25/3.38/3.38 | 3.38/3.19/3.38 | accepted |
| 4 | 5 | B→M | 32.059315 | 24.092009 | 2.65/3.59/3.73 | 3.19/2.65/3.19 | accepted |
| 5 | 6 | M→B | 28.264140 | 24.393876 | 3.59/3.63/4.50 | 3.63/3.61/3.82 | accepted |
| 6 | 7 | B→M | 32.061965 | 54.232288 | 24.96/16.14/24.96 | 3.61/24.96/25.74 | discarded: load >7 |
| 6 | 8 | B→M | 23.736473 | 23.892318 | 4.21/3.30/4.21 | 4.96/4.21/4.96 | accepted |
| 7 | 9 | M→B | 23.980689 | 25.012749 | 3.30/2.95/3.30 | 2.95/2.93/2.95 | accepted |

### C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.499618 | 2.457794 | 2.93/2.93/2.93 | 2.93/2.93/2.93 | accepted |
| 1 | 2 | M→B | 2.376265 | 2.533197 | 2.93/2.94/2.94 | 2.94/2.94/2.94 | accepted |
| 2 | 3 | B→M | 2.383738 | 2.391059 | 2.78/2.78/2.78 | 2.94/2.78/2.94 | accepted |
| 3 | 4 | M→B | 2.388470 | 2.374218 | 2.78/2.88/2.88 | 2.88/2.88/2.88 | accepted |
| 4 | 5 | B→M | 2.363515 | 2.404074 | 2.89/2.89/2.89 | 2.88/2.89/2.89 | accepted |
| 5 | 6 | M→B | 2.535040 | 2.508027 | 2.89/3.14/3.14 | 3.14/3.14/3.14 | accepted |
| 6 | 7 | B→M | 2.412236 | 2.417849 | 3.53/3.53/3.53 | 3.14/3.53/3.53 | accepted |
| 7 | 8 | M→B | 2.369542 | 2.431076 | 3.53/3.40/3.53 | 3.40/3.40/3.40 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above. Raw output files are preserved.



