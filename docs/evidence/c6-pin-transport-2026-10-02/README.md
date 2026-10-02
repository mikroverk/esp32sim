# C6 digital I2C and timed GPIO

EX206 implementation `92a92a29b1bda7cfa06a27e58d8fc2119b16b615`, on PR #169's
`717f02633ddc1447d4fc82a9c30eafcfdfed2f3f`. Behavior reference
`221080ffccfa829106b398f896653535853c76c8` was inspected with
`git diff dddb128 221080f -- esp32c6/src/bus.rs esp32c6/src/periph.rs`.
No fork device models, product ABI, or classic ESP32 changes are dependencies.

EX201 established the S3 pin attachment and output timestamp API. EX206 differs
by adding C6's digital controller, C6 matrix decoding, and board input scheduling,
and by checking Arduino `attachInterrupt` and `pulseIn` against an echo produced
by a timestamped output consumer. Related EX047 defines scheduler granularity and
EX056 covers board input timing. This is a correctness workload, not a speed test.

## Contract

The shared `esp_periph::i2c::I2c` occupies C6 address `0x60004000`, interrupt
source 50. `BoardModel::i2c_devices` attaches bus 0 devices through
`SocBus::attach_board_devices`. Other bus numbers are ignored because C6 has one
digital controller. CLI, network, WASM setup and SoC reboot call the attachment
method. Custom Rust boards call it after assigning `bus.board`, as on S3.

Pinned devices match SDA/SCL matrix signals 46/45 at transaction start. Both input
and output routes must agree, the IO_MUX must select GPIO with input enabled,
and neither input, output nor output-enable may be inverted. GPIO-selected output
enable must be asserted. C6 has GPIO0 through GPIO30 and uses output-enable
selection bit 9, unlike S3's bit 10. The installed Arduino IDF headers
`soc/esp32c6/include/soc/gpio_sig_map.h`, `soc_caps.h`, and
`soc/esp32c6/register/soc/gpio_struct.h` were checked against the fork reference.
Unpinned devices keep controller-only addressing. Same-address devices on
different pin pairs coexist; same-address/same-pair attachment replaces a device.

GPIO output and enable changes call `gpio_output_at` at the current bus cycle,
including release of a low pin when there is no level edge. The default callback
still calls `gpio_changes` for legacy boards. Board deadlines bound time skips;
`advance_to` and `take_edges` update GPIO input and IRQ state while retaining each
edge's original timestamp for observers. Reboot restores persistent board inputs
and reattaches fresh controller devices. C6 has no consumer of the shared GPIO
input-change queue, so each tick discards its previous entries.

The shared controller completes command lists immediately. This change does not
model I2C wire duration, clock stretching, electrical contention, LP I2C, or PCR
peripheral-only reset. It does not change C6 SPI, C3, CPU execution, or scheduling.
Timestamps use the existing scheduler bus horizon: native ordinary execution can
quantize events by a 64-cycle round; WASM retains its existing quantum. No browser
execution or hardware calibration is claimed.

## Checks and Arduino result

Three focused tests cover physical routing, equal-address isolation, replacement,
legacy addressing, invalid pins, inversion, output-enable selection, I2C interrupt
assertion/clearing, reset reattachment, timestamped output-enable changes, timed
input and GPIO interrupts. The external Arduino test is ignored in ordinary test
runs and was run explicitly. The full workspace passes 470 tests with 24 ignored.
Release and WASM builds, the evidence privacy scanner and `git diff --check` pass.
See `results.json` for outcomes and hashes.

The unchanged Arduino-ESP32 3.3.8 APIs in [main.cpp](main.cpp) run from a complete
PlatformIO ROM-booted factory image, without firmware stubs. UART0 output:

```text
I2C right=0
I2C count=1 value=a5
I2C wrong=2
ECHO width_us=999 edges=2
C6 TRANSPORT DONE
```

At 160 MHz, the requested 100 us GPIO4 pulse rises at cycle 3381184 and falls at
3397376: 16192 cycles, or 101.2 us. The board schedules GPIO5 high 32000 cycles
after the falling output, at 3429376, and low 160000 cycles later, at 3589376.
Arduino measures 999 us for this 1000 us echo and counts both CHANGE interrupts.
The run stops at 160000000 cycles and 160000000 instructions. Both native Arduino
runs produced these same samples and serial results.

The initial output assertion allowed only 64 cycles around 16000 and failed with
`assertion failed: (low - high).abs_diff(16000) <= 64`. That bound came from S3's
fixture and omitted C6 Arduino call/poll overhead. Inspection of Arduino
`esp32-hal-misc.c::delayMicroseconds` confirmed that it polls a microsecond timer.
One test-only correction allows 2 us plus one native round, 384 cycles; the
implementation and firmware were unchanged. The echo assertion allows 2 us.
These bounds check emulator behavior and do not establish hardware accuracy.

The first PlatformIO build failed with
`PermissionError: [Errno 1] Operation not permitted: '/Users/alice/.platformio/platforms.lock'`.
One retry with package-cache access passed. The local commit initially failed to
create the worktree `index.lock`; one metadata-access retry passed. An attempt to
read a Git revision from the installed default PlatformIO package failed with
`fatal: not a git repository (or any of the parent directories): .git`.
The actual build reports platform `55.3.38+sha.fbdfc29`; that build metadata is
retained instead. No commands pushed or contacted GitHub to change repository state.

## Reproduce

Provide `ESP32SIM_ROM` as the C6 revision-0 ROM ELF, available in PlatformIO's
`tool-esp-rom-elfs` package. Commands run from the repository root:

```sh
c6_dir=$(mktemp -d /tmp/esp32sim-c6periph.XXXXXX)
mkdir -p "$c6_dir/src"
cp docs/evidence/c6-pin-transport-2026-10-02/platformio.ini "$c6_dir/"
cp docs/evidence/c6-pin-transport-2026-10-02/main.cpp "$c6_dir/src/"
pio run -d "$c6_dir"
ESP32SIM_TRANSPORT_BUILD="$c6_dir/.pio/build/c6" \
  cargo test --release -p esp32c6 --test pin_transport arduino_pin_transport -- --ignored --nocapture
cargo test -p esp32c6 --test pin_transport
cargo build --release
cargo test --workspace
tools/wasm-build.sh
node tools/check-evidence-privacy.mjs
git diff --check
```

Record hashes before removing the temporary `.pio` directory. The measured
folder was deleted after recording its evidence. Only the new Rust test file was
formatted, with `rustfmt --edition 2021 esp32c6/tests/pin_transport.rs`; status
review confirmed no unrelated files changed.

## Evidence privacy

This receipt retains revisions, commands, tool versions, hashes, numeric samples,
serial checks and failures. Personal paths are normalized; raw build logs and
redundant ROM startup output are omitted. No process inventories or private
captures are retained. These omissions do not change any measured value. ROM
and firmware binaries are identified by SHA-256 and are not committed. Rebuilding
can change firmware hashes. The pattern scanner supplements manual review.
