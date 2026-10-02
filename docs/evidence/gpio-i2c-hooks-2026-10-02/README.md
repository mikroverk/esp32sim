# GPIO snapshots and I2C removal

EX209 adds a read-only `SocBus::gpio_state(pin)` and I2C device removal on
upstream base `dddb128052dca15250e2169b92ab73c4d87f524c`. Implementation and tests
are at `1582deb2fff03d060ecb5739d2dd8b457dfe231d`. No fork or classic-ESP32
changes are dependencies.

`GpioState` reports the GPIO output latch, output enable, and programmed IO_MUX
pull-up and pull-down bits. Unsupported pins return `None`, including S3 pins
22 through 25. The default trait method returns `None` to keep external bus
implementations compatible. The snapshot does not resolve electrical input,
RTC-pad ownership, peripheral-matrix output or open-drain behavior.

`I2c::detach(address)` returns the removed device for reattachment and updates
the current device index. Removing another device preserves the selection.
Removing the selected device makes subsequent writes NACK and reads return
`0xff` until another address phase. `clear_devices()` removes all devices while
preserving registers, FIFOs, counters and interrupt status.

On S3, `bus.clear_i2c_devices()` clears both controllers, including devices
attached directly. A host can then replace its board and call the existing
`attach_board_devices()`. This explicit sequence preserves the existing attach
method's additive behavior. C3 and C6 have no digital I2C attachment path at this
base; adding their controllers is separate work.

The behavior reference was `221080ffccfa829106b398f896653535853c76c8`, inspected
with `git diff dddb128 221080f -- esp-periph/src/i2c.rs` and `git show` for the
three chip `soc.rs` files, peripheral IO_MUX writes and `wasm/src/lib.rs`.
The fork's electrical pull model, timed I2C execution and product ABI were not
ported. The prior experiment search covered GPIO, pull-up, pull-down, snapshot,
I2C and detach. Related EX028 concerns cached GPIO interrupt summaries; EX209
instead checks host observation and attachment correctness. No speed claim.

The five default tests in [gpio_i2c_hooks.rs](../../../cli/tests/gpio_i2c_hooks.rs)
cover every valid GPIO, the upper S3 register bank, invalid pins, independent
output latch and enable, both pull bits and the MMIO effects of INPUT_PULLUP,
INPUT_PULLDOWN and OUTPUT. They also cover absent-address detach, removal on
both sides of a selected index, continuation after detach, reattachment, clear
and moving a board device between S3 controllers without a reset.

The opt-in test boots the unchanged [Arduino sketch](main.cpp) through the S3
ROM and bootloader. Serial checkpoints let the host inspect the snapshot after
actual `pinMode` calls and change attachments before the next transmission.
`Wire.endTransmission()` returns `0, 2, 0, 2` for attached, detached, reattached
and cleared devices. All eight serial checkpoints pass, with 11,000,000 cycles,
4,694,155 retired instructions and zero reboots. No firmware stubs or emulator
register writes are used by this test. This checks native execution, not browser
execution or physical hardware. C3 and C6 coverage is register-driven.

[validation.json](validation.json) records commands, results, input hashes and
limitations. PlatformIO initially failed to acquire its cache lock under the
filesystem sandbox. The retry with cache write access passed. The first
workspace run reached doctests and failed with `E0463: can't find crate for
esp32s3`; a focused rebuild overlapped it. The receipt records the isolated retry
separately. The isolated retry passed 466 tests with 23 ignored, including the
new firmware test, which passed separately. Release and WASM builds passed.
These initial failures are retained, not counted as passing runs.

Only compact results and the test's serial checkpoints are retained. Compiler
logs, package-cache paths and unrelated boot output are omitted. No login,
hostname, device identifier or process inventory is retained. The omission does
not change the numeric results or input hashes. Firmware binaries are identified
by hashes and can be rebuilt; the temporary PlatformIO `.pio` directory was
removed after validation.

To reproduce from the repository root, supply your S3 ROM path in `HOOKS_ROM`:

```sh
hooks_project=$(mktemp -d /tmp/esp32sim-hooks.XXXXXX)
mkdir "$hooks_project/src"
cp docs/evidence/gpio-i2c-hooks-2026-10-02/platformio.ini "$hooks_project/"
cp docs/evidence/gpio-i2c-hooks-2026-10-02/main.cpp "$hooks_project/src/"
pio run -d "$hooks_project"
HOOKS_FIRMWARE="$hooks_project/.pio/build/s3" cargo test --release -p esp32sim --test gpio_i2c_hooks arduino_s3_gpio_and_i2c_detach -- --ignored --nocapture
cargo test -p esp32sim --test gpio_i2c_hooks
cargo build --release
cargo test --workspace
tools/wasm-build.sh
node tools/check-evidence-privacy.mjs
git diff --check
rm -rf "$hooks_project/.pio"
```

Run Cargo commands sequentially. Export `HOOKS_ROM` before the firmware test.
The test requires Espressif's `esp32s3_rev0_rom.elf`; the receipt identifies the
ROM used here. The PlatformIO project pins pioarduino `55.03.38-1`, Arduino-ESP32
3.3.8 and board `esp32-s3-devkitc-1`.
