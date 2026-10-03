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

## Review correction (EX203)

UART inherits typed IO_MUX from the pin-transport stack and uses its shared route decoder. C6 TX masking, S3 GPIO46 and the S3/C3 20 MHz RC source are corrected. Board UART delivery is opt-in and cached at device attachment; callbacks carry emulated cycle stamps. RX delivery and framing checks are shared in esp-soc.

[review.json](review.json) records the full checks; [review-mutations.json](review-mutations.json) records eleven killed mutations, including all reported UART survivors and the C3 reset/idle and RMT mask checks. Run `python3 docs/evidence/uart-endpoint-2026-10-02/check-review-mutations.py` from an otherwise idle clean worktree to repeat; it restores each edited source in a `finally` block and writes logs under `/tmp`. All eight UART endpoint tests pass after restoration.

The shared [EX205 CPU receipt](../c3-peripherals-2026-10-02/cpu-rmt-final.json) measures three 30-emulated-second c3-hello runs per arm. Main CPU seconds: 2.606990, 2.624362, 2.614370; published UART: 2.708919, 2.738410, 2.696680; corrected C3 stack: 2.753298, 2.773484, 2.772790; UART on corrected stack: 2.803772, 2.748303, 2.773400. UART adds 0.02% to the corrected stack median (2.773400 versus 2.772790 s), within observed noise. The full stack remains 6.1% above main. Every sample executes 4.8 billion instructions/cycles, zero exceptions and identical console bytes. Background load is uncontrolled; these native CPU samples make no browser or hardware timing claim. This revises EX203's register correctness and idle delivery mechanism, retaining historical acceptance measurements above.

The deleted patch's historical hash and original/sanitized receipt hashes are preserved in `redactions.json`; the current hashes include this review update.

## Scoped idle participation follow-up

The lower C3 stack retains tick return 1 and its existing per-round interrupt
scans. UART joins the cached optional pin-service flag; UART-only boards still
receive input, covered by the C3 round-trip test with GPIO edges disabled. The
mutation that removes UART from that flag fails this test. Its initial TX-only
selector did not exercise RX and was corrected before the recorded mutation run.
[scope-mutations.json](scope-mutations.json) records the failure.

Two fixed seven-round batches per comparison, after one warmup per arm, use
c3-hello for 30 emulated seconds and getrusage user + system CPU time. Pooled
medians are main 2.427469 s and UART 2.439019 s,
+0.48%. In the separate direct comparison, C3 is
2.465262 s and UART 2.455657 s, -0.39%.
Both pairs have overlapping ranges. Every run has 4.8 billion instructions/cycles,
zero exceptions, 3028 interrupts and identical console output. All matching
completed batches are retained. Individual UART/C3 batches were -0.92% and
+0.96%; the pooled figure meets the bound, with visible between-batch variation.

The published UART head `094fdce` measured 2.503144 s against main 2.405076 s
in the earlier seven-round five-arm reference preserved under EX205. Final
acceptance uses two arms, as requested. [scope-cpu.json](scope-cpu.json) records
every sample, source/input/binary hash, load reading, the separate build
directories and limits. Load stays below 3 for each accepted run. These are
local CPU results, not browser or hardware timing and not a universal zero-cost
guarantee.

[scope-review.json](scope-review.json) records both Rust 1.99.0 Clippy checks,
505 passing workspace tests, the WASM build and all eight demos. Goldens are
unchanged. The final documentation-only rebase has identical non-documentation
files to the measured and checked revision; the receipt supplies the reachable
reproduction revision. No new external firmware/hardware acceptance is claimed.
