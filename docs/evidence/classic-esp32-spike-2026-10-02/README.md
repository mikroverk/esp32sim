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

## GPIO and interrupt-routing extension

Revision `d64ac6e` extends EX199 rather than starting another experiment. The target,
ROM, Arduino version and functional workload are unchanged. The material difference is
the correctness contract: GPIO pad configuration, matrix routing, external edges and
peripheral interrupts must now reach the running application through the classic DPORT
matrix. This remains functional evidence, not a speed or cycle-accuracy claim.

The added register tests cover GPIO output and enable aliases, matrix output selection,
output and output-enable inversion, constant and pad input selection, IO_MUX input,
pull-up and function selection, the GPIO34 input-only rule, falling and level interrupt
status/clear behavior, both DPORT CPU maps, and delivery from GPIO, UART, timer, timer
watchdog and RTC watchdog sources. They also cover the classic UART FIFO pointer status
used by Arduino's receive path.

The temporary validation sketch was built outside the repository with the same
`platformio.ini` shown above. Its source was:

```cpp
#include <Arduino.h>

volatile unsigned edges;

void IRAM_ATTR on_falling() {
  ++edges;
}

void setup() {
  Serial.begin(115200);
  pinMode(4, INPUT_PULLUP);
  attachInterrupt(4, on_falling, FALLING);
  Serial.printf("initial=%d\n", digitalRead(4));
}

void loop() {
  static unsigned sample;
  Serial.printf("sample=%u level=%d edges=%u\n", sample++, digitalRead(4), edges);
  while (Serial.available()) {
    Serial.printf("rx=%02x\n", Serial.read());
  }
  delay(100);
}
```

The CLI script used the existing S3-compatible host-input mechanism:

```text
0.20 gpio 4 0
0.30 gpio 4 1
0.40 gpio 4 0
0.50 gpio 4 1
0.60 serial Z
```

SHA-256 inputs and temporary artifacts:

| Item | SHA-256 |
| --- | --- |
| `/tmp/esp32sim-classic-gpio-validation/platformio.ini` | `b86c69259be417474b2dfef705f80db5720beff2f80b471ef5762f2a65651500` |
| `/tmp/esp32sim-classic-gpio-validation/src/main.cpp` | `1c18cb54dc1929aafa5fefcbdbb3a89a96b8cec72562b9fc8f21d65ee1347593` |
| `firmware.factory.bin` | `d4838cbfd862e6b1bdb9a96d8d43c3e9ccc4960b510f5ed541cb5509b7653844` |
| `firmware.elf` | `f31527bca844cf90e1569c23870a1496203b7c64e740a8ac437cb232f9028101` |
| `/tmp/esp32-classic-gpio.script` | `37b2579dca5c9c2a9fb45050d5a0557082e558e694d569b120f31c8c493bf38f` |
| GPIO validation VCD | `c24e93dbfa30dadaf0a2e401ae45389512ca5dc0c4d24295f59ae8707348e077` |
| Blink validation VCD | `b7ef6b9fd286854bf82708e66d16f31f456e89077341ac4c23d2839e7e523e08` |

Build and run commands were:

```sh
cd /tmp/esp32sim-classic-gpio-validation
pio run

# From the repository root:
cargo build --release
target/release/esp32sim --chip esp32 --boot rom \
  --rom "$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf" \
  --flash-image /tmp/esp32sim-classic-gpio-validation/.pio/build/esp32dev/firmware.factory.bin \
  --elf /tmp/esp32sim-classic-gpio-validation/.pio/build/esp32dev/firmware.elf \
  --max-seconds 1.2 --no-reboot --no-dump \
  --script /tmp/esp32-classic-gpio.script \
  --vcd /tmp/esp32-classic-gpio-validation.vcd
```

The relevant UART output was:

```text
initial=1
sample=1 level=1 edges=0
[script] t=0.200s Gpio(4, false)
sample=2 level=0 edges=1
[script] t=0.300s Gpio(4, true)
sample=3 level=1 edges=1
[script] t=0.400s Gpio(4, false)
sample=4 level=0 edges=2
[script] t=0.500s Gpio(4, true)
sample=5 level=1 edges=2
[script] t=0.600s Serial("Z\n")
rx=5a
rx=0a
[emu] stop: Halted; emulated 1.200s (288000000 cycles)
```

The GPIO VCD contains the externally driven levels at exactly 0.2, 0.3, 0.4 and
0.5 modeled seconds:

```text
#200000000000 0s4
#300000000000 1s4
#400000000000 0s4
#500000000000 1s4
```

The unchanged blink artifact documented earlier was rerun for 2.2 modeled seconds. It
printed five `tick 0` lines and recorded GPIO2 changes at 0.518145, 1.018151, 1.518157
and 2.018163 seconds after its initial transition. This proves that `delay(500)` and the
FreeRTOS timer interrupt continue past two seconds. `millis()` still reports zero because
the classic high-resolution timer/latch path is not modeled; that limitation is retained.

At `d64ac6e`, these commands passed:

```sh
cargo test -p esp32
cargo test --workspace
cargo build --release
tools/wasm-build.sh
node tools/check-evidence-privacy.mjs
```

The workspace run includes the existing S3, C3 and C6 suites. The privacy checker
reported 1,472 tracked evidence files checked and no configured patterns found; manual
review found no retained user name, host name, device identifier or unrelated process
data in this extension.

## I2C controller extension

Revision `caabeb7` extends EX199 again. It keeps the classic ECO3 target, mask ROM and
Arduino-ESP32 3.3.8 platform. The mechanism and correctness contract differ from the
earlier GPIO work. Both classic I2C controllers now execute the classic command encoding,
use their FIFO aliases and DPORT interrupt sources, resolve SDA and SCL through the
classic GPIO matrix, and transact with the existing board-device models. The workload is
an I2C scanner, a repeated-start register read, and an address-NACK check. This is
functional evidence, not a bus-timing or execution-speed measurement.

The implementation reuses the shared I2C command engine and adds only its classic layout:
16 command registers and the older RSTART, READ, and STOP opcode values. The classic adapter
maps I2C0 at `0x3ff53000`, I2C1 at `0x3ff67000`, the APB FIFO write aliases at
`0x6001301c` and `0x6002701c`, GPIO-matrix SCL and SDA signals 29, 30, 95, and 96, and
DPORT sources 49 and 50. The model handles START, STOP, repeated START, address and data
ACK and NACK, and 32-byte transmit and receive FIFOs. It raises END_DETECT,
TRANS_COMPLETE, NACK, and TIMEOUT interrupts. A transaction times out if either routed
input is absent or low when `TRANS_START` is written.

Classic `--board` accepts the existing S3 board names for their reusable board-device
models. The validation used `waveshare-amoled18-v2`, whose I2C0 devices are CST820 at
`0x15`, TCA9554 at `0x20`, AXP2101 at `0x34`, PCF85063A at `0x51`, and QMI8658 at
`0x6b`. Device attachment is repeated after a chip reset.

Register-level tests cover both controllers, all 16 classic command slots, FIFO aliases,
END and STOP completion, repeated-start register reads, address NACK, held-SCL timeout,
GPIO-matrix input and output hooks, PRO DPORT delivery from sources 49 and 50, board-device
attachment and reset reattachment. At the implementation revision, these touched-crate
checks passed:

```text
cargo test -p esp-periph -p esp32
  esp-periph: 59 passed; esp32: 14 passed; 0 failed

cargo test -p esp32sim
  28 passed; 0 failed; 15 external-firmware tests ignored by their contracts
```

The temporary PlatformIO project used the same `platformio.ini` as the earlier checks.
Its fixed sketch was:

```cpp
#include <Arduino.h>
#include <Wire.h>

void setup() {
  Serial.begin(115200);
  Wire.begin(21, 22);

  unsigned found = 0;
  for (uint8_t address = 1; address < 127; ++address) {
    Wire.beginTransmission(address);
    if (Wire.endTransmission() == 0) {
      Serial.printf("found=0x%02x\n", address);
      ++found;
    }
  }
  Serial.printf("count=%u\n", found);

  Wire.beginTransmission(0x6b);
  Wire.write(0x00);
  uint8_t write_error = Wire.endTransmission(false);
  uint8_t received = Wire.requestFrom(0x6b, static_cast<uint8_t>(1));
  int who_am_i = received ? Wire.read() : -1;
  Serial.printf("qmi write=%u received=%u who=0x%02x\n", write_error, received, who_am_i);

  Wire.beginTransmission(0x7e);
  Serial.printf("missing=%u\n", Wire.endTransmission());
}

void loop() {
  delay(1000);
}
```

Build and run commands were:

```sh
cd /tmp/esp32sim-classic-i2c-validation
pio run

# From the repository root:
cargo build --release
target/release/esp32sim --chip esp32 --boot rom \
  --rom "$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf" \
  --flash-image /tmp/esp32sim-classic-i2c-validation/.pio/build/esp32dev/firmware.factory.bin \
  --elf /tmp/esp32sim-classic-i2c-validation/.pio/build/esp32dev/firmware.elf \
  --board waveshare-amoled18-v2 \
  --max-seconds 1.2 --no-reboot --no-dump
```

PlatformIO reported platform `55.3.38+sha.fbdfc29`, Arduino-ESP32 3.3.8 and framework
libraries `5.5.4+sha.735507283d`. The relevant UART output was:

```text
found=0x15
found=0x20
found=0x34
found=0x51
found=0x6b
count=5
qmi write=0 received=1 who=0x05
missing=2

[emu] stop: Halted; core0 6595449 + core1 2857161 insns;
emulated 1.200s (288000000 cycles); 14419 exceptions, 2600 interrupts
```

The five scanner hits exactly match the attached board devices. The QMI8658 WHO_AM_I
register returned `0x05` after a no-STOP write and repeated-start read. Arduino Wire
returned error 2, address NACK, for unattached address `0x7e`.

SHA-256 inputs and temporary artifacts:

| Item | SHA-256 |
| --- | --- |
| `$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf` | `920b70635440517866aab2230964a570d2cf2b676658d93c52fbac108c1cca31` |
| `/tmp/esp32sim-classic-i2c-validation/platformio.ini` | `b86c69259be417474b2dfef705f80db5720beff2f80b471ef5762f2a65651500` |
| `/tmp/esp32sim-classic-i2c-validation/src/main.cpp` | `a0b0a9b1a56bc27034bba45bef996ee18f8d2ce1fc07f3969ff4b494539bd9ed` |
| `bootloader.bin` | `a227e3ab93f15f1155efb3144810cc06ff7a259e9bc9b4542417afcc6c238214` |
| `partitions.bin` | `148b959cbff1c38aa8e1d5c0ba9d612c54997b945e56a63f41223eef650653a1` |
| `firmware.bin` | `b329a331ea762f27f7d5ccc0e44363aa0befcd641c6858a92acd4a7f5eaaec7c` |
| `firmware.factory.bin` | `1f2266ba4b66a14393eba45dc7b093a5d94bc07d105609c9aee12afa773aff69` |
| `firmware.elf` | `bf76bbc3bf99ea7b1a47a0b09f0616938a548dcd1dbfb5558a1ea4571539e642` |

Host tools were Rust and Cargo 1.96.0 and PlatformIO Core 6.1.19 on Darwin arm64, macOS
26.6.2 build 25G83. The first sandboxed PlatformIO build failed with
`PermissionError: [Errno 1] Operation not permitted: '$HOME/.platformio/platforms.lock'`;
repeating it with access to the existing package cache succeeded. A repository-wide
`cargo fmt --check` exited 1 on pre-existing formatting differences. The first difference
was at `cli/src/bin/esp32sim-c3.rs:1`. The command changed no files and was not a requested
gate.

The controller executes each command list atomically when `TRANS_START` is written. It
does not generate bit-level SDA and SCL waveforms, model clock-stretch duration, arbitration,
10-bit addressing or slave mode. The GPIO matrix must resolve both inputs high, and its
output hooks hold the open-drain lines released between transactions. Board devices are
functional transaction models rather than electrical bus models.

The final source tree passed `cargo build --release`, touched-crate tests,
`cargo test --workspace`, `tools/wasm-build.sh`, `git diff --check`, and
`node tools/check-evidence-privacy.mjs`. Existing tests that require external firmware
remained ignored by their stated contracts. The privacy check and a manual review found
no retained login name, host name, device identifier, unrelated command line, raw capture
or backup. Temporary paths use generic names and home paths are normalized to `$HOME`.

## Modeled behavior and known gaps

The target models the classic memory/cache windows, both MMU tables, mask ROM and SRAM,
SPI0/1 flash commands, three UARTs and their FIFO aliases, boot strap `0x13`, partial
RTC/TIMG watchdog behavior, ECO3 eFuse identity, SHA-256, random input and APP CPU
control. I2C0 and I2C1 execute classic master command lists against attached board devices and
deliver their interrupts through DPORT. GPIO0-39 include output/enable aliases, pad input,
supported pulls, matrix input/output routing and PRO/APP edge and level interrupts;
GPIO34-39 remain input-only.
DPORT routes GPIO, UART0-2, TIMG0/1 timer and watchdog, RTC watchdog and CPU-to-CPU
sources through the per-core maps. Other peripheral blocks use the existing round-trip
register RAM: reads start at zero and writes persist, but there is no device behavior or
interrupt generation.

Direct IO_MUX function selection bypasses the GPIO matrix, but direct peripheral pad
waveforms are not generated until those peripherals are modeled. The matrix exposes
classic-local input and output hooks for that later work. Watchdog interrupt actions are
modeled; watchdog reset actions are not. External GPIO drive is an absolute host level,
matching the existing script API; there is no separate release-to-pull command.

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
- The validation sketch rebuild initially failed with `PermissionError: [Errno 1]
  Operation not permitted: '$HOME/.platformio/platforms.lock'`. Repeating it with
  explicit access to the existing PlatformIO package cache succeeded.
- The first Serial-input run returned the injected bytes followed by zero padding.
  Arduino reads the classic `UART_MEM_RX_STATUS` FIFO pointers at offset `0x60`; the
  shared newer-chip UART layout returned storage there. The classic adapter now reports
  pointers from the actual FIFO depth, and the rerun returned exactly `5a` and `0a`.
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
