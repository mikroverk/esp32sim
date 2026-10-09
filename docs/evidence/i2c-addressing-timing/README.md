# EX216: I2C addressing and transfer time

Base: upstream main `2f9443a90a8cfbd81764af7fdabcbd8d33eaf283`.
S3, C3 and C6 share one controller. Device callbacks can change the primary
address, match aliases and accept write-only general calls. A single `start_address(configured, address, read)` callback matches and accepts
the address after physical pin filtering. Every matching device receives the
address and data phases. ACK is the OR of replies; read
data is their wired AND. END retains selection; STOP, route changes and device
removal update it.

Transfers advance at byte/ACK deadlines derived from the programmed bus clock.
Board time advances before peripheral callbacks. The existing optional dispatch
owns activation, ticks and deadlines; S3/C6 interrupt sources also use it, while
C3 I2C interrupts keep main's pin-source scan. Inactive controllers return
no clock and leave the active list. C3 uses its existing active pin-service path;
S3/C6 advance the board before the peripheral tick and deliver its inputs after it,
under main's existing `board_edges` gate (`drain_board_inputs` is main's
`deliver_board_inputs` without the advance). No CPU-loop fields were added; C3 pin activation uses one inlined predicate.

Related experiments: EX201 and EX206 established physical pin matching and
timestamped board callbacks but left I2C instantaneous; EX209 added removal.
EX216 adds address fanout and elapsed bus time to those existing paths.
There were no open upstream pull requests when overlapping changes were checked.

## Register sources and model limits

Public ESP-IDF headers, checked at v5.5.4, matching the panel firmware, and v4.4.7:

- [S3 i2c_reg.h, v5.5.4](https://github.com/espressif/esp-idf/blob/v5.5.4/components/soc/esp32s3/register/soc/i2c_reg.h):
  lines 16-26 low period; 72-78 TRANS_START; 111-117 FSM_RST; 173-179 BUS_BUSY;
  932-1006 high, wait-high, start and stop periods; 1043-1074 clock fields.
  C3 and C6 headers in the corresponding chip directories have the same
  9-bit period, 7-bit wait-high, reset and busy fields.
- [S3 i2c_ll.h, v5.5.4](https://github.com/espressif/esp-idf/blob/v5.5.4/components/hal/esp32s3/include/hal/i2c_ll.h):
  lines 175-201 program low/setup/hold minus one and literal high/wait-high;
  lines 205-235 define fractional A as denominator and B as numerator.
  The register header's A/B prose is reversed; the model follows the HAL.
  C6's equivalent bus configuration is at lines 173-200.
- [C6 pcr_reg.h, v5.5.4](https://github.com/espressif/esp-idf/blob/v5.5.4/components/soc/esp32c6/register/soc/pcr_reg.h#L240-L278):
  NUM at bits 12-19, A at 0-5, B at 6-11, source at 20.
  C6 uses PCR rather than the S3/C3 controller clock register.
- [S3 clk_tree_defs.h, v5.5.4](https://github.com/espressif/esp-idf/blob/v5.5.4/components/soc/esp32s3/include/soc/clk_tree_defs.h#L39):
  nominal RC_FAST 17.5 MHz. C3 uses the same value at line 39, C6 at line 47.
  The model uses a 40 MHz crystal and the existing 80 MHz APB clock.
- [S3 i2c_ll.h, v4.4.7](https://github.com/espressif/esp-idf/blob/v4.4.7/components/hal/esp32s3/include/hal/i2c_ll.h#L160-L171)
  and C3 lines 164-175 use the same bus-period convention. Arduino-ESP32 2.x
  uses IDF 4.4; 3.x uses IDF 5.x. [IDF v4.4.7 S3 soc.h, lines 243-246](https://github.com/espressif/esp-idf/blob/v4.4.7/components/soc/esp32s3/include/soc/soc.h#L243-L246) gives APB 80 MHz, RTC 20 MHz and XTAL 40 MHz;
  the model uses the IDF 5.x nominal value and does not calibrate oscillators.

The sum of programmed start/stop periods, nine clocks per byte including ACK,
one APB tick for END/empty commands, reset cancellation and command completion
semantics are inferred. No hardware timing claim. The register-driven fixtures
need no firmware SDK. Existing panel firmware exercises the production driver.

No SDA/SCL electrical waveform, arbitration, clock stretching, timeout engine,
clock gating or slave mode is modeled. Callbacks see the enclosing bus tick;
a caller consuming several deadlines in one tick sees the tick's final time.
Machine scheduler granularity still applies. C6 samples PCR on controller MMIO access; clock reconfiguration during
an active transfer is not validated. PCA9685 is a test fixture for aliases, not a
new PWM device model. No classic ESP32 or physical sensor validation is claimed.

## Verification

Both strict Clippy checks, the release build, eight production WASM demos,
WASM section/timing/BLE ABI checks, virtual-quantum tests, CI Node/Python
checks and evidence privacy checks pass.

Commands run from the repository root with Rust 1.99.0; the default toolchain
was not changed. Final check results are in `checks.json`; source, firmware and public-header hashes
are in `inputs.json`.

```sh
tools/fetch-demo-assets.sh --no-linux
cargo +1.99.0 clippy --workspace --all-targets -- -D warnings
cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
cargo +1.99.0 test --release --workspace
RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh
node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel
node tools/check-evidence-privacy.mjs
python3 docs/evidence/i2c-addressing-timing/mutate.py
```

Release workspace: **703 passed**, zero ignored under CI policy; plain tests:
**682 passed, 35 ignored**. Both results repeat with an empty HOME, with only
the ROM input for CI policy and no firmware-related variables for plain tests. Build-tool locations are
supplied separately so changing HOME does not hide the Rust installation.
Temporary test output stays inside the worktree. Reproduce the clean environment
with pre-existing Rust tool/cache directories, then run both policies:

```sh
mkdir -p target/empty-home target/test-tmp
env -i PATH="$PATH" RUSTUP_HOME="$HOME/.rustup" CARGO_HOME="$HOME/.cargo" \
  HOME="$PWD/target/empty-home" TMPDIR="$PWD/target/test-tmp" \
  ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" \
  cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
env -i PATH="$PATH" RUSTUP_HOME="$HOME/.rustup" CARGO_HOME="$HOME/.cargo" \
  HOME="$PWD/target/empty-home" TMPDIR="$PWD/target/test-tmp" \
  cargo +1.99.0 test --release --workspace
```

The mutation script restores each edited source in a `finally` block and
requires a failing test result, not a compiler error. Run it without concurrent
source edits or Rust builds. `mutations.json` is the mutation-to-killing-test
table, **44/44 killed**. Sixteen register-driven tests cover current-address replacement/removal, alias enable/disable,
general-call filtering, ACK aggregation, read collision, byte deadlines,
fractional clocks, reset, repeated starts, command exhaustion, optional dispatch
and board callback order on all three chips. The S3/C6 board-order mutations
restore main's advance-after-tick delivery. The last five mutations run the
`esp-periph` `devices` and `esp32c3` library tests listed under Idle path.

## Intentional golden changes

Only `panel-sid.console.txt`, `panel-sid.insns` and `panel-sid.wav.sha256`
change. I2C expander initialization now consumes bus time. Panel main startup
moves from 725 to 1147 ms; instructions from 396469561 to 398637113.
The count includes a final scheduling round cut short at the seven-second
cycle ceiling (#197); console, report and WAV do not depend on that cut.
The fixed seven-second audio window changes accordingly. Console content other
than timestamps is identical. `panel-sid.report.txt` additionally pins stop
interrupt totals and per-source counts. Other existing goldens remain exact.

Old WAV SHA-256:
`89880538c0dc82e11f74bfdd7a4e98e02cc793cfd806bdb78c08e6c9b1378dfe`.
New WAV SHA-256:
`37b4961b189338e0977f283e2485080f5f8078380fac956f969283cc20ca2171`.

## Pocket Tank correctness

[Console comparison, transfer trace, instantaneous control and hardware-model limits](pocket-tank.md):
finite I2C time shifts initialization by 1–2 ms and changes the seeded fish
workload. No new NACK or transfer failure; restoring synchronous completion
reproduces main's console and instruction/interrupt totals exactly. The
AMOLED board transfers are pinned by a firmware-independent register test.

## Idle path

While no transfer is active, I2C does no per-tick or per-deadline-query work
on any chip. Changes reachable on that path, relative to main:

| Path | Change | Idle cost |
| --- | --- | --- |
| Tick delivery (`device_set!` `tick`) | I2C is an `optional` entry | None: existing empty-list branch; optional devices tick only from `active_optional`. |
| Deadline query (`device_set!` `cycles_until_deadline`) | Optional entries leave the static scan; one empty-list branch calls a non-inlined scan over `active_optional` | I2C is not polled. On C3 the static scan no longer reads `BLE_LC` state, replaced by the list-length load; S3/C6 add that load and branch. |
| C3 interrupt sources | I2C's optional entry lists no source | `OPTIONAL_SOURCES` and generated `source_status()` equal main; `pin_irqs_enabled`/`refresh_pin_lines()` still own `I2C_EXT0`. |
| C3 tick (`SocBus::tick`) | `pins_active()` includes `i2c.is_active()` | None while `pins_active` is false: `tick_with_pins()` is skipped as on main. |
| S3/C6 tick (`tick_impl`/`devices`) | The board advance moves ahead of `periph.tick` under the same `board_edges` test; edges and releases drain after it | Same calls as main: one `advance_to` and one edge/release drain per tick, only when `board_edges`; one extra test of the cached flag. |
| C3 MMIO write (`periph_write`) | `Peripherals::write32` returns its existing I2C/RMT/SPI2 block test; the bus refreshes `pins_active` on it | No new address test; SPI2 writes also refresh `pins_active` (idempotent). |

Deadline equivalence: an optional device is listed whenever `clock()` is
`Some`; `refresh_optional` runs after its writes and ticks. I2C `has_deadline()`,
`next_deadline()` and `clock()` all follow `active`. C3 `BLE_LC` is enabled
iff `clock()` is `Some`; `enable_ble_full` refreshes it and nothing disables
it. LEDC and MCPWM implement no deadline. Deadline values, divider and
rounding are unchanged.

Tests: `optional_devices_are_not_polled_until_configured_and_keep_stopped_irqs`
counts `has_deadline()`/`next_deadline()` calls before configuration and after
stop, and checks an active APB deadline (7 ticks, divider 3: 18 CPU cycles).
`pin_clock_participation_tracks_i2c_transfers` checks that TRANS_START enters
and completion leaves C3 pin service, with a 6-cycle STOP deadline.
`wifi_and_pin_sources_survive_interrupt_refresh` rejects C3 pin-source bits in
the optional cache. `controller_leaves_optional_dispatch_after_completion_and_reset`
checks I2C deadline methods before, during and after a transfer and after reset.

## CPU comparison

PENDING

No CPU benchmarks were run for this revision. Central comparison remains required before merge.

## Evidence scope

The receipt retains public input hashes, commands, assertions and results.
Raw compiler/test output and downloaded headers are omitted. No personal paths,
host identities, device identifiers or private firmware are retained.
