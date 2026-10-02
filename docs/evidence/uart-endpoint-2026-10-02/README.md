# UART endpoint correctness, 2026-10-02 (EX203)

Three-chip acceptance passes after the follow-up below. The original, unchanged
Arduino firmware builds pass on S3, C3 and C6, including exact echoes, wrong-pin
silence, baud mismatch detection and firmware resets. Six focused tests pass.
The initial failures and their measured artifacts are retained below.

Base: `dddb128052dca15250e2169b92ab73c4d87f524c`, local branch `up-uart`.
Behavior reference: `221080ffccfa829106b398f896653535853c76c8`, inspected with
`git diff dddb128 221080f -- esp-soc esp32s3 esp32c3 esp32c6` and `git show` of
its UART, SoC and machine code. No fork device models, ABI or record formats were
copied. No dependency on PR #168. Initial source hashes are in `source-hashes.json` at `9142a78a16745985aa188149c07efb1da0ed7c26`.
Final source hashes are in `followup-source-hashes.json`; the containing commit
identifies that source revision.

The catalog was searched for UART, serial, matrix and IO_MUX. No previous UART
endpoint experiment was present. EX075 measured boot/TE timing. EX203 instead
checks byte delivery, routes, baud matching and reset persistence; it measures
neither execution speed nor electrical UART timing.

## Reproduce

Create a temporary PlatformIO project outside the repository. Copy `platformio.ini`
there and `firmware.cpp` to its `src/main.cpp`. Run:

```sh
pio run -d /tmp/esp32sim-uart-firmware
cargo build --release
cargo test --workspace
cargo test -p esp32sim --test uart_endpoint
cargo build --release --example uart_echo
# ROM_DIR is the caller's installed tool-esp-rom-elfs directory.
target/release/examples/uart_echo s3 /tmp/esp32sim-uart-firmware/.pio/build/s3 "$ROM_DIR/esp32s3_rev0_rom.elf"
target/release/examples/uart_echo c3 /tmp/esp32sim-uart-firmware/.pio/build/c3 "$ROM_DIR/esp32c3_rev3_rom.elf"
target/release/examples/uart_echo c6 /tmp/esp32sim-uart-firmware/.pio/build/c6 "$ROM_DIR/esp32c6_rev0_rom.elf"
tools/wasm-build.sh
node tools/check-evidence-privacy.mjs
git diff --check
```

The runner uses the same ROM, bootloader, partition table and app loading as the
CLI's `--boot rom --bootloader ... --ptable ... --app ...`; see `docs/cli.md`.
It uses `Machine::run` to schedule every core, stopping after two firmware reset
requests with a ten-second guest-time limit. It checks UART0 console reports
separately from the UART1 device stream. It installs a board model without changing the sketch,
Arduino core or SDK. Each boot performs two ten-byte `UART-PING\n` exchanges at
9600 baud on RX GPIO4 / TX GPIO5, with a one-byte 19200-baud mismatch between them.
The other endpoint is on GPIO7. Success requires four exact echo reports, two
`UART PASS reset` lines, 40 device bytes, two detected mismatches, zero bytes on
the other pins, and two firmware resets.

All three PlatformIO builds succeeded. Arduino-ESP32 3.3.8, libraries
`5.5.4+sha.735507283d`, pioarduino platform tag `55.03.38-1`, Xtensa/RISC-V
GCC `14.2.0+20260121`. The platform URL and board names are in `platformio.ini`.
Host: macOS 26.6.2, arm64; rustc 1.96.0 (`ac68faa20`, 2026-05-25), cargo 1.96.0.
No hardware timing or browser-speed comparison was performed.

`inputs.json` records each firmware/ROM size and SHA-256. Firmware binaries and
ELFs were retained under ignored `target/uart-evidence/firmware/`. PlatformIO's
`.pio` directory was deleted after recording the evidence. The sources and
commands suffice to rebuild; retained binaries are local, not published artifacts.

## Initial runs and corrections

| Run | S3 | C3 | C6 |
| --- | --- | --- | --- |
| Initial endpoint | Two bootloader watchdog resets; zero device bytes | Four echo timeouts; 40 device bytes, 2 mismatches, 0 wrong-pin bytes | Two echo passes and two timeouts; 40 device bytes, 2 mismatches, 0 wrong-pin bytes |
| After RX routing correction | Same bootloader watchdog failure | Same four echo timeouts | Four echo passes across two firmware resets; 40 device bytes, 2 mismatches, 0 wrong-pin bytes |

The initial code required the RX pin's IO_MUX output function to select GPIO.
IDF `components/esp_driver_uart/src/uart.c:uart_set_pin` instead enables the input
buffer and connects the input matrix without changing that output function.
Removing this requirement fixed C6. The same review corrected native UART TX to
use peripheral output enable rather than GPIO_ENABLE, following
`components/hal/gpio_hal.c:gpio_hal_iomux_out`.

C3's last initial command exited 101: `assertion left == right failed`, left `0`, right
`4`, for the number of successful echo reports. The model observed all 40 guest
bytes and two baud mismatches, but firmware reported `UART echo=FAIL bytes=0`
four times. S3's last initial command also exited 101 at that assertion; ROM serial
reported `rst:0x10 (RTCWDT_RTC_RST)` after `entry 0x403c88b8`. There were no sketch
UART reports or endpoint bytes. `serial.txt` preserves the relevant output.
At this stage neither failure had been established to predate the patch. The
initial task stopped further retries under its one-fix-attempt limit. The user
then explicitly requested the follow-up below.

A debug-build diagnostic reproduced the C3/C6 symptoms and RX routes with
`rx_pin: None`. The S3 debug diagnostic eventually reproduced the boot failure.
An attempted early termination could not query processes in the sandbox; a later
restricted termination found that the diagnostic had already exited. No process
inventory was retained.

The Arduino executable hash in `results.json` identifies the measured candidate.
After those initial runs, source replaced equivalent chip pin checks with valid-pin
masks, ignored empty injected batches, and connected S3 UART2's missing interrupt
source 29. The Arduino runs were not repeated on that final source. Validation at that stage included five focused tests and the workspace/build checks. The
reconstructed `arduino-candidate.patch` retains the implementation and runner used
for the measured runs, relative to the base revision; it excludes unrelated tests
and documentation. Apply it to the base with `git apply --unidiff-zero`.

The first PlatformIO command failed with
`PermissionError: [Errno 1] Operation not permitted: '/Users/alice/.platformio/platforms.lock'`.
The authorized cache-access retry built all three environments. The first focused
test compile reported E0061 because the C3/C6 constructors require flash size;
that was corrected. The next run passed C3/C6 but S3 returned `Err(Prohibited)`
for an 8-bit peripheral store. The check now uses the supported 32-bit store.
The workspace run then passed all three chip cases. Final focused coverage adds
fractional source/divider arithmetic and UART2 RX interrupt routing.

## Follow-up: all three chips pass

The follow-up started from `9142a78a16745985aa188149c07efb1da0ed7c26` and keeps
EX203. Its stronger evidence adds a clean upstream baseline and uses the correct
multicore execution API. No firmware, pin, board or SDK changes were needed.
All 15 retained firmware and ROM artifacts still match `inputs.json` exactly.

A clean temporary clone at `dddb128052dca15250e2169b92ab73c4d87f524c` built with
`cargo build --release -p esp32sim`. Its working tree was clean before and after
the run. The exact retained S3 build reached the sketch, printed two expected
`UART echo=FAIL bytes=0` lines without an attached echo endpoint, then requested
software reset cause `0xc` at guest time 2.053 s. It did not hit the bootloader
watchdog. The baseline command was:

```sh
/tmp/esp32sim-uart-base/target/release/esp32sim --chip s3 --boot rom --board none --mac 00:00:00:00:00:00 --rom "$ROM_DIR/esp32s3_rev0_rom.elf" --bootloader target/uart-evidence/firmware/s3/bootloader.bin --ptable target/uart-evidence/firmware/s3/partitions.bin --app target/uart-evidence/firmware/s3/firmware.bin --elf target/uart-evidence/firmware/s3/firmware.elf --max-seconds 5 --no-reboot --console uart0
```

The S3 failure was in the acceptance runner. It called `run_until_cycle`, whose
contract explicitly schedules only core 0. The dual-core Arduino startup could
not complete while core 1 never ran. The example now calls ordinary `Machine::run`
with `max_cycles` as its limit. No S3 emulator or bootloader change was needed.

The C3 diagnostic observed GPIO matrix input value `0x44`, IO_MUX value `0x200`,
and baud 9600, but decoded `rx_pin: None`. The endpoint discarded the reply before
it entered the FIFO. C3 input selection is bit 6, inversion is bit 5, and GPIO is
bits 4:0. S3/C6 use bits 7, 6 and 5:0. The C3 output fields are narrower too: signal
bits 7:0, inversion bit 8, output-enable selection bit 9. `UartPins` now stores
chip-specific masks. The regression check uses the firmware's `0x44` encoding and
covers both RX and TX inversion. These definitions were checked against IDF's
`components/soc/esp32c3/register/soc/gpio_reg.h`. No FIFO or interrupt workaround
was added.

All final commands used the same original firmware artifacts:

```sh
target/release/examples/uart_echo s3 target/uart-evidence/firmware/s3 "$ROM_DIR/esp32s3_rev0_rom.elf"
target/release/examples/uart_echo c3 target/uart-evidence/firmware/c3 "$ROM_DIR/esp32c3_rev3_rom.elf"
target/release/examples/uart_echo c6 target/uart-evidence/firmware/c6 "$ROM_DIR/esp32c6_rev0_rom.elf"
```

Each exited 0 with four exact ten-byte echoes, two `UART PASS reset` reports,
40 device bytes, two detected baud mismatches, zero bytes on the other pins, and
two firmware reset requests. `followup-serial.txt` preserves the output plus the
baseline and one diagnostic sample. Final-source release, workspace, focused-test,
WASM, privacy and diff checks pass; totals and executable hashes are in the
`followup` record of `results.json`. All original negative records remain there.
The source diff is limited to the route masks, the example runner and tests.

One early rerun launched before the release link finished and therefore used the
old diagnostic executable. Its results were excluded from final acceptance; the
final runs waited for a successful build and used the recorded executable hash.
No new PlatformIO build directory was created.

## Limits and evidence privacy

The interface delivers completed bytes; it does not model wire timing, parity,
flow control, inverted signals or clock gating. Three percent baud tolerance is
an emulator contract, not a measured oscillator limit. Host console bytes remain
independent of board delivery. Reset tests verify board state persistence and
firmware reconfiguration, not device power cycling.

Committed evidence contains source, hashes, commands, test totals, selected serial
lines and exact relevant errors. Raw compiler/ROM logs, tool cache paths, process
IDs and redundant boot banners were omitted. The one home path in the failure
above was normalized. These omissions do not change numeric results. Binary
build metadata is kept only in ignored local artifacts. Privacy pattern checks
supplemented manual inspection of every new evidence file.
