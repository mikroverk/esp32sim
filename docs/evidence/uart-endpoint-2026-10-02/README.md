# UART endpoint correctness (EX203)

The original Arduino-ESP32 3.3.8 firmware passed on S3, C3 and C6 after the
routing and acceptance-runner corrections below. Each chip produced four exact
ten-byte echoes, 40 device bytes, two baud mismatches, zero wrong-pin bytes and
two firmware resets. These are historical results; the review checks are recorded
in `review.json`. No electrical timing claim is made.

Base revision: `dddb128052dca15250e2169b92ab73c4d87f524c`.
`source-hashes.json` identifies the initial source and `followup-source-hashes.json`
the corrected source. `inputs.json` identifies all 15 firmware and ROM artifacts.
`results.json` retains numeric results, executable hashes and commands, including
negative results. `serial.txt` and `followup-serial.txt` retain the relevant output.

## Reproduce

Copy `platformio.ini` and `firmware.cpp` to a temporary PlatformIO project, with
the latter at `src/main.cpp`. Supply `ESP32SIM_ROM_DIR` containing the three ROMs.

```sh
pio run -d /tmp/esp32sim-uart-firmware
cargo +1.99.0 test -p esp32sim --test uart_endpoint
cargo +1.99.0 build --release -p esp32sim --example uart_echo
target/release/examples/uart_echo s3 /tmp/esp32sim-uart-firmware/.pio/build/s3 "$ESP32SIM_ROM_DIR/esp32s3_rev0_rom.elf"
target/release/examples/uart_echo c3 /tmp/esp32sim-uart-firmware/.pio/build/c3 "$ESP32SIM_ROM_DIR/esp32c3_rev3_rom.elf"
target/release/examples/uart_echo c6 /tmp/esp32sim-uart-firmware/.pio/build/c6 "$ESP32SIM_ROM_DIR/esp32c6_rev0_rom.elf"
```

The example loads the ROM, bootloader, partitions and application, and runs all
cores with `Machine::run`, stopping after two resets or ten guest seconds. UART0
console checks are separate from the UART1 endpoint. Each boot exchanges two
`UART-PING\n` messages at 9600 baud on RX GPIO4 / TX GPIO5, separated by a
19200-baud mismatch. The endpoint also checks that GPIO7 receives nothing.

Historical builds used pioarduino `55.03.38-1`, Arduino 3.3.8, IDF libraries
`5.5.4+sha.735507283d`, GCC `14.2.0+20260121`, macOS 26.6.2 arm64 and Rust 1.96.0.
Rebuilding may change artifact hashes because of paths and timestamps.

## Negative results and corrections

| Run | S3 | C3 | C6 |
| --- | --- | --- | --- |
| Initial endpoint | Two bootloader watchdog resets; zero device bytes | Four echo timeouts; 40 bytes, 2 mismatches, 0 wrong-pin bytes | Two echo passes and two timeouts; 40 bytes, 2 mismatches, 0 wrong-pin bytes |
| RX output-function restriction removed | Same bootloader failure | Same four timeouts | Four echo passes; 40 bytes, 2 mismatches, 0 wrong-pin bytes, 2 resets |
| Multicore runner and C3 matrix masks corrected | Four echo passes | Four echo passes | Four echo passes |

UART matrix input selection is independent of the pin's output function; FUN_IE
must still be set. Requiring the GPIO output function caused the initial C6
failure. C3 uses input-select bit 6, inversion bit 5 and GPIO bits 4:0. Its
observed input value `0x44`, IO_MUX `0x200`, and baud 9600 were incorrectly decoded
as disconnected by the wider mask. Native TX uses peripheral output enable.

The S3 acceptance runner used `run_until_cycle`, which schedules core 0 only.
A clean baseline at the revision above booted the identical image, reached the
sketch and requested reset `0xc` at 2.053 guest seconds, with instruction counts
6,283,827 and 3,441,448. Using `Machine::run` fixed the runner without changing
firmware. One run using an older executable was excluded; the retained final
results identify the successfully built executable by hash.

Initial test development also exposed the required C3/C6 flash-size constructor
argument and S3's unsupported byte-wide peripheral store. The test uses a word
store. Later coverage added fractional baud dividers and S3 UART2 interrupt 29.
The superseded candidate patch has been removed; its historical hash remains in
`results.json`. The source-hash manifests and original negative measurements remain.

## Limits and privacy

This models completed non-inverted 8N1 bytes, not parity, flow control, electrical
wire timing or clock gating. Three percent baud tolerance is an emulator contract.
Reset tests cover board-state persistence and firmware reconfiguration, not device
power cycling. Review callbacks carry CPU bus cycles; replies arrive at device ticks.

Session narrative, cache-permission errors and unresolvable fork revision references
were removed. `redactions.json` records original and sanitized receipt hashes. Numeric
measurements and historical artifact hashes are unchanged. The removed patch limits
reconstruction of that superseded candidate; retained hashes still identify it.
