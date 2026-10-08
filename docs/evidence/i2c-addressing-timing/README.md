# EX216: I2C addressing and transfer time

Base: upstream main `017af5241f48855c412d912ff91f0a801dd3c1bd`.
S3, C3 and C6 share one controller. Device callbacks can change the primary
address, match aliases and accept write-only general calls. A single `start_address(configured, address, read)` callback matches and accepts
the address after physical pin filtering. Every matching device receives the
address and data phases. ACK is the OR of replies; read
data is their wired AND. END retains selection; STOP, route changes and device
removal update it.

Transfers advance at byte/ACK deadlines derived from the programmed bus clock.
Board time advances before peripheral callbacks. The existing optional dispatch
owns activation, deadlines and interrupt sources. Inactive controllers return
no clock and leave the active list. C3 uses its existing active pin-service path;
S3/C6 reorder existing board callbacks. No CPU-loop fields were added; C3 pin activation uses one inlined predicate.

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

Release workspace: **657 passed**, zero ignored under CI policy; plain tests:
**636 passed, 35 ignored**. Both results repeat with an empty HOME, with only
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
table, **39/39 killed**. Sixteen register-driven tests cover current-address replacement/removal, alias enable/disable,
general-call filtering, ACK aggregation, read collision, byte deadlines,
fractional clocks, reset, repeated starts, command exhaustion, optional dispatch
and board callback order on all three chips.

## Intentional golden changes

Only `panel-sid.console.txt`, `panel-sid.insns` and `panel-sid.wav.sha256`
change. I2C expander initialization now consumes bus time. Panel main startup
moves from 725 to 1147 ms; instructions from 396469561 to 398637173.
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

## Idle deadline dispatch

`I2c::has_deadline()` returns `self.active`, matching `next_deadline()`.
The shared `device_set!` implementation in `esp-periph/src/mmio.rs` checks
this gate before requesting a deadline. S3, C3 and C6 all use that dispatch
through `cycles_until_timer()`; S3 has two I2C controllers. The gate is read
dynamically, so starting a transfer makes its deadline visible immediately.
No deadline calculation runs for an idle controller.

`controller_leaves_optional_dispatch_after_completion_and_reset` checks both
deadline methods before any transfer, during a transfer, after completion and
after reset. Forcing the gate always true or always false fails this test.
The Pocket Tank seed-change receipt and intentional panel goldens are retained.
CPU impact of this gate requires the central comparison; no speed claim here.

## CPU comparison

PENDING

No CPU benchmarks were run. Central comparison remains required before merge.

## Evidence scope

The receipt retains public input hashes, commands, assertions and results.
Raw compiler/test output and downloaded headers are omitted. No personal paths,
host identities, device identifiers or private firmware are retained.
