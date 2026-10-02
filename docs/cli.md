# Command line

```
esp32sim [--chip s3|c3|c6] --boot rom --bootloader B.bin --ptable P.bin --app A.bin [--elf X.elf ...] [options]
esp32sim --flash-image flash.bin --boot rom ...
```

One binary for every chip; `esp32sim-c3` is `esp32sim --chip c3` and `esp32sim-c6` is
`esp32sim --chip c6`. The RISC-V chips' defaults differ where the chip does (`--boot rom`,
`--console uart0`, `--flash-mb 4`) and they refuse the flags their chip has nothing behind: the C3
board, WiFi, camera, PSRAM and register presets; the C6, which has a board and WiFi, only camera,
PSRAM and register presets.

## Images and boot
| Flag | Meaning |
| --- | --- |
| `--boot rom\|app` | `rom`: start at the mask ROM reset vector (real boot chain). `app`: load the app image segments and jump to its entry |
| `--bootloader F`, `--ptable F`, `--app F` | written to flash at 0x0 / 0x8000 / 0x10000 |
| `--flash-image F` | whole flash dump written at 0 |
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
| `--stub SYMBOL[=value]` (repeatable) | return `value` (default 0) immediately when execution reaches the function's entry; numeric function addresses require a `0x` prefix; accepts decimal, `0x` hex, `true` (1) or `false` (0); rejects invalid values |
| `--wifi SPEC` | attach a virtual access point the WiFi blob hears, plus a virtual network (DHCP/ARP/ICMP/DNS/SNTP; station 10.0.2.15, gateway 10.0.2.2) — for example `ssid=demo,chan=6,psk=demo-password,bssid=02:00:00:00:00:01`. `password` and `pass` alias `psk`; unknown keys and invalid values are rejected. Open and WPA2-PSK networks both join end to end, on the S3 (docs/wifi-plan.md) and on the C6 (docs/wifi-c6-plan.md) |
| `--net nat\|none` | what the virtual network does with traffic it is not itself answering: `nat` (default) forwards TCP and UDP to the host's own network through ordinary sockets, `none` refuses it |
| `--trace-fn PREFIX` (repeatable) | log every call to functions whose name starts with PREFIX, with args and caller; append `$` for an exact name |
| `--regstat FILE` | write per-register access statistics (count, pc, symbol) at exit — for reverse-engineering |

## Running
| Flag | Meaning |
| --- | --- |
| `--max-seconds S` | stop after this much emulated time, including all reboots; also applies to `--cooja` |
| `--max-insns N` | cap scheduler work across all reboots (details below); unavailable with `--cooja`, which uses `--max-seconds` |
| `--script F` | host actions at emulated times (below) |
| `--console usb\|uart0\|both\|all\|none`, `--console-prefix` | which consoles to print |
| `--realtime` | pace to wall time without the UI |
| `--web PORT [--web-dir DIR]` | browser UI (implies real time) |
| `--cam-image F`, `--cam-fps N` | camera source for boards with a camera |
| `--cooja` (C6) | run as a Cooja-NG external mote: the lock-step NDJSON protocol on stdin/stdout, the guest console as `log` events, the 802.15.4 frames as `tx`/`rx`. Honors `--no-reboot` and `--max-seconds` (see [esp32c6.md](esp32c6.md), "Cooja-NG lock-step") |
| `--cooja-slice-us N` | how long a busy guest runs before asking csim to step it again (default 100; `hello.args.slice_us` overrides). A transmission reaches csim's medium at the end of the slice it started in, so this bounds how late it is |
| `--cooja-rx-timing start\|end` | what an `rx` at `t` is: the frame's start (default — csim hands a frame-consuming mote the frame when it starts: `t` is the first preamble byte, the SFD five byte times later, RX_DONE after the whole PPDU, the ACK 192 µs after that) or its end, complete at `t` |
| `--cooja-verbose` | narrate the exchange on stderr |

The legacy name `--max-insns` counts scheduler work, not total retired instructions. The
default scheduler charges one unit per core 0 instruction or idle cycle advanced; another
core does not add to that count. It checks the cap between scheduling rounds, so it may finish
the current round past the requested number. With an approximate cost model, it counts
scheduled events. Use `--max-seconds` for a predictable duration. The final execution report
separately lists the actual instruction count for each core.

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
| `--debug AREAS` | what the model prints: device names or prefixes (`spi`, `usb`, `i2c`, `wifi`, `gdma`, `sha`, `rsa`, `lcd_cam`), `net`, `wifi-frames`, `aes`, `rom`, `mmio`, `rt`; also `ESP_EMU_DEBUG=a,b` |
| `--log-periph` | log the first access to every unknown peripheral register |
| `--no-jit` | run blocks through the interpreter instead of native code (aarch64 hosts compile blocks to machine code by default); the two must produce identical results, so this is the oracle when something looks wrong |
| `--stop-after-exceptions N` | stop after N exceptions |
| `--regtrace F`, `--regtrace-from-pc PC`, `--regtrace-max N` | register trace file for `hw/compare.py` |

Environment: `ESP_EMU_DEBUG=wifi,spi,net` is `--debug` for every run (the older
`ESP_EMU_DEBUG_SPI`, `ESP_EMU_DEBUG_NET`, `ESP_EMU_LOG_ALL`, `ESP_EMU_RT_LOG`... still work as aliases).
`XTENSA_DIS_FILES=a.dis:b.dis` feeds the decoder equivalence test.

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
4.6  adc 34 1.650          # apply 1.650 V to an ADC pad
4.7  touchpad 4 1          # capacitive GPIO4 pad touched; 0 releases
5.5  stop
```

`adc <gpio> <volts>` supplies a constant voltage on ESP32-S3 and classic ESP32.
`adcwave <gpio> <file> <rate_hz>` supplies waveform samples in volts, separated by
whitespace or commas, starting at the command time and holding the last sample
after the file ends. Classic ESP32 clamps sampled voltages to 0..3.3 V and rounds
to millivolts. `touchpad <gpio> <0|1>` controls classic capacitive pads separately
from display `touch`. Unsupported pads do not affect conversions. The classic
report includes enabled DAC output voltages on GPIO25 and GPIO26.

`hw/wsdrive.py [port] [seconds]` drives the same inputs over the UI's WebSocket and reports
real-time keep-up (push gaps, lag, audio delivered); `hw/wsaudio.py [port] [seconds]` listens to the
UI's audio stream and reports sample counts/peak (how to check sound without listening).
