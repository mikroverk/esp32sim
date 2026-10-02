# C3 peripheral and board validation

EX205, implementation `ffbb5158c7c45c19369863f7f0cb9fed622cc4a7`, based on
`717f02633ddc1447d4fc82a9c30eafcfdfed2f3f` from PR #169. The behavior reference
was `221080ffccfa829106b398f896653535853c76c8`, inspected with `git show` and
`git diff dddb128 221080f -- esp32c3/src/bus.rs esp32c3/src/periph.rs`.
The fork's SPI route decoder and compact RMT map were also inspected.
There is no dependency on the classic ESP32 work or the fork.

Related EX201 establishes the S3 attachment and GPIO callback contract. EX205
adds a different chip, C3 register routes and interrupts, and Arduino RMT
workloads. EX047 covers scheduler granularity and EX056 covers board timing.
No execution optimization, browser benchmark, scheduler change, or hardware
calibration was attempted.

## Design and scope

C3 now mounts the shared I2C master at `0x60013000`, GP-SPI2 at `0x60024000`,
and compact RMT at `0x60016000`, with interrupt sources 29, 19, and 28.
The existing C6 RMT register adapter moved unchanged into `esp-periph` as
`rmt_compact::RmtCompact`; `esp32c6::periph::RmtC6` remains a public alias.

An external board implements the existing `BoardModel` and `I2cDevice` traits.
Assign `bus.board`, then call `bus.attach_board_devices()`. Controller 0 is the
C3 digital I2C controller; other controller numbers are ignored. Reboot recreates
peripherals and reattaches the persistent board. I2C pin matching checks matrix
input and output selection plus IO_MUX input enable. SPI callbacks expose C3
matrix routes, the native FSPI pins, mirrored outputs, and asserted active-low
hardware or software chip selects. Unpinned I2C devices and legacy board
callbacks retain their existing behavior.

GPIO drive changes call `gpio_output_at` immediately, including releasing a low
output. Board deadlines participate in the existing scheduler; returned input
edges retain their original cycle. RMT completions are routed by channel signal
to `rmt_frame`, where the existing `Ws2812Chain` observes the colours.

SPI support here is CPU FIFO transfers. C3 GDMA register layout and SPI DMA
pumping remain unimplemented; DMA submissions cannot complete through this bus.
RMT RX, peripheral clock/reset gate emulation, electrical contention, inverted
routes, and active-high SPI attachment are also outside the tested contract.
RMT routing retains the existing first-matching-pin observer convention.
No product ABI, record format, device catalogue, or new dependency was added.

## Results

- `cargo build --release`: passed.
- `cargo test --workspace`: 473 passed, 24 ignored, zero failures.
- `tools/wasm-build.sh`: passed. The WASM artifact was built, not browser-tested.
- Six focused C3 tests passed. They cover I2C pins and replacement, fixed
  addressing, SPI native/matrix routes and selects, GPIO output/release and input
  timestamps, legacy callbacks, reset reattachment, and both RMT TX channels,
  decoded colours, deadlines and C3 interrupts.
- The normally ignored Arduino test passed twice. The second run also explicitly
  checked transmitted SPI bytes `a5` and `5a` at the host consumer.
- `node tools/check-evidence-privacy.mjs` and `git diff --check`: passed.

Both Arduino runs used the same firmware, built from [main.cpp](main.cpp) and
[platformio.ini](platformio.ini). Wire, SPI, `rgbLedWrite`, `delayMicroseconds`,
and Adafruit NeoPixel were unmodified. UART0 output in each run:

```text
I2C right=0
I2C wrong=2
SPI right=a5 wrong=ff
PULSE requested_us=100
RGB DONE
NEOPIXEL DONE
TRANSPORT DONE
```

I2C address `0x42` ACKs on GPIO8/9 and NACKs on GPIO6/7. SPI uses SCLK6, MOSI7,
MISO2; CS10 returns `a5`, and CS3 returns `ff`. `rgbLedWrite` on GPIO5 produces
RGB `[18,52,86]`. Adafruit NeoPixel on GPIO1 produces `[171,205,239]` followed by
`[33,67,101]`. The test decodes both transmissions with `Ws2812Chain` and checks
the GPIO identity and every colour byte.

The GPIO4 consumer records high at cycle 19236608 and low at 19252608. The
16000-cycle difference equals the requested 100 microseconds at 160 MHz.
Acceptance allows a difference of one native scheduler round, 64 cycles.
Each run stops at 160000000 cycles and reports 160000000 instructions. These
are emulator bus timestamps and work counters, not measurements on hardware.
Two identical runs establish repeatability for this fixture, not a timing
accuracy bound for other programs. No wall-clock performance claim is made.

The host was macOS 26.6.2 arm64 with rustc 1.96.0 and PlatformIO 6.1.19.
Firmware used pioarduino 55.3.38 at `fbdfc29`, Arduino 3.3.8, IDF libraries
5.5.4 at `735507283d`, RISC-V GCC 14.2.0+20260121, and Adafruit NeoPixel 1.15.2.
[results.json](results.json) preserves inputs, artifact sizes and SHA-256 hashes,
commands, work counts, numeric samples, and failures.

The first PlatformIO attempt could not write the package-cache lock. One retry
with cache access succeeded. A new RMT test initially expected a deadline of
exactly 32 cycles; the shared clock conversion conservatively returned 31. The
test now accepts at most 32 cycles and advances past the last symbol to consume
its end marker; its single retry passed. The first local commit could not lock
this worktree's Git index; one retry with metadata access succeeded. Exact errors
are retained in the JSON receipt with personal home paths normalized.

## Reproduce

Supply `ESP32SIM_ROM` with a C3 revision-3 ROM ELF. PlatformIO's
`tool-esp-rom-elfs` package provides `esp32c3_rev3_rom.elf`; its measured hash is
in the receipt. From the repository root:

```sh
firmware_dir=$(mktemp -d /tmp/esp32sim-c3periph.XXXXXX)
mkdir -p "$firmware_dir/src"
cp docs/evidence/c3-peripherals-2026-10-02/platformio.ini "$firmware_dir/"
cp docs/evidence/c3-peripherals-2026-10-02/main.cpp "$firmware_dir/src/"
pio run -d "$firmware_dir"
ESP32SIM_TRANSPORT_BUILD="$firmware_dir/.pio/build/c3" \
  cargo test --release -p esp32c3 --test pin_transport arduino_pin_transport -- --ignored --nocapture
cargo test -p esp32c3 --test pin_transport
cargo build --release
cargo test --workspace
tools/wasm-build.sh
node tools/check-evidence-privacy.mjs
git diff --check
```

The host test loads `firmware.factory.bin` at flash offset zero and boots the mask
ROM. The CLI equivalent uses `--chip c3 --boot rom --flash-image` and `--rom`,
but attachment assertions require the test's board implementation. Firmware
hashes were recorded before deleting the temporary `.pio` directory. Rebuilt
firmware may have different hashes due to build paths or timestamps.

Only new Rust files were formatted with `rustfmt --edition 2021`; no existing
file, crate, or workspace was run through rustfmt. Git status was checked after
formatting. All commits are local. No GitHub PR, comment, or native stack was
created; the draft names PR #169 as a dependency.

## Evidence privacy

This is a curated summary, not a copy of raw logs. Redundant ROM boot output,
local compiler paths, and build progress were omitted. The one personal home
path in an error uses `/Users/alice/`. Commands accept caller-supplied ROM and
build paths. No host identity, process inventory, device identifier, or private
capture is retained. Omissions do not change measured values or input hashes.
No previously published receipt was sanitized or replaced.
