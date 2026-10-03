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

Supply `ESP32SIM_ROM_DIR` containing `esp32c3_rev3_rom.elf`. PlatformIO's
`tool-esp-rom-elfs` package provides `esp32c3_rev3_rom.elf`; its measured hash is
in the receipt. From the repository root:

```sh
firmware_dir=$(mktemp -d /tmp/esp32sim-c3periph.XXXXXX)
mkdir -p "$firmware_dir/src"
cp docs/evidence/c3-peripherals-2026-10-02/platformio.ini "$firmware_dir/"
cp docs/evidence/c3-peripherals-2026-10-02/main.cpp "$firmware_dir/src/"
pio run -d "$firmware_dir"
ESP32SIM_TRANSPORT_BUILD="$firmware_dir/.pio/build/c3" \
  cargo +1.99.0 test --release -p esp32c3 --test pin_transport external_c3_arduino_pin_transport -- --nocapture
cargo +1.99.0 test -p esp32c3 --test pin_transport -- --skip external_
cargo +1.99.0 build --release
cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh
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

## Review revision (EX205)

The C3 now skips idle RMT ticking, SPI completion polling, board clock/edge callbacks and disabled pin interrupt sources. Board GPIO capability is cached at attachment; RMT tracks active channels in a mask. SPI transfers are delivered on submission. The reset selector, retained GPIO buffer and idle board path have regression tests, including interrupt enable/clear/completion transitions.

[review.json](review.json) records the full check results. [cpu-rmt-final.json](cpu-rmt-final.json) records the adopted native CPU comparison, all samples, revisions, binary/input hashes, command, correctness checks and limitations. The other `cpu-*.json` files retain earlier and rejected measurements; the two `rejected-*.patch` files preserve the later unadopted candidates. This revises EX205's measurement quality and idle-path mechanism; it does not introduce a new experiment.

Three c3-hello runs, 30 emulated seconds each, user plus system CPU seconds:

| Build | CPU seconds | Median |
|---|---|---|
| main | 2.606990, 2.624362, 2.614370 | 2.614370 |
| Published C3 | 3.080239, 3.079602, 3.059772 | 3.079602 |
| Corrected C3 | 2.753298, 2.773484, 2.772790 | 2.772790 |
| Published UART | 2.708919, 2.738410, 2.696680 | 2.708919 |
| Corrected UART on C3 stack | 2.803772, 2.748303, 2.773400 | 2.773400 |

Every run executed 4,800,000,000 instructions/cycles with zero exceptions and identical console SHA-256. C3 improves 10.0% against its published branch but remains 6.1% above main. UART adds 0.02% to the corrected C3 median. These are local macOS arm64 CPU samples under uncontrolled background load, not browser or hardware timing. No process inventory was retained.

Reproduce with `python3 docs/evidence/c3-peripherals-2026-10-02/bench-idle.py FW main=MAIN_BINARY published-c3=PUBLISHED_C3_BINARY fixed-c3=FIXED_C3_BINARY published-uart=PUBLISHED_UART_BINARY fixed-uart=FIXED_UART_BINARY`. The harness writes logs and the JSON receipt under `/tmp`; supply firmware and binaries explicitly.

## Quiet-machine follow-up (EX205)

This repeats the c3-hello workload with seven interleaved rounds after a warmup, separate build targets and one-minute load below 3 before and after each sample. It supersedes the earlier uncontrolled-load speed conclusion while retaining those samples and rejected patches above.

The profile found interrupt refresh in 4,080 of 7,565 C3 samples. `Bus::tick` returned 1 unconditionally even though the dispatcher already reports IRQ changes. The correction propagates that result, includes RMT transitions, and keeps MMIO/board dirty notifications. SPI commands already finish at MMIO writes, so their scheduler poll is removed. Inactive new interrupt sources are no longer scanned every round. UART shares the existing board-activity check, and its RX loop stays out of line: generated idle-tick stack usage returns from 336 to 240 bytes, matching C3.

| Arm | CPU seconds, user + system | Median seconds |
|---|---|---|
| main | 2.441188, 2.439410, 2.489314, 2.400678, 2.410777, 2.447775, 2.454372 | 2.441188 |
| published-c3 | 2.926025, 2.814050, 2.932286, 2.909935, 2.951779, 2.918322, 2.950065 | 2.926025 |
| fixed-c3 | 1.172755, 1.157276, 1.164665, 1.167727, 1.184475, 1.217162, 1.224255 | 1.172755 |
| published-uart | 2.588128, 2.605506, 2.509926, 2.533853, 2.599415, 2.605478, 2.568803 | 2.588128 |
| fixed-uart | 1.187673, 1.207320, 1.160124, 1.174879, 1.167878, 1.137358, 1.174847 | 1.174847 |

All samples execute 4,800,000,000 instructions/cycles, with zero exceptions and identical console hashes. [quiet-cpu.json](quiet-cpu.json) retains the samples, loads, build/source and firmware hashes, commands and limitations. [quiet-profile.json](quiet-profile.json) keeps aggregate simulator samples only; process IDs, paths, addresses and loaded-image/session metadata were omitted. The raw profile hashes remain for provenance. The earlier UART candidate comparison and the rejected load-gate attempt are retained separately.

[quiet-review.json](quiet-review.json) records the full Rust 1.99.0 and eight-demo checks. Goldens are unchanged. [quiet-mutations.json](quiet-mutations.json) records failures after restoring unconditional refresh, dropping RMT change notification, or excluding UART-only boards from activity; all ten C3 and eight UART control tests pass with source restored. Run `python3 docs/evidence/c3-peripherals-2026-10-02/check-idle-mutations.py` from an idle clean top-of-stack worktree to repeat.

Reproduce timing with `python3 docs/evidence/c3-peripherals-2026-10-02/bench-quiet.py FW main=MAIN_BINARY published-c3=PUBLISHED_C3_BINARY fixed-c3=FIXED_C3_BINARY published-uart=PUBLISHED_UART_BINARY fixed-uart=FIXED_UART_BINARY`. Build each binary in its own target directory. The script polls load before starting each sample and rejects a batch if load reaches 3 during a sample.

Corrected C3 is 51.96% faster than main. UART adds 0.18% over corrected C3, with overlapping ranges. Neither corrected branch overlaps main because both are substantially faster; this exceeds the non-regression bound rather than claiming statistical equality with main.
