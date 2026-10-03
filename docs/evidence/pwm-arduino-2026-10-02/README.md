# Arduino LEDC and MCPWM validation

This receipt covers EX204. It validates the same Arduino sketch on ESP32-S3, C3 and C6 and records the register-level MCPWM check. No prior experiment matched the searched `LEDC`, `MCPWM`, `PWM`, `tone`, `Arduino`, `timer duty` or `duty timer` aliases.

## Provenance and conditions

- Upstream base: `dddb128052dca15250e2169b92ab73c4d87f524c`.
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

## PR 167 review follow-up

Candidate `5075cd07429f78d8e6b5cae9f2b1db4d51eb6e37` corrects the matrix layout on C3/C6, uses 17.5 MHz for C6 RC_FAST,
and rejects MCPWM dead time and UTEP/UT0/UT1 actions. Five SoC tests write the actual peripheral
addresses and observe `pwm_output`. All 13 deliberate mutations failed assertions: eight wiring
mutations, RC_FAST, both old matrix decoders, dead-time bypass and extra generator actions.
Exact substitutions, commands and results are in [review167.json](review167.json).

Unconfigured LEDC returns no clock domain, so dispatch skips its tick and before/after IRQ reads.
Disabled MCPWM also returns no clock domain. LEDC phase units now cancel the factor of 256;
ordinary ticks use u64 arithmetic and remainder, with u128 only for overflowing large batches.
The exact-phase test includes `u64::MAX`; existing fractional-divider and latch checks pass.

This extends EX204 with stronger wiring checks and the requested c3-hello idle comparison.
Both executables use `cargo +1.99.0 build --release -p esp32sim`. Run each with:

```sh
target/release/esp32sim --chip c3 --rom web/wasm/fw/esp32c3_rev3_rom.elf --boot rom --flash-mb 4 --bootloader web/wasm/fw/public/c3-hello-bootloader.bin --ptable web/wasm/fw/public/c3-hello-ptable.bin --app web/wasm/fw/public/c3-hello_world.bin --max-seconds 30 --no-dump
```

CPU seconds are child user + system time, measured with Python `resource.getrusage` around
`subprocess.run`. The JSON retains revisions, binary/input hashes, run order and all samples.
Each recorded run retired 4,800,000,000 instructions over 30.000 emulated seconds.
The paired runs all printed Hello world and had the same normalized console SHA-256
`dbe0f29616984936966a482e1f4703e4bd3d6c4f4c41a2f2448981760739ab76`. This is also the hash of the baseline console confirmation run.

| Measurement | Published head CPU seconds | Candidate CPU seconds | Median change |
| --- | --- | --- | --- |
| Initial separate batches | 2.587584, 2.537439, 2.504536 | 2.640012, 2.651190, 2.732550 | +4.48% |
| Interleaved B/A, A/B, B/A | 2.641291, 2.673320, 2.633051 | 2.683554, 2.709617, 2.626860 | +1.60% |

The initial batches ran several minutes apart, so the second measurement changed the order to
reduce drift. The paired result is mixed, with a 1.60% slower candidate median;
it does not demonstrate a net speedup. Host activity and CPU frequency were uncontrolled.
Retain this negative result: removing idle device work is supported by the dispatch guard and
test, but does not establish zero whole-program overhead.

Both Clippy commands with warnings denied passed. The full release workspace suite with
`--include-ignored --skip external_` passed 485 tests, zero ignored, including unchanged goldens.
The wasm build and all eight requested demos passed. The restored-source PWM tests passed after
mutation checks. The evidence privacy check passed. No JIT code changed.

The earlier receipt's nonexistent base hash was corrected to
`dddb128052dca15250e2169b92ab73c4d87f524c`; earlier firmware results and artifact hashes are unchanged.
No personal paths, hostnames, usernames or raw private captures are included in this follow-up.
