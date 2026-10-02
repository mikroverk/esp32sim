# Arduino LEDC and MCPWM validation

This receipt covers EX204. It validates the same Arduino sketch on ESP32-S3, C3 and C6 and records the register-level MCPWM check. No prior experiment matched the searched `LEDC`, `MCPWM`, `PWM`, `tone`, `Arduino`, `timer duty` or `duty timer` aliases.

## Provenance and conditions

- Upstream base: `dddb128052d31cd009826299d6359a42d1696426`.
- Port source: Schematik fork `pr-1` at `221080ffb5ee8c19b8ce8b31d8953a93603b7fda`, merge base `d5446b4`.
- Executed candidate: `9cc58f1` (the following source-clock test and this receipt do not alter the executed model).
- PlatformIO Core 6.1.19, pioarduino platform `55.03.38+sha.fbdfc29`, Arduino-ESP32 3.3.8.
- Rust 1.96.0 (`aarch64-apple-darwin`), Darwin 25.6.0 arm64.
- The firmware boots from the matching Espressif mask-ROM ELF and the PlatformIO bootloader, partition table and application image. S3/C6 use 8 MiB flash; C3 uses 4 MiB.

The shared `platformio.ini` was:

```ini
[platformio]
default_envs = s3, c3, c6

[env]
platform = https://github.com/pioarduino/platform-espressif32.git#55.03.38-1
framework = arduino
monitor_speed = 115200

[env:s3]
board = esp32-s3-devkitc-1

[env:c3]
board = esp32-c3-devkitm-1

[env:c6]
board = esp32-c6-devkitc-1
```

Every environment compiled this unchanged sketch (SHA-256 `3a67a3a50f4744edc931bd5fd8f8870ee280944476dbd1408c7dc0e40fc4e734`):

```cpp
#include <Arduino.h>

constexpr uint8_t kPin = 4;

void setup() {
  Serial.begin(115200);
  delay(1000);

  const bool attached = ledcAttach(kPin, 5000, 8);
  const bool written = ledcWrite(kPin, 64);
  Serial.printf("LEDC pin=%u attach=%u write=%u requested_hz=5000 requested_duty=64\n",
                kPin, attached, written);

  delay(10000);
  ledcDetach(kPin);
  tone(kPin, 440);
  Serial.printf("TONE pin=%u requested_hz=440\n", kPin);
}

void loop() { delay(1000); }
```

## Exact work and checks

The build command was:

```sh
platformio run -d target/ledc-arduino
```

It completed all three environments successfully. The application image hashes were:

| Environment | `firmware.bin` SHA-256 |
| --- | --- |
| S3 | `b8c0b37d58b0fbc08f8ab2f622565fc2b1095af839773e28c77b2ae697be4eac` |
| C3 | `1e76d379994d7e2d509e9241bab2a5a2407df98452209bed2623dfca371368d0` |
| C6 | `b16c25151364941b64e3feac033116935d31920e135d1d9e53e366b6ec3e2fc8` |

Each image was booted twice with the release CLI. `CHIP` and `ENV` were respectively `s3/s3`, `c3/c3` and `c6/c6`; `ROM` was the matching `esp32s3_rev0_rom.elf`, `esp32c3_rev3_rom.elf` or `esp32c6_rev0_rom.elf`; `FLASH_MB` was 8, 4 or 8. The two runs used `SECONDS=5` and `SECONDS=15`:

```sh
target/release/esp32sim --chip "$CHIP" --boot rom --rom "$ROM_DIR/$ROM" \
  --bootloader "target/ledc-arduino/.pio/build/$ENV/bootloader.bin" \
  --ptable "target/ledc-arduino/.pio/build/$ENV/partitions.bin" \
  --app "target/ledc-arduino/.pio/build/$ENV/firmware.bin" \
  --elf "target/ledc-arduino/.pio/build/$ENV/firmware.elf" \
  --board none --flash-mb "$FLASH_MB" --max-seconds "$SECONDS" \
  --console all --pwm 4 --no-dump
```

| Chip | Stop | Console check | GPIO4 observation |
| --- | ---: | --- | --- |
| S3 | 5.000 s | `attach=1 write=1 requested_hz=5000 requested_duty=64` | 5000.000 Hz, 25.00% |
| S3 | 15.000 s | `TONE pin=4 requested_hz=440` | 440.005 Hz, 49.90% |
| C3 | 5.000 s | `attach=1 write=1 requested_hz=5000 requested_duty=64` | 5000.000 Hz, 25.00% |
| C3 | 15.000 s | `TONE pin=4 requested_hz=440` | 440.005 Hz, 49.90% |
| C6 | 5.000 s | `attach=1 write=1 requested_hz=5000 requested_duty=64` | 5000.000 Hz, 25.00% |
| C6 | 15.000 s | `TONE pin=4 requested_hz=440` | 440.005 Hz, 49.90% |

LEDC and MCPWM register tests run under `cargo test -p esp-periph`. They check timer/divider configuration, duty and compare latching, output routing, interrupt state, 20-bit C6 values and source-clock scaling. An Arduino MCPWM path was not used: the generic Arduino `ledcAttach`, `ledcWrite` and `tone` APIs exercise LEDC, while direct MCPWM setup would require a chip-specific ESP-IDF program.

## Limits and privacy

`--pwm` reports the configured steady-state frequency and duty through the GPIO matrix. It does not sample individual output edges. Hardware fades and MCPWM sync, capture, fault, carrier and dead-time behavior are not modelled. ESP32-C3 has no MCPWM peripheral.

No raw host capture is retained. Home-directory paths are normalized to `$ROM_DIR`; no hostname, login name, device identifier, unrelated process inventory or session data is recorded. The redaction does not affect firmware hashes, emulator values or the commands needed to reproduce the result.
