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

## Scoped idle participation revision

This continues EX205 with the original scheduler contract. C3 tick always returns
1, and existing interrupt sources are rescanned every scheduler round. The added
RMT and board services participate only while configured. Pin interrupt enable
writes select the added-source path. The three new controllers are boxed after
the existing hot fields; C3-only inlining preserves the baseline fixed-device
clock path. SPI2 delivery remains submission-driven. No baseline interrupt cache
is introduced.

`pin_clock_participation_stops_when_rmt_finishes` checks active and idle transitions
and the unchanged tick return. `pin_interrupt_participation_tracks_enable_registers`
checks all three added controllers. The transport tests check mapped CPU lines
as well as source bits. All three targeted mutations are killed; see
[scope-mutations.json](scope-mutations.json).

[scope-pilots.json](scope-pilots.json) retains the exploratory samples and rejected
variants. [scope-patches.json.gz](scope-patches.json.gz) preserves their patches
against the stated base. The ablation is a diagnostic with pin behavior disabled,
not a correctness candidate. The boxed trial overlapped a later build and is
invalid for acceptance. The final comparison uses separate build directories
and frozen binaries with hashes checked before and after timing. Two earlier
final batches were rejected when one-minute load reached 3; their partial samples
are excluded. A harness warmup failed before measured rounds because its round
argument shadowed the rounding function; the harness was corrected and restarted.

The final harness is adapted from the supplied quiet benchmark. Run
`python3 docs/evidence/c3-peripherals-2026-10-02/scope-bench.py FW main=MAIN_BINARY published-c3=PUBLISHED_C3_BINARY c3=C3_BINARY published-uart=PUBLISHED_UART_BINARY uart=UART_BINARY`.
Build each arm with `CARGO_TARGET_DIR=ARM_TARGET cargo +1.99.0 build --release -p esp32sim --bin esp32sim`
in its own checkout. It waits for `uptime` one-minute load below 3 before each
subprocess and rejects a batch if load reaches 3 during a sample. Outputs stay
in `/tmp`. Raw profiler process metadata is not retained; curated counts omit
identities, paths and loaded-image inventories. Numeric samples and artifact
hashes are unchanged by this omission.

The first complete five-arm run missed the median threshold: main 2.405076 s, C3
2.420847 s, UART 2.426573 s. Both ranges overlapped main, but +0.66% and +0.89%
exceeded +0.5%. [scope-before.json](scope-before.json) retains this result.

Final two-arm comparison: three seven-round batches with identical main and C3
binary hashes. Pooled CPU medians are main 2.411907 s and C3
2.418538 s, +0.27%. Ranges are 2.377721 to
2.446411 s and 2.389145 to 2.430564 s. Each run executes
4,800,000,000 instructions/cycles with zero exceptions, 3028 interrupts and the
same console hash. Individual batch deltas were -0.46%, +0.56% and +0.98%; the
pooled result meets the threshold, with visible variation between batches.

The original published head `e6daec5` measured 2.844415 s against main 2.405076 s
in the earlier seven-round five-arm reference. Final acceptance uses two arms,
as requested. [scope-cpu.json](scope-cpu.json) retains all matching two-arm
batches, source/input/binary hashes, load readings and limits. [scope-before.json](scope-before.json)
retains the five-arm reference. The cold-deadline, source-bit-cache and enable-word
trials were rejected; their receipts and patches remain alongside the earlier
pilots. The optional-callback trial was rejected after disassembly because it
still saved a frame and added an indirect call. None of these later trials is
in the adopted code.

[scope-review.json](scope-review.json) records both Clippy checks, 497 passing
workspace tests and all eight WASM demos. Goldens are unchanged.
[scope-profile.json](scope-profile.json) contains curated profile and assembly
observations; [scope-exclusions.json](scope-exclusions.json) records rejected
load batches. Raw captures and full build logs stay outside Git.
