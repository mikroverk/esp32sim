<p align="center">
  <img src="docs/assets/esp32sim-logo-icon.png" alt="esp32sim — a smiling chip in an emulator window" width="320">
</p>

# esp32sim — an ESP32 emulator in Rust: **Xtensa (ESP32, S3) and RISC-V (C3, C6)**

Instruction-level emulation of four ESP32 SoCs, across **both of Espressif's CPU architectures**.
Each boots the **real mask ROM**, the real 2nd-stage bootloader and an unmodified application
image — no patched firmware, no stubs in the way — with enough of the SoC modelled to run real
projects end to end. No cloud, no accounts. MIT.

The [classic ESP32](docs/esp32.md) also boots its ECO3 ROM on dual LX6 cores, with GPIO, UART and timers.

| | **ESP32-S3** | **ESP32-C3** | **ESP32-C6** |
| --- | --- | --- | --- |
| CPU | 2 × Xtensa LX7 @ 240 MHz | 1 × RISC-V RV32IMC @ 160 MHz | 1 × RISC-V RV32IMAC @ 160 MHz |
| Command | `esp32sim` | `esp32sim-c3` | `esp32sim-c6` |
| Core crate | `xtensa-lx7` | `riscv-rv32` | `riscv-rv32` (+ the A extension) |
| Reported full-listing decoder checks | 977 k instructions, 0 mismatches | 161 k instructions, 0 mismatches | 126 k instructions, 0 mismatches |
| Boots | ROM → bootloader → FreeRTOS → app | ROM → bootloader → FreeRTOS → app | ROM → bootloader → FreeRTOS → app |
| Boards / displays | ST7735, ST7701S 480×480 + touch, WS2812, camera, audio | none — console only | ST7789 172×320 over SPI+DMA, WS2812, an 802.15.4 energy-detect stand-in |
| WiFi | virtual AP, WPA2, DHCP/DNS/NTP, NAT to the real network, HTTPS | the same virtual AP and network: scan, WPA2, DHCP, NAT (one station) | the same virtual AP and network: scan, WPA2, DHCP, NAT (one station, legacy rates) |
| Speed | block interpreter + **AArch64 JIT**, ~150–240 Minsn/s | plain interpreter (no block cache or JIT yet) — still well above real time on hello_world | same interpreter |
| In the browser | yes (WebAssembly) | yes (WebAssembly) | yes (WebAssembly) |
| Reported hardware comparisons | JTAG trace, 8000 steps after timing-loop resynchronization | console diff, 205/208 lines identical | console diff, 203/204 lines identical |
| Status | mature | draft — see [docs/esp32c3.md](docs/esp32c3.md) | draft — see [docs/esp32c6.md](docs/esp32c6.md) |

These full-listing and hardware counts describe earlier manual runs. CI runs the checked-in
sampled decoder corpora and emulator golden outputs; it excludes the external full-listing
tests. The Xtensa objdump test excludes PIE operand comparisons. Hardware trace comparison
normalizes window exceptions and resynchronizes delay loops, so it is not uninterrupted
cycle-by-cycle agreement. See the [test scope](tests/README.md), [CI workflow](.github/workflows/ci.yml),
[Xtensa decoder test](xtensa-lx7/tests/objdump_diff.rs) and [hardware comparator](hw/compare.py).

Most of the SoC is shared: the C3 reuses the S3's UART, USB-Serial/JTAG, systimer, timer groups,
GPIO, SPI flash controller, GDMA and SHA/AES/RSA models unchanged, and adds its own memory map,
cache controller and interrupt matrix; the C6 reuses the same models again, with its own memory
map, PLIC front-end, L1 cache, PCR and always-on LP blocks. Only the CPU crates are genuinely
separate.

**Try it in a browser, no install:** <https://mikroverk.github.io/esp32sim/> — Linux on the
S3, the Touch-LCD-4B panel with its SID player, the pocket-tank LLM aquarium, and the C3 and C6 demos.

```
esp32sim/
  emu-core/       the Bus and Core traits, Trap, ClockTree, AArch64 encoder — shared by both cores
  esp-soc/        Machine<S: Soc>: scheduler, device time, console, scripts, web UI, loaders, boards —
                  one machine for every chip; a chip plugs in its cores, bus and interrupt routing;
                  the virtual WiFi access point, the network behind it and the NAT, for every chip
  esp-periph/     the peripheral IP the chips share (UART, systimer, TIMG, GPIO, SPI_MEM, GDMA, crypto,
                  I2S, RMT, I2C, …), the Device trait, and the device_set! table that mounts them
  ── ESP32-S3 (Xtensa LX7, dual core) ──
  xtensa-lx7/     decoder (sampled objdump checks plus optional full listings), interpreter
                  (windowed regs, loops, XEA2 exceptions/interrupts, FPU, MAC16, booleans,
                  PIE SIMD), basic-block interpreter, AArch64 JIT, objdump-compatible disassembler
  esp32s3/        SoC + boards: memory map, cache MMU, SPI flash/PSRAM, SHA/AES/RSA, RNG,
                  systimer, timer groups, interrupt matrix (per core), GPIO, USB-CDC,
                  UARTs, I2C, GDMA + I2S/LCD_CAM, RMT TX, regi2c, RTC WDT, WiFi MAC;
                  board/: atech14 / waveshare-cam / waveshare-lcd4b / waveshare-amoled18-v2 / none
  cli/            the `esp32sim` command line, every chip (`--chip`); `esp32sim-c3` / `esp32sim-c6` are alias binaries
  ── ESP32-C3 and ESP32-C6 (RISC-V, single core) ──
  riscv-rv32/     RV32IMAC decoder (sampled objdump checks), interpreter, disassembler
  esp32c3/        the C3 SoC: memory map, interrupt matrix, cache controller, RNG;
                  peripheral models shared with esp32s3 where the IP is identical
  esp32c6/        the C6 SoC: unified memory map and MMU, interrupt matrix + PLIC/INTPRI, L1 cache,
                  PCR, the LP blocks (reset cause, software reset, RTC timer), ASSIST_DEBUG,
                  the 802.15.4 and WiFi MACs
  tests/          golden-output regression tests and their fixtures (tests/README.md)
  ── shared ──
  wasm/           C-ABI crate wrapping all three Machines for the browser in one module
  web/            browser UI (board drawing, console, audio, camera) + emu.js/worker.js for wasm
  hw/             JTAG differential-test scripts against a real board, captured C3 and C6 consoles
  examples/       hello_world (IDF, S3, C3 and C6), waveshare-cam (autopling run script + photo),
                  wifi-station (S3), c6-wifi-station (builds for the S3, C3 and C6; the WiFi goldens'
                  firmware), c6-radio-dashboard (WiFi + 802.15.4 on the C6 board)
  tools/          PIE table generator (TRM-derived); bench.py (interleaved A/B benchmark);
                  wasm-build.sh (the WebAssembly module)
```

## Boards (ESP32-S3)

The SoC model emits pin-level events (GPIO edges, RMT symbol streams, I2S samples); a
`BoardModel` (`esp-soc/src/board.rs`) interprets them. `--board atech14` (default) is the Atech
14‑port board with its ST7735, WS2812 ring, encoder and buttons; `--board none` is a bare
module — any ESP32‑S3 firmware, console only; `--board waveshare-lcd4b` is the Waveshare
ESP32‑S3‑Touch‑LCD‑4B (ST7701S 480×480 over the LCD_CAM RGB bus, GT911 touch, TCA9554, codecs)
running the esp32-screen LVGL panel with touch; `--board waveshare-amoled18-v2` is the Waveshare
ESP32-S3-Touch-AMOLED-1.8 V2 (CO5300 368x448 over QSPI, CST820 touch); `--board waveshare-cam` is the Waveshare
ESP32‑S3‑CAM‑OV5640 (CH32V003 IO expander, OV5640 over SCCB, ES8311/ES7210 codecs on I2C0,
speaker on I2S1, OV5640 on the LCD_CAM DVP port) — runs the `waveshare-autopling` firmware
(IDF 5.5, 16 MB flash, 8 MB octal PSRAM: `--flash-mb 16 --psram-mb 8`) end to end: camera frames
from `--cam-image` or the browser (picture upload / webcam) → esp‑dl pedestrian detector on the
emulated PIE SIMD unit → pling on the ES8311. See `examples/waveshare-cam/`. Adding a board =
implementing the trait (`gpio_changes`, `rmt_frame`, `i2c_devices`, `camera_frame`, …).

The Pocket Synth firmware the `atech14` demos run is its own project:
[atech-firmware](https://github.com/joakimeriksson/atech-firmware) — a PlatformIO build of the SID synth and a cRSID
player for real `.sid` tunes. This repo carries the built images it produces, in
`web/wasm/fw/public/`.

## Run — ESP32-S3

```sh
cargo build --release
FW=web/wasm/fw/public                       # the demo images the goldens and the web page run
./target/release/esp32sim --boot rom \
    --bootloader $FW/atech-bootloader.bin --ptable $FW/atech-ptable.bin --app $FW/atech-firmware.bin \
    --script $FW/atech-script1.txt --wav out.wav --tft-png tft.png --max-seconds 5
```

The mask ROM ELF is picked up from `~/.espressif/tools/esp-rom-elfs/*/esp32s3_rev0_rom.elf`
(shipped with ESP‑IDF). Without ESP-IDF, `tools/fetch-demo-assets.sh --no-linux` puts the S3, C3 and
C6 ROMs in `web/wasm/fw/`: pass `--rom web/wasm/fw/esp32s3_rev0_rom.elf`, or set
`ESP32SIM_ROM_DIR=web/wasm/fw` for the tests. `--boot app` skips ROM+bootloader and loads the app
image directly.

A plain ESP‑IDF project, e.g. `examples/hello_world` (the IDF 5.5 get-started example built with
`idf.py set-target esp32s3 && idf.py build`):

```sh
H=examples/hello_world/build
./target/release/esp32sim --board none --boot rom --console uart0 \
    --bootloader $H/bootloader/bootloader.bin --ptable $H/partition_table/partition-table.bin \
    --app $H/hello_world.bin --elf $H/hello_world.elf --elf $H/bootloader/bootloader.elf --max-seconds 26
```

prints the ROM banner, the bootloader and app logs on UART0, `Hello world!`, the countdown, and
then reboots through the RTC watchdog exactly as silicon does (`rst:0xc (RTC_SW_CPU_RST)`),
~30× faster than real time. Chip resets (software, RTC watchdog) restart the machine from the
ROM with the right reset cause; `--no-reboot` stops at the first reset instead.

## Run — ESP32-C3

`esp32sim-c3` runs unmodified ESP-IDF firmware on the emulated RISC-V core, from the real mask
ROM through the real bootloader into FreeRTOS. Same flags as `esp32sim` (`--trace`, `--break`,
`--watch`, `--peek`, `--disasm`, `--log-periph`), printing RISC-V mnemonics with symbols.

```sh
H=examples/hello_world-c3/build
./target/release/esp32sim-c3 --boot rom --flash-mb 4 \
    --bootloader $H/bootloader/bootloader.bin --ptable $H/partition_table/partition-table.bin \
    --app $H/hello_world.bin --elf $H/hello_world.elf --max-seconds 26
```

prints the ROM banner, the bootloader log, `Hello world!` and the reboot, three times over.
Checked line-for-line against a physical C3 module: **205 of 208 console lines identical** over
three boot cycles — the difference is the ROM's `Saved PC:` line. `--mac`, `--reset-cause` and
`--strap` let a run adopt a board's identity so the comparison is meaningful. WiFi station mode
supports open and WPA2 virtual APs with DHCP and NAT. No boards; `--boot app` unsupported. See [docs/esp32c3.md](docs/esp32c3.md) for what works,
what does not, and the five emulator bugs the hardware found.

## Run — ESP32-C6

`esp32sim-c6` is the same front end on the ESP32-C6: RV32IMAC, the PLIC, a unified memory map.

```sh
H=examples/hello_world-c6/build
./target/release/esp32sim-c6 --boot rom --flash-mb 4 \
    --bootloader $H/bootloader/bootloader.bin --ptable $H/partition_table/partition-table.bin \
    --app $H/hello_world.bin --elf $H/hello_world.elf --max-seconds 27
```

Checked against a Waveshare ESP32-C6-LCD-1.47: **203 of 204 console lines identical** over three
boot cycles, including the ROM's `Saved PC:` after `esp_restart()`; the one difference is that
line on the first boot. `--board waveshare-c6-lcd147` adds that board — the ST7789 over SPI2 and
the GDMA, the WS2812, the BOOT button — and the C6's 802.15.4 MAC answers energy scans, so the
board's LVGL spectrum-scanner firmware runs end to end (`examples/waveshare-c6-lcd147/`). The MAC
also sends and receives frames with the timing of the air, and `--cooja` makes the C6 an external
mote of Cooja-NG, driven in exact lock-step over NDJSON (an unmodified Contiki-NG-on-IDF image
exchanging broadcasts with emulated MSP430 nodes). WiFi works too: `--wifi` attaches the same
virtual access point as on the S3 (see [WiFi](#wifi-esp32-s3-c3-and-c6) below). See
[docs/esp32c6.md](docs/esp32c6.md).

## Scripts (host actions at emulated time)

```
1.5  press btn1 150                       # GPIO17 low for 150 ms
2.2  knob cw 2                            # two quadrature detents on CLK5/DT4
3.4  press btn2 120
4.0  serial {"action":"set_note","value":"9"}   # into USB-CDC RX
5.0  stop
```

## WiFi (ESP32-S3, C3 and C6)

`--wifi ssid=NAME[,psk=PASS,chan=N,bssid=..]` attaches a virtual access point that the **unmodified**
Espressif WiFi blob associates with — scan, authentication, association and, with a passphrase, the
WPA2-PSK four-way handshake — and a virtual network behind it (DHCP, ARP, ICMP, DNS, SNTP off the
host clock). `--net nat` (the default) relays the guest's TCP and UDP over ordinary host sockets, so
firmware reaches the real network; `--net none` refuses it. No firmware changes, no root, no tun
device.

```sh
./target/release/esp32sim --board waveshare-lcd4b --boot rom --flash-mb 16 --psram-mb 8 \
    --bootloader $P/bootloader/bootloader.bin --ptable $P/partition_table/partition-table.bin \
    --app $P/energy_panel.bin --console usb --wifi "ssid=home,psk=secret" --max-seconds 45
```

runs the esp32-screen energy panel: it joins, takes a lease, syncs its clock, fetches two days of
electricity prices over **HTTPS** and polls a real Home Assistant on the LAN.
[docs/networking-howto.md](docs/networking-howto.md) is the how-to (flags, debugging, limits);
[docs/wifi-plan.md](docs/wifi-plan.md) and [docs/networking-plan.md](docs/networking-plan.md)
describe how the MAC model and the packet path work, and the Networking section of
[docs/architecture.md](docs/architecture.md#networking) the layers every chip shares.

The ESP32-C6 joins the same access point and the same network, through its own MAC model
([docs/wifi-c6-plan.md](docs/wifi-c6-plan.md)): one station, legacy rates, no power save.
`examples/c6-wifi-station` is the specimen — scan, join, lease, five pings, and the state on the
board's screen — and every C6 radio run needs `--stub bb_init=0` (the PHY's baseband calibration
wants analog hardware):

```sh
B=examples/c6-wifi-station/build-emu
./target/release/esp32sim-c6 --boot rom --flash-mb 4 --board waveshare-c6-lcd147 --console usb \
    --bootloader $B/bootloader/bootloader.bin --ptable $B/partition_table/partition-table.bin \
    --app $B/c6_wifi_station.bin --elf $B/c6_wifi_station.elf --stub bb_init=0 \
    --wifi ssid=esp32sim,psk=esp32sim-pass --max-seconds 14
```

The ESP32-C3 joins the same access point too, through a MAC model derived from the S3's. The
same station example, built for each chip without the display, is committed under
`web/wasm/fw/public/` as the input of the WiFi goldens (`wifi_station_{s3,c3,c6}` in
`cli/tests/goldens.rs`), so it runs without a local build:

```sh
P=web/wasm/fw/public
./target/release/esp32sim-c3 --boot rom --flash-mb 4 --console usb \
    --bootloader $P/c3-wifi-bootloader.bin --ptable $P/c3-wifi-ptable.bin --app $P/c3-wifi_station.bin \
    --wifi ssid=esp32sim,psk=esp32sim-pass --max-seconds 14
```

`examples/c6-radio-dashboard` runs WiFi and 802.15.4 energy scans in one firmware, a page each.

## In the browser (WebAssembly)

```sh
tools/wasm-build.sh                  # the module -> web/wasm/esp32sim.wasm
tools/fetch-demo-assets.sh           # mask ROMs, xterm.js and the Linux image (--no-linux skips its 16 MB)
python3 -m http.server -d web 8790   # then open http://127.0.0.1:8790/?wasm&fw=hello
```

The demo firmware is committed; the mask ROMs, xterm.js and the Linux image are not, and every demo
boots from the ROM, so run the fetch once. It gets the same files the Pages site serves.

The same emulator compiled to WebAssembly, running inside the page in a Web Worker: pick a board,
load the ROM ELF and firmware from disk (or `?wasm&fw=<name>` for a hosted manifest), press Boot.
hello_world, the Touch-LCD-4B panel with its SID player, the Atech board, the ESP32-C3 and the
ESP32-C6 all run at real time in Chrome; there is no NAT (the browser has no sockets). The S3 has
a WebAssembly JIT of its own (hot blocks and regions, PIE on WASM SIMD). See [docs/wasm.md](docs/wasm.md).
**Live: https://mikroverk.github.io/esp32sim/** — Linux 6.11 on the ESP32-S3
([svermigo/Linux-on-esp32-S3](https://github.com/svermigo/Linux-on-esp32-S3), log in and type on the
console), the Touch-LCD-4B panel with its SID player, the Atech board,
[pocket-tank](https://github.com/mediacutlet/pocket-tank) (a 4-bit transformer steering the fish on
the Touch-AMOLED-1.8), and the ESP32-C3 and C6
booting hello_world; or load your own firmware for any chip from disk (pick the board, or
`esp32c3` / `esp32c6`).

## Debugging

`--trace [--trace-from N]`, `--break ADDR`, `--watch ADDR` (stop when a word changes),
`--peek ADDR[,N]`, `--profile` (pc histogram), `--log-periph` (first touch of every
unknown register), `--stop-after-exceptions N`, `--gram-png` (raw ST7735 GRAM), `--no-jit`
(interpret instead of running native code — must give identical results).
Env: `ESP_EMU_DEBUG`, `ESP_EMU_DEBUG_SPI`, `ESP_EMU_DEBUG_USB`, `ESP_EMU_DEBUG_WIFI[_FRAMES]`,
`ESP_EMU_DEBUG_NET`, `ESP_EMU_DEBUG_AES`, `ESP_EMU_DEBUG_SHA`, `ESP_EMU_DEBUG_RSA`.

## Documentation

- [docs/adding.md](docs/adding.md) — adding a peripheral, a board with devices, a CPU/chip, an observer

`docs/` — [architecture](docs/architecture.md), [peripheral coverage](docs/peripherals.md),
[boards](docs/boards.md), [CLI reference](docs/cli.md), [web UI protocol](docs/web-ui.md),
[design decisions & gotchas](docs/decisions.md), [roadmap](docs/roadmap.md),
[networking how-to](docs/networking-howto.md), the [WiFi](docs/wifi-plan.md) and
[networking](docs/networking-plan.md) design notes, and the [testing](docs/testing-plan.md) plan.

## Provenance

Written from the ESP‑IDF register headers, the Xtensa core config shipped with ESP‑IDF, the
RISC-V specification and the C3 and C6 technical reference manuals, and observed firmware behaviour. QEMU was consulted only to confirm instruction semantics
(no code copied). MIT.

## Differential testing against real silicon (`hw/`)

Every chip is checked against hardware, by different methods: the S3 in **JTAG lock-step**
(below), the C3 and C6 by **diffing a captured console** against the emulator running the same
binaries (`hw/c3-hello-world-real.txt`, 205/208 lines identical — see
[docs/esp32c3.md](docs/esp32c3.md); `hw/c6-hello-world-real.txt`, 203/204 — see
[docs/esp32c6.md](docs/esp32c6.md)). Those comparisons found five emulator bugs each, including
a device-command race and a cycle counter that did not restart at reset — neither of which a
board-free test would have caught.

### ESP32-S3, JTAG lock-step

`DIFF_DIR=hw/<board> FLASH_MB=8 hw/difftest.sh 3000` — the scripts read efuses/strap from the
attached chip over JTAG, then step it and the emulator in lock-step on the same flash dump
(`flash-8M.bin` if present). Atech board, 2026-08-25: 3000 steps from reset, 0 divergences.

Any ESP32‑S3 board on USB works (its built‑in USB‑Serial/JTAG carries both the console
and JTAG). The flow, all scripted:

```sh
# one-time: dump the board's bootloader/partition table/app start (esptool) into hw/flash-0-1M.bin
hw/difftest.sh 8000                 # reset → single-step 8000 instructions on the chip and in the emulator, diff
hw/difftest-at.sh 403c8948 6000     # run both to a PC (here the 2nd-stage bootloader entry), then step 6000 and diff
```

`difftest*.sh` read the chip's efuses (`hw/efuse.txt`), strapping pins and the peripheral
reset state (`hw/reset-regs.txt`, dumped over JTAG at `reset halt`) and start the emulator
from the same image and state. `hw/compare.py` diffs `pc a0..a15 ps windowbase` per
instruction, masks `PS.INTLEVEL` (forced during single‑step), hides window‑exception
handlers (the debugger steps over them atomically) and resynchronises across CCOUNT‑timed
delay loops, which iterate a different number of times when each step takes milliseconds.

Result so far: the ROM reset path (8000 steps) and 3000+ steps of the IDF 5.5 bootloader
run with zero PC divergence; remaining register differences are RTC‑domain values that
depend on the previous boot.

## Live board UI (ESP32-S3)

```sh
./target/release/esp32sim --boot rom --bootloader … --ptable … --app … --web 8766
# open http://127.0.0.1:8766/
```

`--web PORT` runs the emulator in real time and serves `web/run.html`: the 14‑port board
(knob + LED ring, buttons, speaker VU, the ST7735 in its physical orientation plus a readable
copy), USB‑CDC and UART0 consoles, an action box for the SDK JSON protocol, and audio through
WebAudio. Inputs: click the buttons, wheel/drag/←→ on the knob, click the cap to push it.

## Performance / real time (ESP32-S3)

Blocks of instructions are decoded once and executed from a cache, then compiled to native
AArch64 code (`--no-jit` falls back to the interpreter and must give identical results); idle
cores (`waiti`) are skipped, device time is delivered lazily against computed timer deadlines,
and interrupt lines are recomputed only when a source changes. See
[docs/speed-plan.md](docs/speed-plan.md) for the measurements behind each step.
The Pocket Synth firmware runs at real time with margin while idle or playing notes; the one
place it cannot keep up is a full display redraw, where core 1 runs 100 % busy bit‑banging SPI
(4.8 M instructions per 20 ms — 240 Minsn/s needed, ~70 Minsn/s achieved on an M‑series
MacBook). Each redraw therefore costs ~0.3 s of lag, which the UI's adaptive audio buffer
absorbs (it grows on underrun, up to 400 ms) and the pacer recovers afterwards; if the
emulator ever falls more than 0.5 s behind it resynchronises instead of bursting. The header
shows `real time`, `⚠ N s behind` and the resync count.

`ESP_EMU_RT_LOG=1` prints every 20 ms window that took > 40 ms wall with both cores'
instruction counts and PCs. `hw/wsdrive.py [port] [seconds]` drives the UI protocol without a
browser (button presses + knob turns) and reports push gaps, lag and audio delivered — use it
to measure changes to the scheduler.

## Contributing

Contributions are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) lists the checks a pull request needs
(the same ones CI runs) and the conventions for tests, experiments and evidence.

## License

esp32sim is released under the [MIT License](LICENSE), copyright Joakim Eriksson
([@joakimeriksson](https://github.com/joakimeriksson)) and Alice
([@aliceisjustplaying](https://github.com/aliceisjustplaying)).

Third-party code, data and demo files keep their own licenses:

- **pocket-tank** ([mediacutlet/pocket-tank](https://github.com/mediacutlet/pocket-tank), MIT). Its
  bootloader, partition table and app are committed under `web/wasm/fw/public/` with
  `pocket-tank-LICENSE.txt`; its model is fetched.
- **Mask ROM ELFs** ([espressif/esp-rom-elfs](https://github.com/espressif/esp-rom-elfs), Apache-2.0),
  fetched, not committed.
- **xterm.js** ([xtermjs/xterm.js](https://github.com/xtermjs/xterm.js), MIT), fetched, not committed.
- **Linux image** ([svermigo/Linux-on-esp32-S3](https://github.com/svermigo/Linux-on-esp32-S3),
  GPL-3.0), fetched, not committed.
- **ESP-IDF ADC calibration data** (Espressif, Apache-2.0): the curve-fitting coefficients
  and S3 ADC1 calibration from #165, now together in `esp-periph/src/sar_adc.rs`;
  see `esp-periph/LICENSE-ADC`.
