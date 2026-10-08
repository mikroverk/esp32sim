# Command line

```
esp32sim [--chip s3|c3|c6] --boot rom --bootloader B.bin --ptable P.bin --app A.bin [--elf X.elf ...] [options]
esp32sim --flash-image flash.bin --boot rom ...
```

One binary for every chip; `esp32sim-c3` is `esp32sim --chip c3` and `esp32sim-c6` is
`esp32sim --chip c6`. The RISC-V chips' defaults differ where the chip does (`--boot rom`,
`--console uart0`, `--flash-mb 4`) and they refuse the flags their chip has nothing behind: the C3
board, camera, PSRAM and register presets; the C6, which has a board and WiFi, only camera,
PSRAM and register presets.

## Images and boot
| Flag | Meaning |
| --- | --- |
| `--boot rom\|app` | `rom`: start at the mask ROM reset vector (real boot chain). `app`: load the app image segments and jump to its entry |
| `--bootloader F`, `--ptable F`, `--app F` | written to flash at 0x0 / 0x8000 / 0x10000 |
| `--flash-image F` | whole flash dump written at 0 |
| `--chip s3\|c3\|c6` | which chip (default s3) |
| `--rom F` | mask ROM ELF (default: the chip's in `ESP32SIM_ROM_DIR`, then `~/.espressif/tools/esp-rom-elfs/*/`) |
| `--mac xx:xx:xx:xx:xx:xx` | the station MAC the efuses report |
| `--cam-stream PATH\|-`, `--cam-size WIDTHxHEIGHT` | live RGB24 camera source and its dimensions; requires `waveshare-cam` |
| `--serial TEXT` | bytes into the USB-Serial/JTAG console before the run |
| `--elf F` (repeatable) | symbols for logs/profiles (app ELF, bootloader ELF) |
| `--flash-mb N`, `--psram-mb N` | flash size (JEDEC follows it) and octal PSRAM size (default 8 / 2) |
| `--board atech14\|waveshare-cam\|waveshare-lcd4b\|waveshare-amoled18-v2\|none` | board model (default atech14); on the C6: `waveshare-c6-lcd147` or `none` |
| `--strap HEX`, `--reset-cause HEX`, `--efuse-regs F`, `--regs-init F` | reproduce a real chip's boot state (used by the differential tests) |
| `--no-reboot` | stop at the first chip reset instead of rebooting from ROM |
| `--flash-at OFFSET=FILE` (repeatable) | write a file into flash at a hex offset — a data partition's contents (the panel's `demo` partition takes `energydata.json`) |
| `--stub SYMBOL[=value]` (repeatable) | return `value` (default 0) immediately when execution reaches the function's entry; numeric function addresses require a `0x` prefix; accepts decimal, `0x` hex, `true` (1) or `false` (0); rejects invalid values |
| `--ble full` | experimental C3 register-level BLE controller; advertising, scanning and one virtual central; excludes `--ble` |
| `--ble-scan` | enable a virtual active scanner in C3 full mode; sends SCAN_REQ and observes guest-configured SCAN_RSP |
| `--ble-observe` | passive full-mode PDU log: half-microsecond timestamp, channel, type, AdvA, decoded AD and raw PDU |
| `--ble` | opt-in virtual BLE controller on S3/C3/C6; requires the matching application `--elf` |
| `--wifi SPEC` | attach a virtual access point the WiFi blob hears, plus a virtual network (DHCP/ARP/ICMP/DNS/SNTP; station 10.0.2.15, gateway 10.0.2.2) — for example `ssid=demo,chan=6,psk=demo-password,bssid=02:00:00:00:00:01`. `password` and `pass` alias `psk`; unknown keys and invalid values are rejected. Open and WPA2-PSK networks both join end to end, on S3, C3 and C6 (docs/wifi-plan.md, docs/esp32c3.md, docs/wifi-c6-plan.md) |
| `--net nat\|none` | what the virtual network does with traffic it is not itself answering: `nat` (default; `user` is an alias) forwards TCP and UDP to the host's own network through ordinary sockets, `none` refuses it |
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

### PWM observation

`--pwm PIN` reports the GPIO-matrix PWM frequency in Hz and duty percentage when the run stops.
Repeat it to observe several pins. PIN is an unsigned 8-bit decimal number.
LEDC is supported on S3, C3 and C6; MCPWM on S3 and C6.
A running timer with 0% or 100% duty still has a snapshot. "No supported running PWM" means
unrouted, disabled, paused or unsupported configuration; it does not imply a low GPIO level.
A paused hardware timer can hold its last level. This reports register-derived steady state,
not sampled edges. LEDC fades and MCPWM dead time, carrier and extra generator actions are unsupported.

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
5.5  stop
```

`hw/wsdrive.py [port] [seconds]` drives the same inputs over the UI's WebSocket and reports
real-time keep-up (push gaps, lag, audio delivered); `hw/wsaudio.py [port] [seconds]` listens to the
UI's audio stream and reports sample counts/peak (how to check sound without listening).

### ADC inputs

Scripts accept `adc <gpio> <volts>` for a constant voltage and
`adcwave <gpio> <file> <rate_hz>` for voltage samples separated by whitespace or
commas. The waveform starts at the action's emulated time. A later input replaces
the previous source. S3 GPIO1–10 select ADC1 and GPIO11–20 select ADC2; C3 GPIO0–4
select ADC1 and GPIO5 selects ADC2; C6 GPIO0–6 select ADC1.

```
0 adc 1 0.5
0 waituart0 10 ADC READY
0 uart0 A
1 adc 1 1.0
1 uart0 C
2 adcwave 1 /tmp/adc-volts.txt 1000
```

`waituart0 <timeout_s> <text>` holds later actions until UART0 prints the text or
the timeout expires. Raw counts and completed-conversion observations are exposed
through the [host ADC API](peripherals.md#host-adc-inputs), with no CLI equivalent.

## Virtual BLE

`--ble --elf firmware.elf` substitutes the controller lifecycle and legacy VHCI
functions on S3 and C3, or the native NimBLE transport on C6. The guest host and sketch remain unchanged.
An emulator-created FreeRTOS task delivers controller packets through guest
callbacks. S3 uses the windowed Xtensa ABI; C3 uses RV32IMC. Unstripped controller,
VHCI, task and BSS symbols are required on S3/C3. C6 requires native transport,
NPL and mbuf allocator symbols; its board image needs `--flash-mb 8`.
C6 is **ESP-IDF 5.5.x only**. Enable reads `esp_app_desc.idf_ver` from the application
ELF and refuses missing, malformed or other versions before installing hooks.
Its native transport uses that version's `os_mbuf` layout and NPL initialization.
S3/C3 accept builds exposing the required VHCI/lifecycle symbols, controller BSS
and function sizes; there is no IDF version gate. Arduino 3.3.8 with IDF 5.5.4 is
validated. IDF 4.4 Bluedroid is unsupported and fails with a missing-symbol error.
Other IDF releases have not been validated.

`[ble]` output reports advertising data and discovered GATT handles. Script commands
use seconds like the other script actions. For example:

```text
0.5 ble connect
0.6 ble discover
1.0 ble read 0x0010
1.1 ble write 0x0010 68656c6c6f
```

Use handles reported by discovery. `ble subscribe CCC_HANDLE` writes notification
enable to a discovered client configuration descriptor. Commands are validated when
CLI arguments are parsed, before loading the ROM, and require `--ble`. `connect` waits for guest advertising;
ATT commands wait for the connection and run in order, with one request outstanding.
At exit, the CLI reports the number of queued or in-flight commands still pending.
Writes are limited to 20 bytes. A scanning guest sees a virtual
peripheral named `esp32sim` advertising the Battery Service.

This models one unencrypted LE link, legacy advertisements and ATT MTU 23.
It does not model RF, pairing, physical connection timing or the controller's
registers. Unsupported HCI commands return Unknown Command. The callback task
and its storage stay allocated until reboot. C6 initializes the guest NPL support
and a heap-backed mbuf pool; it uses no controller BSS reservation. Cost-model execution
does not support function substitutions; reported cycle time is not radio timing.

`--ble full` selects the experimental C3 link-controller register model and excludes
`--ble` HCI substitution. It runs the original guest controller without ELF hooks.
It implements initialization, clock capture, timer IRQs and legacy advertising on
channels 37–39. Completion IRQs let the guest schedule its next event. `--ble-observe`
logs to stderr emitted ADV_IND, ADV_NONCONN_IND or ADV_SCAN_IND packets as `[ble-air]` lines;
`hus` is modeled time in half-microseconds. `[ble-config]` reports configured
SCAN_RSP data separately; a passive observer cannot elicit that response. The queue
holds the latest 1024 observations and reports dropped entries when polled.
`--ble-scan` enables active SCAN_REQ/SCAN_RSP exchanges through guest RX descriptors.
Use script commands `ble connect`, `ble read-uuid SERVICE CHARACTERISTIC`,
`ble central-stop`, and `ble disconnect`. The last sends LL_TERMINATE_IND;
`central-stop` silences the central so the guest supervision timer can expire.
For example, pass this file with `--ble full --script connection.txt`:

```text
0.5 ble connect
0.7 ble read-uuid 4fafc201-1fb5-459e-8fcc-c5c9c331914b beb5483e-36e1-4688-b7f5-ea07361b26a8
3.1 ble central-stop
```

The model supports one unencrypted 1M CSA#1 link, a fixed 30 ms interval and
2 s supervision timeout, default ATT MTU 23 and no L2CAP fragmentation. It replies
to LL_VERSION_IND even without an ATT request. Pairing, CSA#2, PHY updates and
optional LL procedures are unsupported. Register meanings and packet timing
remain inferred/model choices. [EX213 evidence](evidence/ble-c3-connection/README.md)
separates the historical probe-b field checks from unvalidated RF timing.

## Live camera input

On `--board waveshare-cam`, `--cam-stream PATH --cam-size WIDTHxHEIGHT` reads
consecutive RGB24 frames with no headers or row padding. Use `-` for stdin or a FIFO
path for a producer in another process. Streaming enables real-time pacing, keeps only
the latest complete input, and retains the last image at EOF. A truncated frame reports
an error and leaves the previous image intact. This is a live source only: regular
files are read as fast as possible, so the frames observed by firmware are not
deterministic. Use `--cam-image` for deterministic still input. FIFO opening runs
on the reader thread and never blocks emulator startup or shutdown. Input dimensions
share the browser limit: an equivalent RGBA message must fit 8 MiB. This limit also applies to `--cam-image`. `--console` still selects firmware console output.

For a macOS webcam, choose a video device with
`ffmpeg -f avfoundation -list_devices true -i ''`, then replace `0` below with its index:

```sh
ffmpeg -f avfoundation -framerate 10 -video_size 640x480 -i '0:none' \
  -an -pix_fmt rgb24 -f rawvideo - |
  target/release/esp32sim --board waveshare-cam --boot rom \
    --rom "$ROM" --bootloader "$BOOTLOADER" --ptable "$PTABLE" --app "$APP" \
    --cam-stream - --cam-size 640x480 --no-dump
```

The firmware programs the OV5640 through SCCB. The model crops its sensor-array
window, scales to its output dimensions with nearest-neighbour sampling, and emits
RGB565 MSB first, YUYV422, or grayscale Y8. `--cam-image` still loads BMP or PPM;
a live frame replaces that image through the same board hook. Without a host image,
the configured sensor supplies black frames so firmware can initialize. `--cam-fps` controls
emulated capture cadence independently of host updates. JPEG encoding and RAW Bayer
are unsupported and produce no frame. Optical black pixels, ISP offsets, binning,
mirror/flip, exposure and sensor clock calibration are not modeled.

Camera crop/scale does not model ISP offsets (0x3810–0x3813), mirror/flip
(0x3820/0x3821), or binning. Typical driver settings can differ from the sensor
by about 2.5% horizontally and 1.6% vertically; mirror/flip is not applied.
