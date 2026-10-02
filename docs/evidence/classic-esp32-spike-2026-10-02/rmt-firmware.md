# Classic RMT firmware inputs for EX199

This extends [EX199](../../experiments.md#ex199) with the classic RMT register, GPIO
matrix, TX interrupt and WS2812 output correctness contract. The target, ECO3 ROM and
pinned Arduino toolchain remain the same as the earlier GPIO/I2C/LEDC/timer/SPI
extensions. The [main receipt](README.md) records the implementation, measured output
and remaining limits. This file preserves the firmware inputs and expected checks.

## Toolchain

- PlatformIO Core 6.1.19; platform 55.3.38+sha.fbdfc29.
- Arduino-ESP32 3.3.8; framework libraries 5.5.4+sha.735507283d.
- Xtensa toolchain 14.2.0+20260121; esptool 5.2.0.
- Adafruit NeoPixel 1.15.5, pinned after initial resolution.

## Builds and expected output

All builds succeeded using the existing PlatformIO package cache. No firmware build
failed. The NeoPixel project was rebuilt once after replacing the unpinned dependency
with its resolved version, 1.15.5.

### Arduino rgbLedWrite

```ini
[env:esp32dev]
platform = https://github.com/pioarduino/platform-espressif32.git#55.03.38-1
board = esp32dev
framework = arduino
monitor_speed = 115200
```

```cpp
#include <Arduino.h>

void setup() {
  Serial.begin(115200);
  rgbLedWrite(4, 255, 0, 64);
  Serial.printf("RGB_LED gpio=4 rgb=255,0,64 completed=%u\n", rmtTransmitCompleted(4));
}

void loop() { delay(1000); }
```

```sh
cd /tmp/esp32sim-classic-rmt-rgb
pio run > /tmp/esp32-classic-rmt-rgb-build.log 2>&1
```

Run from the repository root. The CLI reports RMT and decoded LED output without an
extra observer flag:

```sh
target/release/esp32sim --chip esp32 --boot rom \
  --rom "$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf" \
  --flash-image /tmp/esp32sim-classic-rmt-rgb/.pio/build/esp32dev/firmware.factory.bin \
  --elf /tmp/esp32sim-classic-rmt-rgb/.pio/build/esp32dev/firmware.elf \
  --max-seconds 0.8 --no-reboot --no-dump \
  > /tmp/esp32-classic-rmt-rgb-run.log 2>&1
```

| Artifact | SHA-256 |
| --- | --- |
| `/tmp/esp32sim-classic-rmt-rgb/platformio.ini` | `b86c69259be417474b2dfef705f80db5720beff2f80b471ef5762f2a65651500` |
| `/tmp/esp32sim-classic-rmt-rgb/src/main.cpp` | `30876670eb7942992097d1836039082b3389601e4d627ed74c60c57ab1283f5a` |
| `/tmp/esp32sim-classic-rmt-rgb/.pio/build/esp32dev/firmware.factory.bin` | `72f3c954b92ce886992066e197747f9fd31791fd09f3dea254a095849bbb2092` |
| `/tmp/esp32sim-classic-rmt-rgb/.pio/build/esp32dev/firmware.elf` | `2a24ec88910bce3c9e8aa1c3dd01ce391748d1114723edeaef1090275fa29cc0` |
| `/tmp/esp32sim-classic-rmt-rgb/.pio/build/esp32dev/firmware.bin` | `934d8281704de3ef17651e87d7fda8877958385a275abafad11033f9f20ea854` |

### Adafruit NeoPixel

```ini
[env:esp32dev]
platform = https://github.com/pioarduino/platform-espressif32.git#55.03.38-1
board = esp32dev
framework = arduino
monitor_speed = 115200
lib_deps = adafruit/Adafruit NeoPixel@1.15.5
```

```cpp
#include <Arduino.h>
#include <Adafruit_NeoPixel.h>

Adafruit_NeoPixel strip(8, 5, NEO_GRB + NEO_KHZ800);
const uint8_t colors[8][3] = {
  {255, 0, 0}, {0, 255, 0}, {0, 0, 255}, {255, 255, 255},
  {1, 2, 3}, {17, 34, 51}, {255, 0, 64}, {0, 0, 0}
};

void setup() {
  Serial.begin(115200);
  strip.begin();
  for (unsigned i = 0; i < 8; ++i)
    strip.setPixelColor(i, strip.Color(colors[i][0], colors[i][1], colors[i][2]));
  strip.show();
  Serial.println("NEOPIXEL gpio=5 count=8 ff0000 00ff00 0000ff ffffff 010203 112233 ff0040 000000");
}

void loop() { delay(1000); }
```

```sh
cd /tmp/esp32sim-classic-rmt-neopixel
pio run > /tmp/esp32-classic-rmt-neopixel-pinned-build.log 2>&1
```

Run from the repository root. The CLI reports RMT and decoded LED output without an
extra observer flag:

```sh
target/release/esp32sim --chip esp32 --boot rom \
  --rom "$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf" \
  --flash-image /tmp/esp32sim-classic-rmt-neopixel/.pio/build/esp32dev/firmware.factory.bin \
  --elf /tmp/esp32sim-classic-rmt-neopixel/.pio/build/esp32dev/firmware.elf \
  --max-seconds 0.8 --no-reboot --no-dump \
  > /tmp/esp32-classic-rmt-neopixel-run.log 2>&1
```

| Artifact | SHA-256 |
| --- | --- |
| `/tmp/esp32sim-classic-rmt-neopixel/platformio.ini` | `b58d4a7ff32e86080f59cedaf7327086b95f1b9b34456c1a12b0291ee5fd3f90` |
| `/tmp/esp32sim-classic-rmt-neopixel/src/main.cpp` | `3a8bb338f2a4c379e68f847540922a3569cdde671cc8498dde1a14fe55ec910b` |
| `/tmp/esp32sim-classic-rmt-neopixel/.pio/build/esp32dev/firmware.factory.bin` | `1c71830ebd419bba692dc61f9861d4c06c26b655cc43d7cbcea039ecc64e2c86` |
| `/tmp/esp32sim-classic-rmt-neopixel/.pio/build/esp32dev/firmware.elf` | `57b26a17821e59be2977cfd20e70829ad6cb6295d4fdf4cda1aa484e27a3f380` |
| `/tmp/esp32sim-classic-rmt-neopixel/.pio/build/esp32dev/firmware.bin` | `a861d137dd994a6cd1e11ef2e5a9832796be10b053ae58b23c1cd3cbc6ca6243` |

### Arduino raw rmtWrite

```ini
[env:esp32dev]
platform = https://github.com/pioarduino/platform-espressif32.git#55.03.38-1
board = esp32dev
framework = arduino
monitor_speed = 115200
```

```cpp
#include <Arduino.h>

void setup() {
  Serial.begin(115200);
  rmt_data_t items[3] = {};
  items[0].level0 = 1; items[0].duration0 = 10;
  items[0].level1 = 0; items[0].duration1 = 20;
  items[1].level0 = 1; items[1].duration0 = 30;
  items[1].level1 = 0; items[1].duration1 = 40;
  items[2].level0 = 0; items[2].duration0 = 50;
  items[2].level1 = 1; items[2].duration1 = 60;
  bool initialized = rmtInit(18, RMT_TX_MODE, RMT_MEM_NUM_BLOCKS_1, 1000000);
  bool wrote = initialized && rmtWrite(18, items, RMT_SYMBOLS_OF(items), 1000);
  Serial.printf("RMT_RAW gpio=18 hz=1000000 init=%u write=%u completed=%u\n",
                initialized, wrote, rmtTransmitCompleted(18));
}

void loop() { delay(1000); }
```

```sh
cd /tmp/esp32sim-classic-rmt-raw
pio run > /tmp/esp32-classic-rmt-raw-build.log 2>&1
```

Run from the repository root. The CLI reports RMT and decoded LED output without an
extra observer flag:

```sh
target/release/esp32sim --chip esp32 --boot rom \
  --rom "$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf" \
  --flash-image /tmp/esp32sim-classic-rmt-raw/.pio/build/esp32dev/firmware.factory.bin \
  --elf /tmp/esp32sim-classic-rmt-raw/.pio/build/esp32dev/firmware.elf \
  --max-seconds 0.8 --no-reboot --no-dump \
  > /tmp/esp32-classic-rmt-raw-run.log 2>&1
```

| Artifact | SHA-256 |
| --- | --- |
| `/tmp/esp32sim-classic-rmt-raw/platformio.ini` | `b86c69259be417474b2dfef705f80db5720beff2f80b471ef5762f2a65651500` |
| `/tmp/esp32sim-classic-rmt-raw/src/main.cpp` | `4093522de3545f74d51d7586f16960ed92dd0d8481d4cb13ec58aa4d264870f1` |
| `/tmp/esp32sim-classic-rmt-raw/.pio/build/esp32dev/firmware.factory.bin` | `bb8613b4d74141c98ee027834cd4b06650573cce35c2a48ca482b4097d74ac53` |
| `/tmp/esp32sim-classic-rmt-raw/.pio/build/esp32dev/firmware.elf` | `7c8515d849316346126d784fd3f37a6722146fd5a4817b5e042f803d6fe36ca0` |
| `/tmp/esp32sim-classic-rmt-raw/.pio/build/esp32dev/firmware.bin` | `6618b08ca3612f5291b7753cb72b1285c4ff873ee4688af49f8d55bac7aa9416` |

## Expected checks

- RGB: serial `RGB_LED gpio=4 rgb=255,0,64 completed=1`; observer must decode one RGB pixel `ff0040` on GPIO4. GRB wire bytes are `00 ff 40`.
- NeoPixel: serial `NEOPIXEL gpio=5 count=8 ff0000 00ff00 0000ff ffffff 010203 112233 ff0040 000000`; observer must decode exactly that eight-pixel RGB sequence.
- Raw: serial `RMT_RAW gpio=18 hz=1000000 init=1 write=1 completed=1`; observer must record level/duration halves `1:10us 0:20us 1:30us 0:40us 0:50us 1:60us`, then default low idle. Consecutive low halves may coalesce on an edge trace to `0:90us`.

## Evidence limits and privacy

The Arduino core and Adafruit library are unchanged. Sketches use public APIs only. Both
LED writers use 10 MHz RMT and 4/8-tick zero, 8/4-tick one timings. Adafruit emits 192
items through one 64-word block, so that run tests TX threshold refill and wrap. These
are functional correctness checks, not hardware timing measurements. Sources and
configuration are retained above; binaries and raw build logs remain outside Git. No
user name, hostname, device identifier or unrelated process data is retained here. Home
paths are normalized to `$HOME`.

ROM SHA-256: `920b70635440517866aab2230964a570d2cf2b676658d93c52fbac108c1cca31`.

## Recorded run samples

Code under test: `916e856d0cca3653be82b1a7764182a3e0e21451`, based on `4ff7f45`.
Host: Darwin arm64, macOS 26.6.2; Rust/Cargo 1.96.0. The three commands above ran
concurrently, in one batch, after `cargo build --release`. All exited 0 and stopped
at the requested 0.800 modeled seconds, 192,000,000 cycles. The first precommit smoke
batch also passed; its wall samples were 7.4, 7.4 and 7.2 seconds for rgb, neopixel and
raw. Its instruction counts and functional output matched the committed-source batch.
Wall values are retained as execution conditions, not comparable speed measurements.
No isolation or timing-accuracy claim is made.

| Run | Core 0 instructions | Core 1 instructions | Exceptions | Interrupts | Wall seconds |
| --- | ---: | ---: | ---: | ---: | ---: |
| rgb | 5735705 | 1602426 | 9613 | 1674 | 1.9 |
| neopixel | 5795802 | 1616867 | 9651 | 1677 | 1.9 |
| raw | 5732246 | 1602768 | 9609 | 1674 | 1.9 |

The output check asserted each exact serial line and observer line reproduced in the
[main receipt](README.md#rmt-and-ws2812-extension), the Halted stop and exact cycle count.
It also rejected any `WS2812 RGB` report for the raw-pulse fixture. All assertions passed.

Temporary output identities, SHA-256 of original bytes, are retained below. The logs
contain only the requested run, ROM boot and emulator report. No raw logs or binaries
are added to Git; the source, commands, input hashes and output checks above reproduce them.
These hashes identify outputs, not redacted replacements of an existing receipt.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| rgb run log | 1710 | `9fc43e71c7140c0c246e417be2fe8c44b522c33b8aa45a0f8285ed93298b0be3` |
| neopixel run log | 1845 | `d147b54e0f3c9acc5f1a0cdc283381e64adf4c20b1b7d549527e29eb117d86c8` |
| raw run log | 1560 | `46ae9fc4a90b113057fff93123b3d61abde7b098106a7bd182e7f56343209880` |
| release esp32sim | 2709864 | `340552b26a404103e7ecb356b0e79525d35ab9f3b683586576012ee62286230a` |
| WASM emulator | 2639125 | `4a9885868d578d1fba213573cda8bf75c776f1ec1c1fd5c1cc2f2d67e248eb70` |

No firmware build, emulator run, Rust test, release build or WASM build failed in this
extension. Existing ignored tests remain ignored because they require external firmware
or their documented special environment. Register tests cover invalid ownership and
memory exhaustion as expected device errors, rather than hiding them as TX success.
