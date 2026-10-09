# EX214: I2S receive input

Base: upstream `017af524`. This branch ports controller-bound stereo PCM input onto
main's camera/crypto GDMA receive path. It retains the existing C3 GDMA adapter;
there is exactly one register-layout translation. EX212 owns the existing scatter
semantics. EX214 adds another producer and moves that walker into `esp-periph` so
all three chips use it.

S3 controllers 0 and 1 and C3/C6 controller 0 accept 16-bit host frames. Standard
RX packs mono/stereo 16-bit words or left-aligned 24/32-bit samples. S3 controller
0 accepts converted PDM16. Queued input and generated tones survive chip reset.
Stopped receivers and unbound DMA channels do not consume the per-port queue.
Unsupported slave, raw-PDM, extended TDM, companding and reordered formats do not
advance it. These are PCM-boundary model rules, not measurements of serial pins
or PDM filter behavior.

## Register sources

Compared with the public headers shipped in Arduino-ESP32 3.3.11, ESP-IDF v5.5.5:

- `components/soc/esp32s3/register/soc/i2s_reg.h`: RX_CONF at line 119,
  mode bits at 123–246, widths/divider at 375–416, clocks at 466–565,
  slots at 677–692 and byte-count EOF at 1058–1065.
- `components/soc/esp32c3/register/soc/i2s_reg.h`: standard RX mode fields
  at 117–230, widths/divider at 375–390 and slots at 657–660.
- `components/soc/esp32c6/register/soc/i2s_reg.h`: mode fields at 149–290,
  widths/divider at 424–478 and slots at 930–945. Its clocks instead live in
  `components/soc/esp32c6/register/soc/pcr_reg.h`, lines 719–784.
- `components/hal/esp32s3/include/hal/i2s_ll.h`: RX clock selectors at 257,
  fractional divider programming at 387. C6 selectors are in its `i2s_ll.h`,
  lines 270–285. XTAL/PLL240/PLL160 selectors are 0/1/2; external is unsupported.
- `components/soc/esp32s3/register/soc/gdma_reg.h`: descriptor error,
  successful EOF and done bits at 115–146. C3's existing adapter maps the
  shared status into its combined interrupt register.

The focused tests program these registers directly. No new firmware fixture is
required, and this receipt makes no Arduino 2.x / IDF 4.4 compatibility claim.
Source hashes are in `headers.json`.

## Reproduction and verification

Fetch assets with `tools/fetch-demo-assets.sh --no-linux`. `checks.json` records
the required native/WASM Clippy, CI-policy workspace, plain workspace and eight
production WASM demo commands and exit statuses. Workspace runs use an empty
`HOME`; only the CI-policy run sets `ESP32SIM_ROM_DIR` to `web/wasm/fw`'s absolute
path. `CARGO_HOME` and `RUSTUP_HOME` retain access to the installed Rust 1.99.0
toolchain. No local firmware or private fixture is used.

Focused contracts live in `cli/tests/i2s_rx.rs` and
`esp-periph/src/i2s/rx.rs` and `esp-periph/tests/i2s_rx_contracts.rs`. Run the removal checks with:

```sh
python3 docs/evidence/i2s-rx-input/mutations.py
```

`mutations.json` is the mutation-to-killing-test table. The script requires a
clean source snapshot and restores each file before starting the next mutation.
All 40 mutations fail an assertion, rather than merely failing compilation.
Existing golden files remain byte-identical; none were regenerated.

## CPU comparison

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 954f2a68 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured on `c4ffbf4dc0460a24e2a3c220240a4fd4821f6868` against main `954f2a68`; the branch was since rebased onto `fe3a9c08` with no change to its own diff. User CPU seconds.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C6 hello | 3.2518 (3.2310–3.3164) | 3.3031 (3.2661–3.4019) | +1.58% | 5/7 | 4800000000 | 2.90 |
| S3 hello | 24.9721 (24.7991–25.3546) | 24.9517 (24.7022–26.6243) | -0.08% | 3/7 | 1789819657 | 2.47 |
| C3 hello | 2.4401 (2.4295–2.4881) | 2.4553 (2.4103–2.4856) | +0.62% | 4/7 | 4800000000 | 1.89 |

### C6 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.317685 | 3.617584 | 2.75/2.75/2.75 | 2.90/2.75/2.90 | accepted |
| 1 | 2 | M→B | 3.253574 | 3.401929 | 2.75/2.61/2.75 | 2.61/2.56/2.61 | accepted |
| 2 | 3 | B→M | 3.251834 | 3.326952 | 2.56/2.43/2.56 | 2.56/2.56/2.56 | accepted |
| 3 | 4 | M→B | 3.316397 | 3.303052 | 2.43/2.32/2.43 | 2.32/2.32/2.32 | accepted |
| 4 | 5 | B→M | 3.233875 | 3.358455 | 2.21/2.51/2.51 | 2.32/2.21/2.32 | accepted |
| 5 | 6 | M→B | 3.277006 | 3.266109 | 2.51/2.51/2.51 | 2.51/2.39/2.51 | accepted |
| 6 | 7 | B→M | 3.251333 | 3.298689 | 2.44/2.44/2.44 | 2.39/2.44/2.44 | accepted |
| 7 | 8 | M→B | 3.230994 | 3.287996 | 2.44/2.33/2.44 | 2.33/2.30/2.33 | accepted |

### S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 24.789521 | 25.202236 | 2.05/1.88/2.05 | 2.30/2.05/2.30 | accepted |
| 1 | 2 | M→B | 25.284845 | 24.757919 | 1.88/1.96/2.05 | 1.96/1.70/1.96 | accepted |
| 2 | 3 | B→M | 24.814118 | 24.762465 | 1.60/1.54/1.60 | 1.70/1.60/1.71 | accepted |
| 3 | 4 | M→B | 24.972055 | 25.579252 | 1.54/1.49/1.58 | 1.49/1.75/1.75 | accepted |
| 4 | 5 | B→M | 24.799061 | 26.624315 | 1.87/2.47/2.47 | 1.75/1.87/1.95 | accepted |
| 5 | 6 | M→B | 24.930923 | 25.166946 | 2.47/2.19/2.47 | 2.19/1.86/2.19 | accepted |
| 6 | 7 | B→M | 25.354633 | 24.702150 | 1.69/1.66/1.69 | 1.86/1.69/1.86 | accepted |
| 7 | 8 | M→B | 25.220010 | 24.951730 | 1.66/1.57/1.66 | 1.57/1.89/1.89 | accepted |

### C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.424768 | 2.423405 | 1.89/1.82/1.89 | 1.89/1.89/1.89 | accepted |
| 1 | 2 | M→B | 2.451351 | 2.467182 | 1.82/1.82/1.82 | 1.82/1.75/1.82 | accepted |
| 2 | 3 | B→M | 2.488115 | 2.455299 | 1.75/1.69/1.75 | 1.75/1.75/1.75 | accepted |
| 3 | 4 | M→B | 2.453692 | 2.485630 | 1.69/1.69/1.69 | 1.69/1.72/1.72 | accepted |
| 4 | 5 | B→M | 2.433062 | 2.483267 | 1.72/1.66/1.72 | 1.72/1.72/1.72 | accepted |
| 5 | 6 | M→B | 2.436326 | 2.421284 | 1.66/1.66/1.66 | 1.66/1.69/1.69 | accepted |
| 6 | 7 | B→M | 2.440081 | 2.410282 | 1.69/1.71/1.71 | 1.69/1.69/1.69 | accepted |
| 7 | 8 | M→B | 2.429529 | 2.443767 | 1.71/1.71/1.71 | 1.71/1.65/1.71 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above. Raw output files are preserved.

Idle structure, for firmware that never enables I2S RX:

- Receiver queue, frame phase and sample buffer sit behind one lazily allocated
  pointer; an unused controller allocates no receiver state. C3 and C6 hold the
  whole controller in a `Box`.
- S3: one cached flag per controller (TX or RX enabled, refreshed on RX_CONF and
  TX_CONF writes) gates the per-flush DMA pump and the active-cadence check. It
  replaces the TX-only test there, so an idle flush does main's work.
  RX_CONF/TX_CONF are crate-private so nothing bypasses the flag; the existing
  S3 cadence test now enables TX by register write instead of a field poke.
- C3/C6: an RX_CONF write returns a write effect. The SoC tests it in the same
  mask test as the existing SPI effect, and only then refreshes pending work.
  C6's pending-work flag replaces its SPI check in the device step. C3 keeps its
  existing per-write pending-work refresh, with one cached boolean term added.
- Remaining idle work: a C3/C6 deadline query tests one cached boolean (C3
  `i2s_rx`, C6 `work_pending`) before applying the 256-cycle RX bound.

Disassembly of `cargo +1.99.0 build --release --bins` output (fat LTO) via
`objdump -d --disassemble-symbols=<symbol>`, against `954f2a68`:

| Function | Instructions | Difference |
| --- | ---: | --- |
| C3 `SocBus::tick` | 279 → 282 | alignment padding only |
| C6 `SocBus::tick` | 1968 → 1968 | SPI branch outlined; two field offsets +8 bytes |
| S3 `SocBus::tick_impl` | 3648 → 3600 | — |
| S3 `SocBus::refresh_tick_budget` | 280 → 279 | — |
| C3 `next_deadline` | 185 → 200 | byte load and branch on the idle path |
| C6 `next_deadline` | 285 → 290 | byte load and branch on the idle path |
| C6 `SocBus::periph_write` | 1594 → 1656 | I2S block arm; RX_CONF shares the SPI effect test |
| S3 `SocBus::periph_write_inner` | 1632 → 1664 | field offsets +16 bytes; I2S write arms longer |

## Limits

No microphone firmware golden, hardware audio capture, pin serializer, filter,
slave clock or audio-fidelity claim. The tests pin exact sample bytes, EOF,
ownership, channel errors and reset behavior. Existing full-run goldens cover
unrelated firmware. Source snapshots and result rows omit local paths and
machine identity; no raw capture was retained or redacted.
