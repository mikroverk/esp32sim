# EX259: CCOUNT counts again from a CPU reset (#220)

Base: upstream/main `2e36224d`. The final source is the commit containing this receipt.
Inputs, hashes and the exact commands are in [inputs.json](inputs.json); the checks that ran
and their results in [checks.json](checks.json).

`Cpu::reset` (`xtensa-lx7/src/state.rs`), which `Machine::reboot` runs for each core, left
CCOUNT where the previous boot had taken it. ESP-IDF's early log timestamp is CCOUNT divided by
the CPU's ticks per millisecond (IDF v5.5.4 `components/log/src/os/log_timestamp.c:28-37`,
`esp_log_early_timestamp`), so after `esp_restart()` the S3's second boot printed
`I (24844) boot: ...` where the first printed `I (7)`, and the classic ESP32 of #220 the same.

`Cpu::reset` now sets CCOUNT to 0 (and recomputes the browser build's cached next-comparator
event). The comparators keep their values and count from the new CCOUNT. Nothing else changes:
the ROM does not write CCOUNT on the way to the bootloader (under emulation the count carried
through it unchanged), so restarting it with the core is what silicon shows below.

## Silicon

The golden's own images (`web/wasm/fw/public/hello-bootloader.bin`, `hello-ptable.bin`,
`hello_world.bin`: ESP-IDF v5.5.4 `examples/get-started/hello_world`, which counts down and
calls `esp_restart()`), flashed to an ESP32-S3 board (chip revision v0.2) and read over its
USB-Serial/JTAG port from a reset by the host through two restarts:
[s3-hello-reboot.txt](s3-hello-reboot.txt).

| Boot | Reset | `boot: ESP-IDF v5.5.4 2nd stage bootloader` | `cpu_start: Multicore app` | `main_task: Calling app_main()` |
| --- | --- | ---: | ---: | ---: |
| 1 | `0x15 (USB_UART_CHIP_RESET)` | 24 | 145 | 261 |
| 2 | `0xc (RTC_SW_CPU_RST)`, from `esp_restart()` | 24 | 145 | 261 |
| 3 | `0xc (RTC_SW_CPU_RST)` | 24 | 145 | 261 |

Every early-log timestamp of the three boots is within 1 ms of the same line in the others: the
count starts again at each reset, a software CPU reset like a chip reset. The application's
`ELF file SHA256: 92ff5a529...` is the golden's.

## Emulator

`hello_world_s3_reboot` (`cli/tests/goldens.rs`), the same images for 12 emulated seconds:

| | Second boot, first bootloader line | Second boot against the first |
| --- | --- | --- |
| main | `I (24844) boot: ESP-IDF v5.5.4 2nd stage bootloader` | each of the 47 timestamps 24837-24838 ms later |
| this change | `I (7) boot: ESP-IDF v5.5.4 2nd stage bootloader` | the 48 lines from the bootloader to `Calling app_main()` identical |

The golden is regenerated: `hello-s3-reboot.console.txt` (the second boot's 47 timestamped
lines) and `hello-s3-reboot.insns` (10306747 -> 10260321). The fewer instructions are
consistent with those 47 lines formatting one- and two-digit timestamps instead of five-digit
ones; that was not traced instruction by instruction. Every other golden is byte-identical:
the C3, C6 and P4 reboot goldens never had the effect (RISC-V), and no other Xtensa golden
reboots.

The absolute values differ from silicon (7 ms against 24 ms to the bootloader's first line);
that is the emulator's boot timing, not part of this change.

## Tests

`xtensa-lx7/src/core.rs`, `reset_restarts_ccount_and_the_comparators_count_from_it`: a reset
from CCOUNT `0x12345678` reads 0, and CCOMPARE0 left at 100 is the next wake 100 cycles later
and matches at the 100th. Without the change it fails.

## Limits

- Probed on one ESP32-S3. The classic ESP32 of #220 runs the same `Cpu::reset` and is not
  probed; the change applies to both.
- CCOMPARE keeps its value over the reset, as before; firmware writes it before it enables the
  timer interrupt. Its reset value on silicon was not probed.
- The classic ESP32's deep-sleep wake (#240) goes through the same reset and now also starts
  CCOUNT again; no classic golden pins its early timestamps.
