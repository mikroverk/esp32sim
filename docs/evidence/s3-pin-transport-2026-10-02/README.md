# S3 pin transport validation

Implementation: `edd985d616276fceae1b1b708c5802370079e2d7`, based directly on
upstream `dddb128052dca15250e2169b92ab73c4d87f524c`. Behavior reference:
`221080ffccfa829106b398f896653535853c76c8`, especially `I2cDevice::pins`,
`SpiPins`, and the fork's SPI route decoder. No fork device models or ABI were copied.

This is EX201, a new attachment and callback correctness workload. Related EX047
sets scheduler granularity, EX056 covers board input timing, and EX190 covers SPI2
MMIO dispatch. None of those mechanisms is changed or benchmarked here.

## Contract

- `I2cDevice::pins()` optionally returns SDA and SCL. The device remains attached
  to the controller returned by `BoardModel::i2c_devices()`. A board that permits
  either controller can return two wrappers over shared device state. Matching
  requires both matrix input and output routes, IO_MUX GPIO selection, and enabled
  IO_MUX input. Inverted routes and invalid pins do not match. Replacement uses
  address plus pin pair, so equal addresses on different pairs coexist. `None`
  retains controller-only addressing; avoid a wildcard at the same address when
  physical isolation is required.
- A board opts into `uses_spi_pins()` and implements `spi_transfer_pins()`. It
  selects its devices by testing the SCLK, MOSI, and active-low CS masks and the
  MISO input pin. Masks preserve mirrored outputs. SPI2 supports matrix routes
  and both native FSPI pin groups. Low enabled software GPIO selects also appear
  in the CS mask. The route is captured at command submission, including delayed
  DMA transactions. Existing boards keep `spi_transfer()` without route decoding.
- `gpio_output_at()` receives the bus cycle, changed levels, and the complete
  GPIO enable and output masks immediately after writes. A low pin's release is
  observable even if its level list is empty. Existing `gpio_changes()` remains
  the default. Board models schedule input through the existing `next_deadline`,
  `advance_to`, and `take_edges` methods. `BoardEdge::cycle` is preserved.

Scope is S3 I2C0, I2C1, SPI2, and GPIO. The existing SPI3 placeholder, C3/C6
transport wiring, multi-lane SPI interpretation, electrical contention, and
active-high attachment are outside this change. The GPIO masks describe register
drive state, not an electrical pad solver. Timing is the emulator's bus clock,
not calibrated hardware timing. Native scheduler rounds can quantize events by
up to 63 cycles; WASM uses the existing 256-cycle contract. The Arduino check ran
natively, not in a browser. There is no speed claim.

## Results

The focused test target covers routing, replacement, both I2C controllers, board
reset, legacy addressing, software and hardware CS, IO_MUX and matrix selection,
inverted MISO rejection, delayed DMA route capture, output timestamps, low-level
release, and board input timestamps. Six tests pass. The full workspace run passes 467 tests, with 23 ignored tests. The external Arduino test
is ignored by ordinary workspace runs and was run explicitly.

The Arduino 3.3.8 sketch uses unmodified Wire, SPI, and GPIO APIs, with no firmware
stubs. Its UART0 output is:

```text
I2C right=0
I2C wrong=2
SPI right=a5 wrong=ff
PULSE requested_us=100
TRANSPORT DONE
```

The host consumer measured GPIO4 high at cycle 27709171 and low at 27733171:
24000 cycles, exactly 100 microseconds at 240 MHz. The check accepts a difference
of at most one native scheduler round, 64 cycles. The run stops at 240000000
cycles after 7288738 instructions. The two SPI transfers also verify SCLK12,
MOSI11, and MISO13. Only GPIO10's CS returns device data; GPIO14 returns `ff`.
I2C answers at address `0x42` on GPIO8/9 and NACKs on GPIO6/7.

Builds use macOS 26.6.2 arm64, rustc 1.96.0, PlatformIO 6.1.19, pioarduino
55.3.38 at `fbdfc29`, Arduino 3.3.8, IDF libraries 5.5.4 at `735507283d`, and
Xtensa GCC 14.2.0+20260121. [results.json](results.json) records commands, outcomes,
artifact hashes, and the final source revision.

The first PlatformIO attempt failed with
`PermissionError: [Errno 1] Operation not permitted: '/Users/alice/.platformio/platforms.lock'`.
One retry with cache access succeeded. The first focused test compile used the
wrong public type path and failed with `error[E0425]: cannot find type Bus in crate esp32s3`.
Changing that test path to `esp32s3::bus::SocBus` fixed the compile. The local Git
commit initially hit the sandbox's worktree index lock restriction; one retry
with metadata access succeeded. These failures were tooling or test setup errors. The evidence scanner also
rejected punctuation after the normalized home label. A trailing slash resolved
that false positive in one retry.

## Reproduce

Set `ESP32SIM_ROM` to an S3 revision-0 ROM ELF, for example the ROM supplied by
PlatformIO's `tool-esp-rom-elfs` package. The ROM hash is in `results.json`.
Run these commands from the repository root:

```sh
transport_dir=$(mktemp -d /tmp/esp32sim-transport.XXXXXX)
mkdir -p "$transport_dir/src"
cp docs/evidence/s3-pin-transport-2026-10-02/platformio.ini "$transport_dir/"
cp docs/evidence/s3-pin-transport-2026-10-02/main.cpp "$transport_dir/src/"
pio run -d "$transport_dir"
ESP32SIM_TRANSPORT_BUILD="$transport_dir/.pio/build/s3" \
  cargo test --release -p esp32s3 --test pin_transport arduino_pin_transport -- --ignored --nocapture
cargo test -p esp32s3 --test pin_transport
cargo build --release
cargo test --workspace
tools/wasm-build.sh
node tools/check-evidence-privacy.mjs
git diff --check
```

Record firmware hashes before deleting `"$transport_dir/.pio"`. The measured
build folder was removed after recording its hashes. Only newly created Rust
files were formatted. No workspace or crate-wide formatting was run.

## Evidence privacy

This receipt retains the test source, sketch, configuration, numeric samples,
commands, and SHA-256 hashes. Full build logs and redundant ROM boot output are
omitted. Home paths in the failure description use the normalized label
`/Users/alice`; commands use caller-supplied paths. No process inventories,
hostnames, or private captures are included. These omissions do not alter the
checks or measured values. Firmware and ROM binaries are identified by hash and
are not committed; rebuilding may produce different firmware hashes.
