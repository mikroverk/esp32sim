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
All 32 mutations fail an assertion, rather than merely failing compilation.
Existing golden files remain byte-identical; none were regenerated.

## CPU comparison

PENDING

C3 RX uses its existing cached pending-work branch. C6 combines SPI and RX in
one cached work flag, refreshed on MMIO writes and SPI completion. Both keep
plain I2S fields at the end of the peripheral structs. RX pumps return their
sample buffer for reuse; no buffer allocation occurs on an idle tick. S3 retains
its existing active-cadence gate. No generic device tick was added. Scheduler
deadline selection includes active RX; central CPU qualification remains pending.

## Limits

No microphone firmware golden, hardware audio capture, pin serializer, filter,
slave clock or audio-fidelity claim. The tests pin exact sample bytes, EOF,
ownership, channel errors and reset behavior. Existing full-run goldens cover
unrelated firmware. Source snapshots and result rows omit local paths and
machine identity; no raw capture was retained or redacted.
