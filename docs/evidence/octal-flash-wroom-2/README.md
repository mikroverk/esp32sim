# EX231: octal SPI flash and octal PSRAM density (ESP32-S3-WROOM-2)

Base: upstream/main `f5a038bf`. The final source is the commit containing this receipt.
Inputs, hashes and the exact commands are in [inputs.json](inputs.json); the checks that ran
and their results in [checks.json](checks.json).

`--flash-octal` (ESP32-S3 only; the C3 and C6 refuse it like `--psram-mb`) makes the CS0 flash
a Macronix octal chip, as on the ESP32-S3-WROOM-2 modules (N16R8V, N32R8V, N32R16V):

- JEDEC ID C2 80 (log2(size) + 0x20), so ESP-IDF's `spi_flash_chip_mxic_opi_detect_size` gives
  the configured size; efuse FLASH_TYPE (RD_REPEAT_DATA3 bit 9) is set, which the ROM's and the
  bootloader's OPI bring-up test.
- The protocol follows CR2 at address 0: 0 SPI at power-on, 1 STR OPI, 2 DTR OPI, written by
  WRCR2 and read by RDCR2. The chip decodes only its current protocol's command form: 8-bit SPI
  commands, or the 16-bit octal form `cmd | !cmd << 8`. In DTR OPI the register reads (RDID, RDSR,
  RDCR2) return every byte twice, as the driver expects; the array reads do not.
- READ / FAST_READ / 8READ / 8DTRD with 32-bit addresses, PP4B, SE4B, BE32K4B, BE4B and CE above
  16 MB; an erase whose low address byte arrives as one MOSI byte (ESP-IDF 5.x's S3 PMS
  workaround) is folded back into the address.
- RSTEN + RST return the chip to SPI, as the ROM does at boot; a chip reset keeps the flash's
  protocol and the PSRAM's MR2, as the silicon does.

The octal PSRAM's MR2 density field follows `--psram-mb` for 4, 8, 16, 32 and 64 MB (ESP-IDF
sizes the PSRAM from MR2 alone, so a 16 MB array used to be mapped as the reset value's 8 MB).
2 MB (the quad PSRAM of the N8R2 modules) has no octal code and keeps the reset value.

Without `--flash-octal` nothing changes on the quad path except one boolean test per SPI1 user
command and two fields copied across a chip reset. No `device_set!`, bus, interrupt or per-tick
change. No golden was regenerated; the hello, atech, panel and classic goldens pass unchanged.

## Public definitions

- ESP-IDF v5.5.5 `components/esp_rom/esp32s3/include/esp32s3/rom/opi_flash.h` lines 84-117:
  the SPI and `_OCT` command values (RDID_OCT 0x609F, WREN_OCT 0xF906, RDCR2/WRCR2 0x71/0x72,
  PP4B_OCT 0xED12, SE4B_OCT 0xDE21, BE4B_OCT 0x23DC, RSTEN_OCT/RST_OCT); line 192
  `esp_rom_spi_set_dtr_swap_mode`, line 217 `esp_rom_opiflash_exec_cmd`. v4.4.8
  `components/esp_rom/include/esp32s3/rom/opi_flash.h` lines 80-120: the same values.
- v5.5.5 `components/spi_flash/spi_flash_chip_mxic_opi.c` line 15 `CMD_OPI_FLASH_MXIC`,
  lines 17-20 (READ_STR 0x13EC, READ_DTR 0x11EE, RDCR2 0x8E71, WRCR2 0x8D72), 41-52
  `detect_size` (`1 << (id - 0x20)`), 97-130 `read_id` (DTR: "[MD, MD, MT, MT, MID, MID]"),
  132-150 `read_reg`, 320-345 (RDCR2 1 = STR, 2 = DTR). v4.4.8 differs only in
  `SPI_FLASH_TRANS_FLAG_PE_CMD` flags and comment typos.
- v5.5.5 `components/spi_flash/esp32s3/spi_flash_oct_flash_init.c` lines 110-128
  `s_set_flash_dtr_str_opi_mode` (WRCR2 as an 8-bit SPI command, 32-bit address 0, one data
  byte: 0x00 SPI, 0x01 STR OPI, 0x02 DTR OPI), 188-215 `s_flash_init_mxic`; v4.4.8 lines 113-131
  the same.
- v5.5.5 `components/hal/spi_flash_hal_common.inc` lines 194-208 (`CONFIG_IDF_TARGET_ESP32S3`):
  an erase's low address byte goes as data so that PMS applies. v4.4.8 (179 lines) has no such
  split; the model takes both forms.
- v5.5.5 `components/soc/esp32s3/register/soc/spi_mem_reg.h` lines 474-486: USER2
  USR_COMMAND_BITLEN (bits 31:28) and USR_COMMAND_VALUE (15:0).
- v5.5.5 `components/soc/esp32s3/register/soc/efuse_reg.h` line 540 (RD_REPEAT_DATA3 at 0x3c),
  602-605 (FLASH_TYPE bit 9); v4.4.8 `components/soc/esp32s3/include/soc/efuse_reg.h` 433,
  472-475 the same. `components/bootloader_support/bootloader_flash/src/bootloader_flash.c`
  1024-1030 `bootloader_flash_is_octal_mode_enabled` (v5.5.5).
- v5.5.5 `components/esp_psram/esp32s3/esp_psram_impl_octal.c` lines 28-29 (0x4040 / 0xC0C0),
  69 (MR2 `density: 3`), 180-205 `s_get_psram_mode_reg`, 371-375 (density 1/3/5/7/6 = 4, 8, 16,
  32, 64 MB). v4.4.8 `components/esp_hw_support/port/esp32s3/opiram_psram.c` lines 264-267 know
  four codes: a 64 MB array reads as size 0 under Arduino-ESP32 2.x. Arduino-ESP32 3.x builds on
  IDF 5.x (the firmware below is on v5.5.5).
- Macronix MX25UM25645G datasheet: JEDEC C2 80 39, CR2 address 0 bit values, the command set.
  APS6408L-OBMx datasheet Rev. 3.9 Table 12: MR2[2:0] density codes.

The DTR byte swap on the wire (`esp_rom_spi_set_dtr_swap_mode`) is not modelled: data is
always in order, which is what the firmware sees after the ROM configures the swap. RDCR
(0x15) answers 0. Program and erase do not check the write-enable latch, as the quad model
does not. The MR2 density part is after NickoScope's esp32sim fork, commit 2b52ad5
(github.com/NickoScope/TWIN-NickoScopeMatrix-64x128, MIT), re-implemented here with the
512 Mbit code added. None of this was compared with silicon: the chip behaviour is the
datasheets' and what the ESP-IDF drivers send and expect.

## Firmware

[octal-probe.ino](octal-probe.ino), Arduino-ESP32 3.3.12 (its libraries built on ESP-IDF
v5.5.5), FQBN `esp32:esp32:esp32s3:FlashMode=opi,PSRAM=opi,FlashSize=16M` and `FlashSize=32M`
(the 32 MB build differs in a comment line). On UART0 it prints the flash ID and sizes
(`esp_flash_read_id`, `ESP.getFlashChipSize`, `esp_flash_get_physical_size`), the PSRAM size
(`esp_psram_get_size`, `ESP.getPsramSize`), a fill-and-check over the largest free PSRAM block
(every 64th word and the first and last 16 KB: a 16 MB array that wraps at 8 MB shows up as
`bad`), an erase / program / read at the last sector of the chip (above 16 MB on the 32 MB
build) and in the spiffs partition (read back through the cache by `esp_partition_mmap`), the
app partition's magic, then `done`; the first boot restarts once with the flash left in DTR OPI.

Run through the CLI from the mask ROM (`--boot rom`, board `none`, 40 emulated seconds,
commands in inputs.json):

| Image | Flags | ROM log | Both boots print |
| --- | --- | --- | --- |
| octal16 | `--flash-octal --flash-mb 16 --psram-mb 8` | `Octal Flash Mode Enabled`, `For OPI Flash, Use Default Flash Boot Mode` | `flash id=c28038 chip=16777216 physical=16777216`, `psram size=8388608 heap=8388608`, `psram block=8257524 … checked=40320 bad=0`, `flash end=ok`, `flash spiffs=ok`, `app magic=e9`, `done` |
| octal32 | `--flash-octal --flash-mb 32 --psram-mb 16` | the same | `flash id=c28039 chip=33554432 physical=33554432`, `psram size=16777216 heap=16777216`, `psram block=16515060 … checked=72576 bad=0`, `flash end=ok`, `flash spiffs=ok`, `app magic=e9`, `done` |
| octal16 on the quad model | `--flash-mb 16 --psram-mb 8` (control) | no octal mode line, `mode:DOUT` | `flash id=204018 chip=16777216 physical=16777216`, `psram size=8388608`, `bad=0`, `flash end=ok`, `flash spiffs=ok`, `done`: the bootloader's flash mode detection takes the quad chip, the image runs as on an N16R8 module |

The second boot (`boot again=1`, reset cause `RTC_SW_CPU_RST`) finds the flash still in DTR OPI
and the ROM's reset sequence brings it back to SPI before the bootloader's bring-up. The
`loop()` of the sketch is empty, so core 1 runs flat out after `done` (9.58 G instructions in
the 40 s run); this is the sketch, not the emulator.

## CPU comparison

Against main `f5a038bf` (the same tree, the same Rust 1.99.0 release build in a separate
worktree), the hello demos as CONTRIBUTING.md asks: S3 hello 3000 emulated seconds (board none),
C3 and C6 hello 30 seconds; child user CPU seconds via `GetProcessTimes` (Windows, 15.625 ms
timer resolution); one warmup pair B→M, then seven measured pairs alternating M→B, B→M; change is
the ratio of medians. Every run of a workload retired the same instruction count and produced the
same console SHA-256 on both builds (`identical_work_and_console` in [cpu.json](cpu.json)).

No load gate was possible and other applications were running on the desktop: the ranges are
wide (S3 hello 26–39 s in run 1, 29–45 s in run 2, on both builds alike), so these numbers show
parity within noise, not a measured difference. By the receipt rules' flags (slower in at least
6/7 pairs, or non-overlapping ranges) nothing is flagged: S3 hello +3.75% in run 1 (slower 4/7)
and −5.60% in the S3-only run 2 (slower 3/7), C3 hello −6.19% (2/7), C6 hello −9.25% (2/7).
Pocket Tank was not run: the change does not touch the S3 core. Structurally, the quad path
gains one boolean test per SPI1 user command and two field copies per chip reset; nothing runs
per tick.

### run 1, started 2026-10-15T14:14:23

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | Console SHA-256 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| s3-hello | 27.5000 (26.3125–36.6719) | 28.5312 (26.2500–39.2812) | +3.75% | 4/7 | 1134713226 + 655106431 | `98de90e19cbd…` |
| c3-hello | 1.7656 (1.5781–1.9375) | 1.6562 (1.3594–2.0469) | -6.19% | 2/7 | 4800000000 | `3b9d8d3eb053…` |
| c6-hello | 2.7031 (2.5156–3.7656) | 2.4531 (2.1406–3.0156) | -9.25% | 2/7 | 4800000000 | `bd69fbb9ac5c…` |

#### s3-hello

| Pair | Order | Main s | PR s |
| --- | --- | ---: | ---: |
| warmup | B→M | 27.094 | 28.891 |
| pair1 | M→B | 27.203 | 26.250 |
| pair2 | B→M | 26.312 | 27.234 |
| pair3 | M→B | 27.078 | 26.594 |
| pair4 | B→M | 27.500 | 28.531 |
| pair5 | M→B | 28.250 | 29.891 |
| pair6 | B→M | 36.312 | 30.562 |
| pair7 | M→B | 36.672 | 39.281 |

#### c3-hello

| Pair | Order | Main s | PR s |
| --- | --- | ---: | ---: |
| warmup | B→M | 1.656 | 1.703 |
| pair1 | M→B | 1.844 | 2.047 |
| pair2 | B→M | 1.766 | 1.359 |
| pair3 | M→B | 1.578 | 1.562 |
| pair4 | B→M | 1.781 | 1.547 |
| pair5 | M→B | 1.938 | 1.953 |
| pair6 | B→M | 1.703 | 1.688 |
| pair7 | M→B | 1.734 | 1.656 |

#### c6-hello

| Pair | Order | Main s | PR s |
| --- | --- | ---: | ---: |
| warmup | B→M | 2.578 | 2.531 |
| pair1 | M→B | 2.703 | 2.328 |
| pair2 | B→M | 2.766 | 2.672 |
| pair3 | M→B | 2.547 | 2.844 |
| pair4 | B→M | 2.547 | 3.016 |
| pair5 | M→B | 3.766 | 2.453 |
| pair6 | B→M | 2.797 | 2.141 |
| pair7 | M→B | 2.516 | 2.328 |

### run 2 (S3 hello only), started 2026-10-15T14:25:41

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | Console SHA-256 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| s3-hello | 40.4688 (29.5625–45.1562) | 38.2031 (27.8281–43.3906) | -5.60% | 3/7 | 1134713226 + 655106431 | `98de90e19cbd…` |

#### s3-hello

| Pair | Order | Main s | PR s |
| --- | --- | ---: | ---: |
| warmup | B→M | 32.875 | 36.406 |
| pair1 | M→B | 29.562 | 27.828 |
| pair2 | B→M | 45.156 | 29.125 |
| pair3 | M→B | 31.359 | 38.203 |
| pair4 | B→M | 36.031 | 41.578 |
| pair5 | M→B | 44.531 | 39.656 |
| pair6 | B→M | 40.469 | 43.391 |
| pair7 | M→B | 42.328 | 30.094 |

## Limits

- No hardware measurement: the octal chip's answers are the datasheets' and what IDF v5.5.5
  and v4.4.8 send; firmware was run on the emulator only.
- The octal firmware images are not committed (they are plain builds of the sketch with the
  options above); no golden pins them.
- One OPI flash model (Macronix); GigaDevice GD25LX256E (`*_GD` commands) is not modelled.
- STR OPI is modelled by the same code paths as DTR without the byte doubling; the firmware
  exercised DTR (Arduino's `FlashMode=opi` is DTR).
- No speed, timing or RF claim.
