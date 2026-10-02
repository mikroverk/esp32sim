# Peripheral coverage

Legend: **full** = everything the IDF/Arduino drivers use; **partial** = the paths exercised
so far; **stub** = accepts writes, returns plausible reads; **—** = not modelled (unknown
registers are logged with `--log-periph`).

ESP32-S3 peripheral MMIO reads and writes must be aligned 32-bit accesses. Byte and halfword accesses raise a prohibited-access fault before any device side effect; unaligned word accesses raise an alignment fault. This is the emulator policy for unsupported accesses, not a claim about every silicon PMS configuration.

| Block | Base | Status | Modelled |
| --- | --- | --- | --- |
| Interrupt matrix (per core) | 0x600C2000 | full | source→line mapping for both cores, level lookup |
| System / sensitive / APB_CTRL | 0x600C0000… | partial | core-1 release/reset, cache enables, clock regs as stubs |
| Cache MMU | 0x600C5000 | full | 512 entries, flash and PSRAM pages, invalid entries fault |
| SPI0/SPI1 (flash controller) | 0x60002000/3000 | full | user commands, JEDEC (size follows `--flash-mb`), read/program/erase, status/QE |
| Octal PSRAM (on SPI1 CS1) | — | full | mode registers MR0–MR8, sync read/write, `--psram-mb` |
| efuse | 0x60007000 | partial | MAC, chip revision, defaults; `--efuse-regs` loads a dump |
| RTC_CNTL | 0x60008000 | partial | reset cause, slow-clock time, SW resets, RTC watchdog reset stages, feed and write protection; watchdog interrupt raw status is set but not routed to the CPU ([model](../esp-periph/src/rtc_cntl.rs)) |
| systimer | 0x60023000 | full | 2 units, 3 targets, one-shot/periodic |
| Timer groups 0/1 | 0x6001F000/20000 | partial | timers 0/1 with alarm, auto-reload and interrupts; timer-group watchdog registers are stubs and never fire ([model](../esp-periph/src/timg.rs)) |
| GPIO / IO_MUX | 0x60004000/9000 | full | out/enable/input, pin matrix in/out selects, edge/level interrupts, strap |
| UART0/1/2 | 0x60000000… | partial | TX straight to the console (FIFO count 0, TX-done/empty raised); a 128-byte RX FIFO fed from the page keyboard and scripts, rxfifo_cnt in STATUS, RXFIFO_FULL as a level against the CONF1 threshold, overflow flagged, rxfifo_rst honoured |
| USB Serial/JTAG | 0x60038000 | full | TX/RX FIFOs, interrupts (IDF console and Arduino `Serial`) |
| I2C0/I2C1 | 0x60013000/27000 | full | IDF `i2c_master` command list, FIFOs, NACK/END/COMPLETE interrupts |
| GDMA | 0x6003F000 | partial | out-channels (I2S0/I2S1) and in-channels (CAM); memory-to-memory channel pairs (MEM_TRANS_EN) for the transaction-based `esp_async_memcpy` of IDF v5.4, which starts both channels for each copy: the copy lands in one round, IN descriptors written back (length, owner, SUC_EOF), IN_SUC_EOF_DES_ADDR set, a CPU-owned descriptor parks that side, faults and runaway walks raise DSCR_ERR. The IDF v5.1 ring driver kicks each copy with RESTART, which this model rewinds to the LINK address, so it copies once and stalls; descriptor walk, DONE/EOF/TOTAL_EOF |
| I2S0 / I2S1 | 0x6000F000/2D000 | partial | TX: frame rate derived from the clock tree (source, integer + fractional MCLK divider, BCK divider, slot width and count), 16-bit stereo capture to PCM; RX — |
| RMT | 0x60016000 | partial | TX channels: symbol RAM, clock divider, end marker, done interrupt; RX — |
| LCD_CAM | 0x60041000 | partial | camera engine (start/reset, VSYNC, frame pump from GDMA RX) and the LCD RGB/DPI engine (timing/clock registers, frame pump into GDMA TX, LCD_VSYNC); i8080 LCD mode — |
| SHA | 0x6003B000 | full | SHA-1/224/256/384/512, block and GDMA modes (bootloader image verification, TLS certificate digests) |
| AES | 0x6003A000 | partial | block and DMA modes, ECB/CBC/CTR/OFB, all key lengths (mbedTLS, WPA2 group-key unwrap); hardware GCM — |
| RSA/MPI | 0x6003C000 | full | large-number multiply, modular multiply and modular exponentiation up to 4096 bits, polled or interrupt-driven (every mbedTLS public-key operation) |
| WiFi MAC | 0x60033000 | partial | TX queues, RX descriptor ring, interrupt events, TSF, filters — enough for scan/auth/assoc/data with the unmodified blob (docs/wifi-plan.md) |
| RNG | 0x6003B000 | full | random words |
| regi2c / I2C_MST (PLL, RF analog) | 0x6000E000 | stub | reads back what was written; BBPLL and pkdet calibration-done bits set |
| GP-SPI2 master | 0x60024000 | partial | CPU-driven command/address/data phases; board MISO responses; bounded GDMA TX descriptor completion; RX DMA is not modeled |
| PCNT | 0x60017000 | full | 4 units × 2 channels, pos/neg/ctrl modes via the GPIO matrix, limits/thresholds/zero events, counter reset/pause |
| LEDC | 0x60019000 | partial | timers, fractional dividers, static duty latching, interrupts and GPIO-matrix output; hardware fades — |
| MCPWM0/1 | 0x6001E000/2C000 | partial | up-counting timers, compare latching, simple generator actions, interrupts and GPIO-matrix output; sync, capture, fault, carrier and dead time — |
| ADC, SPI3, TWAI, SDMMC, USB-OTG | — | — | |
| WiFi baseband/PHY/RF, BT | — | — | radio registers are faked, not modelled; see wifi-plan.md |

CPU-side: full base ISA, FPU (single precision), MAC16, booleans, PIE (all esp-dl/esp-dsp
ops; FFT/GPIO/s32 corners decode but are not executed).
