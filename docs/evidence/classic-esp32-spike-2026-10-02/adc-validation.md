# Classic ADC, DAC and touch validation

This receipt extends [EX199](../../experiments.md#ex199) at port code-under-test revision
`c6ed024d5cf324babc2d74162729cc1f2af62a6a`, based on `0ef8390`.
The original validation below used `6c855ccf0fc8a4e12aff150430df2e26952c4e6a`
on `4ff7f453dec6febb09c1137355c57e07625eefc0`, recorded by `1fd9c06`.
Historical outputs, hashes and negative results are retained; the port revalidation
section records the new run.
The [earlier receipt](README.md) records GPIO, I2C, LEDC, timer and SPI validation.
The new mechanism is the classic SENS and RTC IO register layout for SAR one-shot
conversions, DAC outputs and capacitive touch pads. The correctness contract adds
unmodified Arduino ADC, calibrated millivolt, DAC and touch calls with changing host
inputs. This is functional evidence, with no execution-speed or cycle-accuracy claim.

## Firmware and host inputs

Both projects used PlatformIO Core 6.1.19, board `esp32dev`, and this configuration:

```ini
[env:esp32dev]
platform = https://github.com/pioarduino/platform-espressif32.git#55.03.38-1
board = esp32dev
framework = arduino
monitor_speed = 115200
```

PlatformIO resolved platform `55.3.38+sha.fbdfc29`, Arduino-ESP32 `3.3.8`,
framework libraries `5.5.4+sha.735507283d`, and Xtensa GCC `14.2.0+20260121`.
Both sketches used unmodified framework APIs. Builds stayed in `/tmp`, outside
the repository.

Host tools were Rust/Cargo 1.96.0 on Darwin arm64, macOS 26.6.2 build 25G83.
Host load was uncontrolled; these runs test functional behavior only.

The main sketch in `/tmp/esp32sim-classic-adc-pio/src/main.cpp` was:

```cpp
#include <Arduino.h>

void setup() {
  Serial.begin(115200);
  Serial.printf("CLASSIC_ADC_DAC_TOUCH millis=%lu\n", millis());
  bool written = dacWrite(25, 128);
  Serial.printf("DAC pin=25 code=128 written=%u\n", written);
}

void loop() {
  static unsigned sample;
  uint16_t adc1 = analogRead(34);
  uint32_t mv1 = analogReadMilliVolts(34);
  uint16_t adc2 = analogRead(27);
  uint32_t mv2 = analogReadMilliVolts(27);
  Serial.printf("ADC sample=%u millis=%lu pin34_raw=%u pin34_mv=%lu pin27_raw=%u pin27_mv=%lu\n",
                sample, millis(), adc1, mv1, adc2, mv2);
  uint16_t touch = touchRead(T0);
  Serial.printf("TOUCH sample=%u millis=%lu pin=%u value=%u\n", sample++, millis(), T0, touch);
  delay(100);
}
```

The main script, `/tmp/esp32-classic-adc-input.txt`, was:

```text
0 adc 34 1.650
0 adc 27 1.650
0.30 adc 34 0.800
0.40 touchpad 4 1
0.80 touchpad 4 0
```

The original parser accepted finite volts from 0 through 3.3. The port retains
#165's parser unchanged and clamps/rounds voltage when the classic ADC samples it.
`touchpad` takes a GPIO and binary contact state, separate from display `touch`.
The main run applies 1650 mV, changes GPIO34 to 800 mV at 0.30 modeled seconds,
presses T0 at 0.40 seconds and releases it at 0.80 seconds.

The supplemental sketch in `/tmp/esp32sim-classic-adc-pads-pio/src/main.cpp` was:

```cpp
#include <Arduino.h>

void setup() {
  Serial.begin(115200);
  delay(100);
  Serial.println("CLASSIC_ADC_PADS");
  const uint8_t adc_pins[] = {36, 37, 38, 39, 32, 33, 34, 35, 4, 0, 2, 15, 13, 12, 14, 27, 25, 26};
  for (uint8_t pin : adc_pins) {
    Serial.printf("PAD pin=%u raw=%u\n", pin, analogRead(pin));
  }
  const adc_attenuation_t attenuation[] = {ADC_0db, ADC_2_5db, ADC_6db, ADC_11db};
  for (uint8_t bits = 9; bits <= 12; ++bits) {
    analogReadResolution(bits);
    for (uint8_t atten = 0; atten < 4; ++atten) {
      analogSetAttenuation(attenuation[atten]);
      uint16_t raw1 = analogRead(34);
      uint32_t mv1 = analogReadMilliVolts(34);
      uint16_t raw2 = analogRead(27);
      uint32_t mv2 = analogReadMilliVolts(27);
      Serial.printf("CONFIG bits=%u atten=%u adc1=%u mv1=%lu adc2=%u mv2=%lu\n",
                    bits, atten, raw1, mv1, raw2, mv2);
    }
  }
  const uint8_t touch_pins[] = {T0, T1, T2, T3, T4, T5, T6, T7, T8, T9};
  for (uint8_t pad = 0; pad < 10; ++pad) {
    uint16_t value = touchRead(touch_pins[pad]);
    Serial.printf("TOUCH_PAD pad=%u pin=%u value=%u\n", pad, touch_pins[pad], value);
  }
  bool dac1 = dacWrite(25, 128);
  bool dac2 = dacWrite(26, 64);
  Serial.printf("DAC_PADS pin25=128 ok=%u pin26=64 ok=%u\n", dac1, dac2);
  Serial.println("CLASSIC_ADC_PADS_DONE");
}

void loop() { delay(1000); }
```

The supplemental script, `/tmp/esp32-classic-adc-pads-input.txt`, was:

```text
0 adc 36 0.300
0 adc 37 0.370
0 adc 38 0.440
0 adc 39 0.510
0 adc 32 0.580
0 adc 33 0.650
0 adc 34 1.650
0 adc 35 0.790
0 adc 4 0.860
0 adc 0 0.930
0 adc 2 1.000
0 adc 15 1.070
0 adc 13 1.140
0 adc 12 1.210
0 adc 14 1.280
0 adc 27 1.650
0 adc 25 1.420
0 adc 26 1.490
0 touchpad 0 1
0 touchpad 15 1
0 touchpad 12 1
0 touchpad 27 1
0 touchpad 32 1
```

Distinct voltages identify each ADC pad. GPIO34 and GPIO27 both retain 1650 mV for
the width and attenuation sweep. Alternating touched pads distinguish all ten touch
channels, including the T8/T9 register swap. The sketch ends with both DAC outputs
enabled at different values.

## Reproduction commands

Build the two temporary projects with:

```sh
cd /tmp/esp32sim-classic-adc-pio
pio run
cd /tmp/esp32sim-classic-adc-pads-pio
pio run
```

From the repository root, the main command was:

```sh
cargo build --release
target/release/esp32sim --chip esp32 --boot rom \
  --rom "$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf" \
  --flash-image /tmp/esp32sim-classic-adc-pio/.pio/build/esp32dev/firmware.factory.bin \
  --elf /tmp/esp32sim-classic-adc-pio/.pio/build/esp32dev/firmware.elf \
  --board none --script /tmp/esp32-classic-adc-input.txt \
  --max-seconds 1.2 --no-reboot --no-dump
```

The supplemental command was:

```sh
target/release/esp32sim --chip esp32 --boot rom \
  --rom "$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf" \
  --flash-image /tmp/esp32sim-classic-adc-pads-pio/.pio/build/esp32dev/firmware.factory.bin \
  --elf /tmp/esp32sim-classic-adc-pads-pio/.pio/build/esp32dev/firmware.elf \
  --board none --script /tmp/esp32-classic-adc-pads-input.txt \
  --max-seconds 2 --no-reboot --no-dump
```

## Conversion and pad contract

Primary references are Arduino 3.3.8's
[ADC calls](https://github.com/espressif/arduino-esp32/blob/3.3.8/cores/esp32/esp32-hal-adc.c)
and [touch calls](https://github.com/espressif/arduino-esp32/blob/3.3.8/cores/esp32/esp32-hal-touch-ng.c),
and ESP-IDF 5.5's
[classic calibration](https://github.com/espressif/esp-idf/blob/v5.5/components/esp_adc/esp32/adc_cali_line_fitting.c)
and [analog GPIO preparation](https://github.com/espressif/esp-idf/blob/v5.5/components/esp_driver_gpio/src/gpio.c).
The installed source and linked-library hashes below identify the actual build inputs.

`analogRead` calls `adc_oneshot_read`. ADC continuous mode is not needed by this
workload. Arduino defaults to a 12-bit result and `ADC_11db`, the fourth attenuation
setting. The IDF name for that setting is `ADC_ATTEN_DB_12`.

For unit `u`, attenuation `a`, input millivolts `m` and width `w`, the model
inverts ESP-IDF classic linear Vref calibration:

```text
slope = floor(1100 * scale[u][a] / 4096)
raw12 = min(4095, floor((max(0, m - offset[u][a]) * 65536 + floor(slope / 2)) / slope))
raw = raw12 >> (12 - w)
```

Widths are 9 through 12 bits. Arduino enables the hardware inversion bit for
an increasing-voltage result. Without that bit, the model complements the result
within the selected width.

| Unit | Attenuation scales, settings 0 through 3 | Offsets in mV |
| --- | --- | --- |
| ADC1 | `57431, 76236, 105481, 196602` | `75, 78, 107, 142` |
| ADC2 | `57236, 76175, 105678, 197170` | `63, 66, 89, 128` |

At 1650 mV, 12 bits and attenuation 3, ADC1 has slope `52798` and offset `142`.
The expected raw result is **1872**. ADC2 has slope `52950` and offset `128`, giving
**1884**. The firmware calibrates these raw values with
`floor((slope * raw + 32768) / 65536) + offset`, yielding **1650 mV** for both.
At 800 mV, GPIO34 produces **817** and calibrates to **800 mV**.

The eFuse model previously had no ADC calibration. It now supplies nominal 1100 mV
Vref using the nonzero sign-magnitude encoding `0x10`, negative zero relative to
1100 mV. This is a nominal model value, not measured chip calibration. No two-point
values are invented. Arduino 3.3.8 does not fill `default_vref` in its line-fitting
configuration. Leaving the Vref eFuse zero would make IDF reject calibrated reads
with `default vref didn't set`.

ADC1 channels 0 through 7 map to GPIO36, 37, 38, 39, 32, 33, 34 and 35. ADC2
channels 0 through 9 map to GPIO4, 0, 2, 15, 13, 12, 14, 27, 25 and 26.
Touch T0 through T9 map to GPIO4, 0, 2, 15, 13, 12, 14, 27, 33 and 32.

`dacWrite` uses `dac_oneshot_output_voltage`. The model observes each enabled DC
output as `round(code * 3300 / 255)` mV. GPIO25 code 128 therefore reports **1656 mV**,
and GPIO26 code 64 reports **828 mV**. Power-down and cosine-wave selection suppress
the DC report.

Arduino `esp32-hal-touch-ng.c` performs three one-shot scans, polling the done
bit, then starts continuous scans. The model returns raw counts of 1000 untouched
and 300 touched. IDF applies software filtering.

## Main run output

Selected serial lines and the complete stop counters were:

```text
[script] t=0.000s Adc(34, 1650)
[script] t=0.000s Adc(27, 1650)
CLASSIC_ADC_DAC_TOUCH millis=2
DAC pin=25 code=128 written=1
ADC sample=0 millis=3 pin34_raw=1872 pin34_mv=1650 pin27_raw=1884 pin27_mv=1650
TOUCH sample=0 millis=6 pin=4 value=1000
[script] t=0.300s Adc(34, 800)
ADC sample=3 millis=305 pin34_raw=817 pin34_mv=800 pin27_raw=1884 pin27_mv=1650
TOUCH sample=3 millis=305 pin=4 value=1000
[script] t=0.400s TouchPad(4, true)
ADC sample=4 millis=405 pin34_raw=817 pin34_mv=800 pin27_raw=1884 pin27_mv=1650
TOUCH sample=4 millis=405 pin=4 value=475
ADC sample=5 millis=505 pin34_raw=817 pin34_mv=800 pin27_raw=1884 pin27_mv=1650
TOUCH sample=5 millis=505 pin=4 value=301
[script] t=0.800s TouchPad(4, false)
ADC sample=8 millis=805 pin34_raw=817 pin34_mv=800 pin27_raw=1884 pin27_mv=1650
TOUCH sample=8 millis=805 pin=4 value=826
ADC sample=9 millis=905 pin34_raw=817 pin34_mv=800 pin27_raw=1884 pin27_mv=1650
TOUCH sample=9 millis=905 pin=4 value=1000
[emu] stop: Halted — core0 6873257 + core1 3088273 insns in 2.6s wall = 3.9 Minsn/s; emulated 1.200s (288000000 cycles); 14960 exceptions, 2713 interrupts
[dac] GPIO25: 1656 mV
```

The main run checked all 12 samples. Raw touch changes from 1000 to 300.
IDF's integer software filter reports 475 after contact and settles at 301
when approached from above. It returns through 826 to 1000 after release.
The supplemental run initializes already-touched pads at 300, so their filtered
values remain exactly 300.

## Pad, width, attenuation and DAC2 output

```text
CLASSIC_ADC_PADS
PAD pin=36 raw=196
PAD pin=37 raw=283
PAD pin=38 raw=370
PAD pin=39 raw=457
PAD pin=32 raw=544
PAD pin=33 raw=631
PAD pin=34 raw=1872
PAD pin=35 raw=804
PAD pin=4 raw=906
PAD pin=0 raw=993
PAD pin=2 raw=1079
PAD pin=15 raw=1166
PAD pin=13 raw=1253
PAD pin=12 raw=1339
PAD pin=14 raw=1426
PAD pin=27 raw=1884
PAD pin=25 raw=1599
PAD pin=26 raw=1686
CONFIG bits=9 atten=0 adc1=511 mv1=1037 adc2=511 mv2=1022
CONFIG bits=9 atten=1 adc1=511 mv1=1355 adc2=511 mv2=1342
CONFIG bits=9 atten=2 adc1=446 mv1=1649 adc2=450 mv2=1648
CONFIG bits=9 atten=3 adc1=234 mv1=1650 adc2=235 mv2=1647
CONFIG bits=10 atten=0 adc1=1023 mv1=1038 adc2=1023 mv2=1023
CONFIG bits=10 atten=1 adc1=1023 mv1=1356 adc2=1023 mv2=1343
CONFIG bits=10 atten=2 adc1=892 mv1=1649 adc2=901 mv2=1650
CONFIG bits=10 atten=3 adc1=468 mv1=1650 adc2=471 mv2=1650
CONFIG bits=11 atten=0 adc1=2047 mv1=1038 adc2=2047 mv2=1023
CONFIG bits=11 atten=1 adc1=2047 mv1=1357 adc2=2047 mv2=1344
CONFIG bits=11 atten=2 adc1=1785 mv1=1650 adc2=1802 mv2=1650
CONFIG bits=11 atten=3 adc1=936 mv1=1650 adc2=942 mv2=1650
CONFIG bits=12 atten=0 adc1=4095 mv1=1039 adc2=4095 mv2=1023
CONFIG bits=12 atten=1 adc1=4095 mv1=1357 adc2=4095 mv2=1344
CONFIG bits=12 atten=2 adc1=3570 mv1=1650 adc2=3605 mv2=1650
CONFIG bits=12 atten=3 adc1=1872 mv1=1650 adc2=1884 mv2=1650
TOUCH_PAD pad=0 pin=4 value=1000
TOUCH_PAD pad=1 pin=0 value=300
TOUCH_PAD pad=2 pin=2 value=1000
TOUCH_PAD pad=3 pin=15 value=300
TOUCH_PAD pad=4 pin=13 value=1000
TOUCH_PAD pad=5 pin=12 value=300
TOUCH_PAD pad=6 pin=14 value=1000
TOUCH_PAD pad=7 pin=27 value=300
TOUCH_PAD pad=8 pin=33 value=1000
TOUCH_PAD pad=9 pin=32 value=300
DAC_PADS pin25=128 ok=1 pin26=64 ok=1
CLASSIC_ADC_PADS_DONE
[emu] stop: Halted — core0 8341949 + core1 4774345 insns in 3.5s wall = 3.7 Minsn/s; emulated 2.000s (480000000 cycles); 23050 exceptions, 4393 interrupts
[dac] GPIO25: 1656 mV
[dac] GPIO26: 828 mV
```

At 1650 mV, the 0 dB and 2.5 dB settings exceed their nominal calibrated input
range and clip at the selected width's maximum code. Their lower millivolt results
are expected saturation, not failed input delivery. Reduced widths also quantize
the calibrated voltage. The 6 dB and 11 dB settings retain the applied value within
that quantization.

## Input and artifact hashes

SHA-256 values identify the original inputs and executed firmware. Both builds
use the same `platformio.ini`, `bootloader.bin` and `partitions.bin`.

| Item | SHA-256 |
| --- | --- |
| `$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf` | `920b70635440517866aab2230964a570d2cf2b676658d93c52fbac108c1cca31` |
| `/tmp/esp32sim-classic-adc-pio/platformio.ini` | `b86c69259be417474b2dfef705f80db5720beff2f80b471ef5762f2a65651500` |
| `/tmp/esp32sim-classic-adc-pio/src/main.cpp` | `c5ad8ee9f2859685e53aa6c0e5cf13b05eb609e12db5ea917801d4d48c708e64` |
| `Main bootloader.bin` | `a227e3ab93f15f1155efb3144810cc06ff7a259e9bc9b4542417afcc6c238214` |
| `Main partitions.bin` | `148b959cbff1c38aa8e1d5c0ba9d612c54997b945e56a63f41223eef650653a1` |
| `Main firmware.bin` | `1b79cebb11f1814f0fc417505d8856a69b17d6fd0d08ea49c1cfe63b61686f0c` |
| `Main firmware.factory.bin` | `be09b33f8e608a5332e33d34d1cfd548229daba08622bf8a3df4cb9da8eb4855` |
| `Main firmware.elf` | `06483287171b06dd21441ef494b37f2c9ea43afd8ae05bd2054bcf8b469f0625` |
| `/tmp/esp32sim-classic-adc-pads-pio/src/main.cpp` | `01c1cefb86e44f47a18765429df5a8f1cba46b91a2c25527546f983f3a9bdcaf` |
| `Supplemental firmware.bin` | `a8d79b98b3c88a67947c9d0c866601094c54b1df578f53635c28df166ba788ff` |
| `Supplemental firmware.factory.bin` | `9faad0f9f1350e5954260b93b1855b0c0290ad86f1803378026e0cb0069b9107` |
| `Supplemental firmware.elf` | `fd887ad189e02216b7248259540278b27c1225908dc6d52dda90e12d8eda614d` |
| `/tmp/esp32-classic-adc-input.txt` | `8b02abb6eaf9ad8a37927ba692adf481112efc35b955d08c443f671d93b8aacd` |
| `/tmp/esp32-classic-adc-pads-input.txt` | `1178f9fbd99d62b2d61dd76b2719f32fde66945bbb5d13d621dcc2bbe54dbe00` |

The source review used the following installed files. Table prefixes are relative
to `$HOME/.platformio/packages`: `Arduino` means `framework-arduinoespressif32/cores/esp32`,
`HAL` means `framework-arduinoespressif32-libs/esp32/include/hal/esp32/include/hal`,
`IDF` means `framework-espidf/components`, and `Lib` means
`framework-arduinoespressif32-libs/esp32/lib`. The separate IDF source package
reports `3.50504`, corresponding to IDF 5.5.4. Arduino links the hashed libraries.

| Source or linked library | SHA-256 |
| --- | --- |
| `Arduino/esp32-hal-adc.c` | `0d116c35d1052a8c25ddf48b334b999584eb27733414eac6ee32378598fe750d` |
| `Arduino/esp32-hal-dac.c` | `3af34c32fa3a072b754368b87d5ee8ca482a0d55d0400266884cc08017725753` |
| `Arduino/esp32-hal-touch-ng.c` | `ebfb5d9ba318d187d09ece69545b839e97e49b19c5c5c0533c973ac6dac6e78b` |
| `HAL/adc_ll.h` | `f52a8c9e228eb5cb3ea0e47ca36b689494fa1192f26c60a17888ab6e65fb0f3e` |
| `HAL/touch_sensor_ll.h` | `29d923ad22b04403bbad39aa3456f3ea2ff8b8f7ffceab587cdc7bb55e8a107c` |
| `HAL/dac_ll.h` | `c43d224f6d3a9d0d610d837c6ba430355a1b0b6cb8949626d3fa6a767a3592be` |
| `IDF/esp_adc/esp32/adc_cali_line_fitting.c` | `47d24937c929fcd0efc375f5b8cc1e7560ce6fb3e0d05590d4fa671c4d29429c` |
| `IDF/esp_driver_gpio/src/gpio.c` | `aa8bdfd7fad5fd75830181df84bd8a2fa8ad0a12990ec37ade3338b9dd92c728` |
| `Lib/libesp_adc.a` | `4b7b43860c1a917644b5a0973f11850b942c7ce4ffab2716a7093024563f09e9` |
| `Lib/libesp_driver_touch_sens.a` | `2c7cd6788932d906f72b44f7c9d655f1b62ef670241b7a598375d636fdd2f910` |
| `Lib/libesp_driver_dac.a` | `6a84f8bce96c18bd9fe44c43b04cbd4ca48ff939965e364e362e5c9099aa2920` |
| `Lib/libesp_driver_gpio.a` | `0f0102c486369eb353c9c3e793122a3d23b7eb8567c73ec62c58a6aa33112338` |

## Negative result and correction

The first firmware runs completed but returned zero for every ADC and touch pad.
DAC calls returned success, but the CLI reported no DAC voltage. The main run's
first sample was:

```text
DAC pin=25 code=128 written=1
ADC sample=0 millis=3 pin34_raw=0 pin34_mv=142 pin27_raw=0 pin27_mv=128
TOUCH sample=0 millis=6 pin=4 value=0
```

The millivolt values were the calibration offsets for a raw zero. A bounded
0.15-second diagnostic run read the configured pad registers after initialization:

```text
3ff48480: 00000000
3ff48484: 04040400
3ff48494: 03800000
3ff484b0: 02000000
```

The initial model incorrectly required RTC GPIO mux selection for all analog pads.
IDF `gpio_config_as_analog` calls `rtc_gpio_deinit`, correctly clearing that
selection. That exposed the common cause of all three failures.
One correction removed that condition from the analog paths while retaining RTC
GPIO routing. The same firmware and scripts were then rerun.

The model and original temporary logs have these SHA-256 identifiers:

| Item | SHA-256 |
| --- | --- |
| Initial esp32/src/adc.rs | `30a5f669d1d7b0675239ba13597b34e29563089d5ec7a1e953b7d02781b16f7e` |
| Initial main run original log | `98e37d3750c8e9ab16e908fd44ee7ea14f65b42b977716b9c5bf389adfe6714a` |
| Initial supplemental run original log | `d13178671f77c2e2616385aeca44939be03e7373c72211a51196605dc163e03d` |
| Main final original log, 3096 bytes | `dcb87feddf7594fc6e325c27430bdc423217e4128510863faa2f991ded92e6ef` |
| Supplemental final original log, 3872 bytes | `a0746f1b1cbdee4dcc8faf181077ac3dd7a705bdae2e109001e1087ad90716db` |

The initial main run stopped at 1.200 modeled seconds, 288,000,000 cycles. It
retired 6,872,723 core-0 and 3,059,605 core-1 instructions. The initial supplemental
run stopped at 2.000 modeled seconds, 480,000,000 cycles, after 8,464,831 core-0
and 4,742,574 core-1 instructions. Wall times were 2.3 and 6.4 seconds respectively.
These identify the negative runs. Host load was uncontrolled, so no speed claim follows.

The first PlatformIO invocation failed with
`PermissionError: [Errno 1] Operation not permitted: '$HOME/.platformio/platforms.lock'`.
One retry with access to the existing package cache passed. The supplemental build
used that access and passed. A diagnostic `--peek` argument using a colon failed
with `--addr: bad hex 0x3ff4847c:16`. The corrected comma-separated argument succeeded.

## Checks and limits

All required gates passed on the final code:

```text
cargo build --release
cargo test -p esp32 -p esp-soc -p esp32sim
cargo test --workspace
tools/wasm-build.sh
```

Touched crates passed 145 tests with 15 external-firmware tests ignored.
The workspace passed 497 tests with 22 ignored, including unchanged S3, C3 and C6
suites. Programmatic checks matched all 12 main samples, 18 ADC pads, 16
width/attenuation configurations, ten touch pads and both DAC outputs.
Privacy and whitespace checks are recorded in the parent receipt.

`node tools/check-evidence-privacy.mjs` passed with 1,473 tracked evidence files,
including 15 gzip files. `git diff --check` and manual review also passed.

The 32 classic unit tests include ADC channels, attenuation, width, inversion,
controller ownership, input changes, DAC power/DC selection, touch scans and
T8/T9 mapping, RTC GPIO routing and reset preservation. CLI tests cover scripts.

The ADC conversion completes immediately. The model omits continuous ADC, DMA,
Wi-Fi arbitration, noise, acquisition time and electrical contention. The host
voltage is an absolute input. The nominal Vref transfer is linear. IDF's 11 dB
high-code lookup table starts at raw 2880, about 2462 mV for ADC1 and 2455 mV for
ADC2 under this model. Accuracy above that point is outside this receipt's contract.
At the low end, calibrated readings retain the IDF offset even when raw data clips
to zero. DAC supply is fixed at 3300 mV. Cosine-wave and streamed DAC output are
unmodeled. Touch counts are fixed baseline/contact values, not a capacitance or
electrode model. Hardware threshold interrupts and wakeup are unmodeled.

## Evidence curation

This receipt retains exact sketches, commands, scripts, artifact hashes, register
observations, relevant serial lines and numerical results. It omits full build
logs, ROM banners, interrupt listings, JIT diagnostics and unrelated environment
data. Home paths are normalized to `$HOME`. Original negative-log hashes identify
the unsanitized temporary captures; no private originals or backups are committed.
The summaries preserve the failed readings and their cause. No source receipt was
replaced or rehashed during sanitization.

## Port revalidation with PR #165 host inputs

Code under test: `c6ed024d5cf324babc2d74162729cc1f2af62a6a`; parent: `0ef8390`.
The original implementation and receipt remain identified above. This revision
retains the register, DAC and touch contract, replaces the private voltage array
with `AnalogInputs`, and implements `SocBus::analog_set`. Each conversion samples
at the bus cycle count. Constants and waveform sources survive reboot, including
the waveform origin on the bus's continuing timeline. No change to #165's analog
API, parser, shared S3 conversion code or cherry-picked commit was required.

The classic transfer still clamps voltage to 0..3.3 V and rounds to millivolts.
The old `ScriptAction::Adc`, `SocBus::set_adc_voltage` and separate `adc` parse arm
are absent. The retained touch command uses `ScriptAction::TouchPad` and the
`SocBus::set_touch_input` default method. Newer classic RMT and crypto code remains.

All commands exited 0, without a build or test retry:

| Command | Result |
| --- | --- |
| `cargo build --release` | Pass |
| `cargo test -p esp32 -p esp32sim` | 87 passed, 15 ignored, 0 failed |
| `cargo test --workspace` | 524 passed, 22 ignored, 0 failed |
| `tools/wasm-build.sh` | Pass |
| `node tools/check-evidence-privacy.mjs` | Pass: 1,477 tracked evidence files, 15 gzip; manual review passed |
| `git diff --check` | Pass |

Ignored tests retain their existing external-artifact rules. #165's tests
`analog::tests::wave_follows_emulated_time_and_holds_the_ends`,
`rtc_cntl::sens_adc_tests::idf44_forward_matches_hand_computed_points` and
`rtc_cntl::sens_adc_tests::injected_volts_round_trip_through_the_firmware_formula`
passed unchanged. The S3 crate and the rest of the workspace passed.

The new `bus::tests::classic_adc_samples_host_waveform_at_bus_time` injects
`AnalogSource::Wave` on GPIO34 with samples `[1.650, 0.800]`, rate 10 Hz and
origin 24,000,000 cycles at 240 MHz. Bus writes trigger actual SENS conversions:

| Bus cycle | Raw ADC1 result |
| --- | --- |
| 0, before origin | 1872 |
| 47,999,999, before second sample | 1872 |
| 48,000,000, second sample boundary | 817 |
| 240,000,000, after waveform end | 817 |
| After reboot at the same bus time | 817 |

Both Arduino commands in this receipt were rerun with the existing firmware and
scripts, without rebuilding or patching firmware. All 14 input/artifact hashes
in the original table were recomputed and matched. The host remained Darwin
arm64, macOS 26.6.2, Rust and Cargo 1.96.0. Load was uncontrolled and builds/tests
could overlap; wall durations establish no performance claim.

All 12 ADC and 12 touch samples matched, including timestamps. GPIO34 read
1872 / 1650 mV, then 817 / 800 mV; GPIO27 stayed at 1884 / 1650 mV.
Touch values were `[1000, 1000, 1000, 1000, 475, 301, 301, 301, 826, 1000, 1000, 1000]`.
Every retained main serial line matched. All 49 supplemental serial/DAC lines
matched the original receipt exactly: 18 pads, 16 configurations, ten touch pads,
completion markers and DAC outputs of 1656 mV and 828 mV.

The main run again stopped at 288,000,000 cycles with 6,873,257 core-0 and
3,088,273 core-1 instructions, 14,960 exceptions and 2,713 interrupts. Wall time
was 2.3 s. The supplemental run stopped at 480,000,000 cycles with 8,341,949 core-0
and 4,774,345 core-1 instructions, 23,050 exceptions and 4,393 interrupts. Wall time
was 3.2 s. Host script diagnostics changed from `Adc` to `Analog`.

Port artifact hashes:

| Item | SHA-256 |
| --- | --- |
| `target/release/esp32sim` | `793842d8a14733175323551ef1bc5e386d866c1fb0c2c11d7e43a96805213561` |
| `web/wasm/esp32sim.wasm` | `c224e013b1dbba41c03da3bc5b9ad3acce74665a73c78f8818717bc3355717fc` |
| `esp32/src/adc.rs` | `9963463ce3e7720204b2275cb0d69d63f1b2381ec168c6237e5884172ae303b8` |
| `/tmp/adc2-main.log` | `13da65fb8954bc104f6762b02043a95d3b3e0b9e4199e92dff256a5eb6de1bf4` |
| `/tmp/adc2-pads.log` | `7055597a0610ec5b359c45bfef28e0ac25ba41f0e0449bfdd176f2655f1e5ddd` |

The raw run hashes identify local temporary captures. They are not committed.
Only relevant readings and counters are retained here; banners, IRQ listings,
JIT diagnostics and build paths are omitted. No historical hash was replaced.
No personal identifier was added. Formatting ran only as
`rustfmt --edition 2021 esp32/src/adc.rs`; status confirmed no unrelated changes.

Port application had two setup errors, both resolved. Direct `git apply` reported
`error: patch failed: esp32/src/lib.rs:1` and
`error: patch failed: esp32/src/periph.rs:1` because the parent has newer crypto/RMT
code. The three-way application initially reported
`fatal: Unable to create '/private/tmp/esp32sim-review/.git/worktrees/esp32sim-classic-adc2/index.lock': Operation not permitted`.
After access to this worktree's Git metadata was granted, it applied; the three
additive conflicts in `periph.rs` retained both sides. An evidence-writing shell
command failed with `zsh:131: unmatched` followed by a backtick because nested
heredocs reused a delimiter; a unique delimiter fixed it on one retry.
No validation command failed.

To repeat the output assertions after the documented runs, redirect their output
to `/tmp/adc2-main.log` and `/tmp/adc2-pads.log`, then run:

```sh
python3 - <<'PY'
from pathlib import Path
import re, hashlib, json, os
receipt=Path('docs/evidence/classic-esp32-spike-2026-10-02/adc-validation.md').read_text()
main=Path('/tmp/adc2-main.log').read_text()
pads=Path('/tmp/adc2-pads.log').read_text()
old_main=receipt.split('## Main run output')[1].split('## Pad, width')[0]
old_pads=receipt.split('## Pad, width, attenuation and DAC2 output')[1].split('## Input and artifact hashes')[0]
def serial(text):
    return [l for l in text.splitlines() if re.match(r'^(CLASSIC|DAC|ADC sample=|TOUCH sample=|PAD |CONFIG |TOUCH_PAD |\[dac\])',l)]
assert serial(pads)==serial(old_pads)
assert all(l in serial(main) for l in serial(old_main))
adc=re.findall(r'ADC sample=(\d+) millis=(\d+) pin34_raw=(\d+) pin34_mv=(\d+) pin27_raw=(\d+) pin27_mv=(\d+)',main)
touch=re.findall(r'TOUCH sample=(\d+) millis=(\d+) pin=4 value=(\d+)',main)
assert len(adc)==len(touch)==12
for i,row in enumerate(adc):
    assert tuple(map(int,row))==(i,3 if i==0 else i*100+5,1872 if i<3 else 817,1650 if i<3 else 800,1884,1650)
for i,row in enumerate(touch):
    assert tuple(map(int,row))==(i,6 if i==0 else i*100+5,[1000,1000,1000,1000,475,301,301,301,826,1000,1000,1000][i])
print('Arduino output checks passed')
PY
```
