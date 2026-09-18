# Command line

```
esp32sim [--chip s3|c3|c6] --boot rom --bootloader B.bin --ptable P.bin --app A.bin [--elf X.elf ...] [options]
esp32sim --flash-image flash.bin --boot rom ...
```

One binary for every chip; `esp32sim-c3` is `esp32sim --chip c3` and `esp32sim-c6` is
`esp32sim --chip c6`. The RISC-V chips' defaults differ where the chip does (`--boot rom`,
`--console uart0`, `--flash-mb 4`) and they refuse the S3-only flags (board, WiFi, camera, PSRAM,
register presets).

## Images and boot
| Flag | Meaning |
| --- | --- |
| `--boot rom\|app\|download` | `rom`: start at the mask ROM reset vector (real boot chain). `app`: load the app image segments and jump to its entry. `download`: boot the ROM with the chip's UART-download strap (`0x7` on S3, `0x2` on C3/C6); explicit `--strap` wins |
| `--bootloader F`, `--ptable F`, `--app F` | written to flash at 0x0 / 0x8000 / 0x10000 |
| `--ptable-offset 0xNNNN` | where `--ptable` (and thus the partition table itself) is written, default 0x8000 — match your project's `CONFIG_PARTITION_TABLE_OFFSET` if it isn't the default (a bootloader built with Secure Boot V2 + Flash Encryption is often larger than the default 0x8000 gap and needs this pushed out, or it'll silently overlap and corrupt the tail of the bootloader image) |
| `--app-offset 0xNNNN` | where `--app` is written (and, in `--boot app`, where its image is read from), default 0x10000 — match the offset of the partition you're booting (e.g. the `factory` app partition's actual offset from your partition table), which can differ once earlier partitions have been resized or reordered |
| `--flash-image F` | whole flash dump written at 0 |
| `--flash-state F` | persistent mutable logical flash on S3/C3/C6; create from seed images when missing, otherwise load it and ignore the seeds |
| `--efuse-state F` | persistent mutable S3/C3/C6 physical eFuse state: native 336-byte payload or compatible 1 KiB QEMU backing file |
| `--chip s3\|c3\|c6` | which chip (default s3) |
| `--rom F` | mask ROM ELF (default: the chip's in `~/.espressif/tools/esp-rom-elfs/*/`) |
| `--mac xx:xx:xx:xx:xx:xx` | the station MAC the efuses report |
| `--serial TEXT` | bytes into the USB-Serial/JTAG console before the run |
| `--elf F` (repeatable) | symbols for logs/profiles (app ELF, bootloader ELF) |
| `--flash-mb N`, `--psram-mb N` | flash size (JEDEC follows it) and octal PSRAM size (default 8 / 2) |
| `--board atech14\|waveshare-cam\|waveshare-lcd4b\|waveshare-amoled18-v2\|none` | board model (default atech14); on the C6: `waveshare-c6-lcd147` or `none` |
| `--strap HEX`, `--reset-cause HEX`, `--efuse-regs F`, `--regs-init F` | reproduce a real chip's boot state (used by the differential tests) |
| `--no-reboot` | stop at the first chip reset instead of rebooting from ROM |
| `--flash-at OFFSET=FILE` (repeatable) | write a file into flash at a hex offset — a data partition's contents (the panel's `demo` partition takes `energydata.json`) |
| `--stub SYMBOL[=value]` (repeatable) | return `value` (default 0) immediately when execution reaches the function's entry |
| `--wifi SPEC` | attach a virtual access point the WiFi blob hears, plus a virtual network (DHCP/ARP/ICMP/DNS/SNTP; station 10.0.2.15, gateway 10.0.2.2) — `ssid=NAME,chan=N,psk=PASS,bssid=xx:..`. Open and WPA2-PSK networks both join end to end (docs/wifi-plan.md) |
| `--net nat\|none` | what the virtual network does with traffic it is not itself answering: `nat` (default) forwards TCP and UDP to the host's own network through ordinary sockets, `none` refuses it |
| `--trace-fn PREFIX` (repeatable) | log every call to functions whose name starts with PREFIX, with args and caller |
| `--regstat FILE` | write per-register access statistics (count, pc, symbol) at exit — for reverse-engineering |

### Persistent state

`--flash-state` makes ESP-IDF NVS, OTA metadata and every other flash-backed
partition survive a separate emulator process. `--efuse-state` persists
one-way eFuse burns. Both flags currently support the ESP32-S3 only.

```sh
esp32sim --chip s3 --boot rom \
  --bootloader bootloader.bin --ptable partition-table.bin --app app.bin \
  --flash-mb 16 \
  --flash-state .state/flash.bin --efuse-state .state/efuse.bin
```

If a state file is absent, its seeds are applied once before it is created. If
it exists, state wins and the corresponding seeds are ignored. Delete/select a
new state file to emulate a fresh device; seed files are never overwritten.
Flash state is logical CPU-visible data, not physical encrypted ciphertext.
See [storage.md](storage.md) for exact formats, write timing, reset behavior,
browser profiles and security considerations.

## Running
| Flag | Meaning |
| --- | --- |
| `--max-seconds S`, `--max-insns N` | stop after emulated time / instructions |
| `--script F` | host actions at emulated times (below) |
| `--console usb\|uart0\|both\|all\|none`, `--console-prefix` | which consoles to print |
| `--uart-tcp HOST:PORT` | expose UART0 as a raw binary TCP server; implies real-time pacing and removes UART0 from stdout |
| `--realtime` | pace to wall time without the UI |
| `--web PORT [--web-dir DIR]` | browser UI (implies real time) |
| `--cam-image F`, `--cam-fps N` | camera source for boards with a camera |
| `--cooja` (C6) | run as a Cooja-NG external mote: the lock-step NDJSON protocol on stdin/stdout, the guest console as `log` events, the 802.15.4 frames as `tx`/`rx` (see [esp32c6.md](esp32c6.md), "Cooja-NG lock-step") |
| `--cooja-slice-us N` | how long a busy guest runs before asking csim to step it again (default 100; `hello.args.slice_us` overrides). A transmission reaches csim's medium at the end of the slice it started in, so this bounds how late it is |
| `--cooja-rx-timing start\|end` | what an `rx` at `t` is: the frame's start (default — csim hands a frame-consuming mote the frame when it starts: `t` is the first preamble byte, the SFD five byte times later, RX_DONE after the whole PPDU, the ACK 192 µs after that) or its end, complete at `t` |
| `--cooja-verbose` | narrate the exchange on stderr |

### esptool over UART TCP

Start the S3 in its UART0-only ROM download mode. A persistent flash state represents the mutable
device; firmware seed files are not modified:

```sh
esp32sim --chip s3 --board none --boot download \
  --flash-mb 8 --flash-state .state/device-flash.bin \
  --uart-tcp 127.0.0.1:5555 --console none
```

Raw TCP does not carry RTS/DTR, so esptool cannot reset the emulator into or out of download mode.
Use the no-reset options. ROM-loader mode works with esptool 4.7 and later:

```sh
esptool.py --chip esp32s3 --port socket://127.0.0.1:5555 \
  --before no_reset --after no_reset --no-stub flash_id
```

For the uploaded flasher, use esptool's modern v2 stub (available in esptool 4.8 and later):

```sh
ESPTOOL_STUB_VERSION=2 esptool.py --chip esp32s3 \
  --port socket://127.0.0.1:5555 --before no_reset --after no_reset \
  write_flash 0x0 bootloader.bin 0x8000 partition-table.bin 0x10000 app.bin
```

The legacy v1.3.0 S3 stub contains an upstream hard-coded classic-ESP32 flash-data address and
panics after upload. esp32sim does not add a non-silicon memory alias for it. Use the v2 stub or
`--no-stub`.

C3 and C6 use the same socket workflow with `--chip c3`/`esp32c3` or `--chip c6`/`esp32c6`.
Their ROM loaders support `flash_id`, write, verify and read-back; those ROMs explicitly do not
implement `erase_region`. The v2 stub supports `flash_id`, compressed write, verify, read-back,
bounded erase and chip erase on both. `--flash-state` persists the results across processes, and
normal ROM boot from an esptool-flashed ESP-IDF image is validated on both chips.

After flashing, stop the download-mode process and start a normal ROM boot with the same
`--flash-state` path.

## Outputs
| Flag | Meaning |
| --- | --- |
| `--wav F` | audio captured from I2S (whichever controller played) |
| `--tft-png F`, `--gram-png F` | display frame (visible, scaled) / raw GRAM |
| `--no-dump` | skip the register dump at exit |

## Debugging
| Flag | Meaning |
| --- | --- |
| `--trace`, `--trace-from N` | per-instruction trace (from instruction N) |
| `--break PC` (repeatable) | stop at PC |
| `--watch ADDR` | stop when a word changes |
| `--peek ADDR,N`, `--disasm ADDR,N` | dump memory / disassemble at exit |
| `--profile` | top PCs by instruction count (single-steps, and keeps idle cores stepping: an idle core shows as a hot `waiti`) |
| `--profile-blocks` | time per function from the block path — full speed, no timing change; needs `--elf` for names |
| `--coverage`, `--coverage-file F` | block starts reached, per function; with a file, one `addr symbol` line each |
| `--irq-latency` | cycles from an interrupt line appearing at a core to the core taking it, per line; retains block execution |
| `--vcd F` | GPIO edges and interrupt lines as a VCD waveform (1 ps units); retains block execution |
| `--debug AREAS` | what the model prints: device names or prefixes (`spi`, `usb`, `i2c`, `wifi`, `gdma`, `sha`, `rsa`, `lcd_cam`), `ulp` (controller start/timer/halt lifecycle), `net`, `wifi-frames`, `aes`, `rom`, `mmio`, `rt`; also `ESP_EMU_DEBUG=a,b` |
| `--log-periph` | log the first access to every unknown peripheral register |
| `--no-jit` | run blocks through the interpreter instead of native code (aarch64 hosts compile blocks to machine code by default); the two must produce identical results, so this is the oracle when something looks wrong |
| `--stop-after-exceptions N` | stop after N exceptions |
| `--regtrace F`, `--regtrace-from-pc PC`, `--regtrace-max N` | register trace file for `hw/compare.py` |

Environment: `ESP_EMU_DEBUG=wifi,spi,net` is `--debug` for every run (the older
`ESP_EMU_DEBUG_SPI`, `ESP_EMU_DEBUG_NET`, `ESP_EMU_LOG_ALL`, `ESP_EMU_RT_LOG`... still work as aliases).
`XTENSA_DIS_FILES=a.dis:b.dis` feeds the decoder equivalence test.

When either ESP32-S3 ULP architecture runs, the final report includes its instruction and cycle
counts, traps and main-CPU wake requests. These counts are diagnostic only and are not included
in the main-core `--max-insns` limit. See [ulp.md](ulp.md) for supported behavior and limits.

## Action scripts

One action per line, `<seconds> <cmd> [args]`; buttons/encoder are active low.

```
1.5  press btn1 150        # press for 150 ms (btn1, btn2, knob/sw, or a GPIO number)
2.0  release 16
2.5  gpio 17 0
3.0  knob cw 3             # 3 detents clockwise (ccw for the other way)
4.0  serial {"action":"set_note","value":"5"}
4.2  uart0 root              # a line into UART0's receive FIFO (also `uart1`): a login on a Linux console
4.5  touch 450 30 1        # touch panel press at (450,30); `touch x y 0` releases
5.5  stop
```

`hw/wsdrive.py [port] [seconds]` drives the same inputs over the UI's WebSocket and reports
real-time keep-up (push gaps, lag, audio delivered); `hw/wsaudio.py [port] [seconds]` listens to the
UI's audio stream and reports sample counts/peak (how to check sound without listening).
