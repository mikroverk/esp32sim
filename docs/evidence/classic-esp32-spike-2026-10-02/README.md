# Classic ESP32 ECO3 boot spike (EX199)

## Scope and result

This spike adds the smallest classic ESP32 target that reuses the existing Xtensa core,
SoC runner, and shared peripheral models. It tests functional boot correctness, not speed
or cycle accuracy. It differs materially from [EX008](../../experiments.md#ex008) and
[EX010](../../experiments.md#ex010): the target is the dual-core Xtensa LX6 ESP32-D0WD
ECO3, the input is its real rev-3 mask ROM plus an unchanged Arduino-ESP32 3.3.8 image,
and the contract includes ROM boot, second-stage bootloader and application GPIO output.

All three bounded milestones passed at code-under-test revision `d13362f`:

- A: the rev-3 mask ROM printed its banner, selected SPI fast-flash boot, and loaded the
  second-stage bootloader from the merged image's `0x1000` segment.
- B: the bootloader loaded the application image described by the partition table at
  `0x8000`, then execution reached `esp_startup_start_app` (`0x400f0f68`), `main_task`
  (`0x400f0ef8`), and `app_main` (`0x400d4b74`).
- C: the unchanged sketch printed `CLASSIC_BOOT` and `tick 0`; the GPIO VCD recorded
  GPIO2 rising at 18,664,800,000 ps of modeled time.

The requested example banner date, `ets Jun  8 2016`, belongs to an older ROM. The
caller-supplied ECO3 `esp32_rev300_rom.elf` correctly printed `ets Jul 29 2019 12:21:46`.

## Revisions and inputs

- Base: upstream `dddb128`.
- Implementation revisions: `3b15629` (ROM boot) and `d13362f` (Arduino application).
- QEMU behavior reference: Espressif QEMU `esp-develop` at
  `febae182fb7abb8e76a738a8b6a174b61ca627643`; read only. No GPL source was copied.
- Host tools: Rust/Cargo 1.96.0, PlatformIO Core 6.1.19, Darwin arm64, macOS 26.6.2
  build 25G83.
- Firmware platform: `platform-espressif32` `55.3.38+sha.fbdfc29`, Arduino-ESP32 3.3.8.

SHA-256 inputs and artifacts:

| Item | SHA-256 |
| --- | --- |
| `$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf` | `920b70635440517866aab2230964a570d2cf2b676658d93c52fbac108c1cca31` |
| `/tmp/esp32sim-classic-pio/platformio.ini` | `940f177931f32bc257b058a53dae12fddabb70ec3a80ea4b0e70aa0b4e55aa28` |
| `/tmp/esp32sim-classic-pio/src/main.cpp` | `8ff68737b709d4b704bed48a5965a1eccd9cb1fc8626bb4d569f674ae140ff0e` |
| `bootloader.bin` | `a227e3ab93f15f1155efb3144810cc06ff7a259e9bc9b4542417afcc6c238214` |
| `partitions.bin` | `148b959cbff1c38aa8e1d5c0ba9d612c54997b945e56a63f41223eef650653a1` |
| `firmware.bin` | `cf4632577191a5292e54677bfb782ea9bb83030e8ce71536326d13b858bd2e85` |
| `firmware.elf` | `d19b529a333d47db826bdc3c25c90ecad7b3ab6e5e0fa42e3ba765d854d35917` |
| `firmware.factory.bin` | `b0361619b04087f666fae5c5b7d6a00536062da1dd969d77f5171639f7d701a9` |

The temporary PlatformIO project used this configuration:

```ini
[env:esp32dev]
platform = https://github.com/pioarduino/platform-espressif32.git#55.03.38-1
board = esp32dev
framework = arduino
monitor_speed = 115200
```

Its `src/main.cpp` was the sketch specified for the spike. Build it with:

```sh
cd /tmp/esp32sim-classic-pio
pio run
```

The platform build emitted `firmware.factory.bin`, a 4 MiB merged image containing
`bootloader.bin` at `0x1000`, `partitions.bin` at `0x8000`, the framework's boot-app
selection image at `0xe000`, and `firmware.bin` at `0x10000`.

## Reproduction and output checks

From the repository root:

```sh
cargo build --release
target/release/esp32sim --chip esp32 --boot rom \
  --rom "$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf" \
  --flash-image /tmp/esp32sim-classic-pio/.pio/build/esp32dev/firmware.factory.bin \
  --elf /tmp/esp32sim-classic-pio/.pio/build/esp32dev/firmware.elf \
  --max-insns 20000000 --no-jit --no-reboot --no-dump \
  --vcd /tmp/esp32-classic-gpio.vcd
```

The UART transcript was:

```text
ets Jul 29 2019 12:21:46

rst:0x1 (POWERON_RESET),boot:0x13 (SPI_FAST_FLASH_BOOT)
configsip: 0, SPIWP:0xee
clk_drv:0x00,q_drv:0x00,d_drv:0x00,cs0_drv:0x00,hd_drv:0x00,wp_drv:0x00
mode:DIO, clock div:2
load:0x3fff0030,len:4640
load:0x40078000,len:15660
load:0x40080400,len:3164
entry 0x4008059c
CLASSIC_BOOT
tick 0
[emu] stop: MaxInsns — core0 4545698 + core1 825493 insns in 0.1s wall = 63.6 Minsn/s; emulated 0.083s (20000000 cycles); 3767 exceptions, 238 interrupts
[vcd] wrote 447 events to /tmp/esp32-classic-gpio.vcd
```

The VCD output contained:

```text
$var wire 1 s2 gpio2 $end
#18664800000
1s2
```

The bounded run stops before the sketch's 500 ms delay expires, so one rising edge and
the first serial line are the intended C check. It does not establish later periodicity.

Final gates on the documented source tree all passed:

```sh
cargo test --workspace
cargo build --release
tools/wasm-build.sh
node tools/check-evidence-privacy.mjs
```

The workspace run covered the new classic crate and the existing S3, C3, C6, shared
peripheral, shared SoC, CLI, and WASM tests. Tests that require external firmware remain
explicitly ignored by their existing contracts.

## Modeled behavior and known gaps

The target models the classic memory/cache windows, both MMU tables, mask ROM and SRAM,
SPI0/1 flash commands, three UARTs and their FIFO aliases, basic GPIO, boot strap `0x13`,
partial RTC/TIMG watchdog behavior, ECO3 eFuse identity, SHA-256, random input, APP CPU
control, and DPORT CPU-to-CPU interrupt sources 24 through 27. Other peripheral blocks
use the existing round-trip register RAM: reads start at zero and writes persist, but
there is no device behavior or interrupt generation.

The boot run first touched these register-RAM stubs:

- IO_MUX: `0x3ff49088`, `0x3ff49060`, `0x3ff49064`, `0x3ff49068`, `0x3ff49054`,
  `0x3ff49058`, `0x3ff4905c`, `0x3ff49084`, and `0x3ff49040`.
- SYSCON: `0x3ff6607c`, `0x3ff66000`, `0x3ff66008`, `0x3ff66004`,
  `0x3ff6602c`, `0x3ff66030`, `0x3ff66034`, `0x3ff66038`, `0x3ff66010`, and
  `0x3ff66018`.
- Unnamed register blocks: `0x3ff4f0b0`, `0x3ff4f0a8`, `0x3ff4f008`,
  `0x3ff4f0ac`, and `0x3ff6d0ac`.

The shared LX7-named core matches the LX6 run's 64 physical registers, windowed ABI,
32 interrupt inputs and levels, exception vectors, CCOUNT/three CCOMPARE timers,
loops, MAC16, multiply/divide, and scalar single-precision FPU. Classic configuration
IDs are reported. The principal ISA gap is LX6 double-precision accelerator arithmetic:
the core now preserves F64R_LO/F64R_HI/F64S user-register state because FreeRTOS restores
it, but does not execute the accelerator operations. An external objdump comparison over
217,585 decoded ROM/application instructions found 1,020 mismatches, dominated by
`f64addc1`, `f64cmph91`, `f64cmpl4`, `f64iter27`, `f64norm13`, `f64rnd24`,
`f64sexp2`, and `f64subc22`; `lsi` had 830 mismatches in 1,245 instances because its
encoding overlaps those LX6-only instructions. LX7 PIE remains irrelevant to this LX6
target. LX6 has a three-byte maximum instruction length; LX7 PIE can use four bytes.

## Negative and diagnostic results

- An attempted separate merge named `boot_app0.bin` failed with `No such file or
  directory`; the produced `firmware.factory.bin` already contains that segment and was
  used instead.
- One direct `esptool image_info` attempt failed with `ModuleNotFoundError:
  rich_click`; image offsets were checked through the successful PlatformIO build and
  ROM boot instead.
- A later supplemental `pio pkg list` failed because the sandbox could not initialize
  `$HOME/.platformio/.cache/uv`. The earlier successful build had already reported the
  platform and framework versions.
- Before preserving LX6 F64 user registers, `wur.f64r_lo` at `0x4008f497` entered the
  double-exception vector. Before DPORT CPU-to-CPU interrupts were connected, FreeRTOS
  did not schedule the application task. Before the UART FIFO aliases were mapped, the
  store at `uart_hal_write_txfifo` (`0x400d9036` to `0x60000000`) raised
  StoreProhibited. Each root cause was corrected in the implementation revisions above.

## Evidence curation

No raw disassembly, build tree, VCD, private capture, or backup is committed. They remain
temporary and can be reproduced by the commands above. User and host identifiers were
removed; home paths are normalized to `$HOME`. This removes no measured values, input
hashes, source revisions, or correctness observations. The repository privacy checker
and manual review cover this retained Markdown receipt.
