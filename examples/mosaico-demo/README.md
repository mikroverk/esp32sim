# ESP-Mosaico demo: a compass on the round AMOLED

An ESP-IDF v6.1 project for Espressif's ESP-Mosaico (ESP32-S31, board v1.2) that runs through
Espressif's [esp-mosaico-bsp](https://github.com/esp-mosaico/esp-mosaico-bsp): LVGL 9 on the
round 480×480 CO5300 AMOLED (GP-SPI2 in QSPI mode, the BSP's TE-synchronised path), the CST9220
touch, the AI button and the Boot key, the vibration motor, the BMI270 and both BMM150. It is the
page demo `s31-mosaico` (`web/wasm/fw/s31-mosaico.json`, `run.html?wasm&fw=s31-mosaico`).

Two pages share a ring of ticks:

- **compass**: the first BMM150's field as a needle (red tip: magnetic north), the heading of the
  screen's top edge, and a ball that rolls to the lowest point of its ring from the BMI270's
  accelerometer (centred when the board lies flat, at the bottom held upright);
- **sensors**: accelerometer, gyro and both magnetometers as numbers, and the input counters.

Touch anywhere and a ring follows the finger. The AI button (GPIO 7) changes the accent colour and
pulses the motor for 120 ms; the Boot key (GPIO 61) switches the page. Every input is also a
console line starting with `demo: ` (UART0 and USB Serial/JTAG).

## Build

`tools/fetch-s31-firmware.sh` builds it and pins the three images by SHA-256 into
`web/wasm/fw/public/s31-mosaico-demo*.bin` (it rebuilds only when a hash does not match):

    . ~/esp/idf-v6.1/export.sh        # ESP-IDF v6.1, commit fff9895c
    tools/fetch-s31-firmware.sh

The script copies this directory to `target/s31-mosaico-demo-build`, puts three components of
esp-mosaico-bsp at commit `841f6245` (`esp-mosaico-bsp`, and the `espressif__spi_nand_flash` and
`espressif__dhara` it overrides) in its `components/`, and applies
`tools/s31-mosaico-demo/esp-mosaico-bsp-idf-6.1.patch`: the BSP's manifest asks for ESP-IDF 6.2,
which is not released; the patch lets 6.1 build it and changes no code. `dependencies.lock` pins
the registry components (LVGL 9.5.0, esp_lvgl_adapter 0.6.2, esp_lcd_co5300 2.2.0,
esp_lcd_touch_cst9220 0.1.2, bmi270_sensor 0.4.1, bmm150 1.1.0 and the rest).
`CONFIG_APP_REPRODUCIBLE_BUILD` keeps host paths out of the image: two builds in different
directories give the same bytes.

The images' licences (LVGL, the BSP and Espressif's components, Bosch's sensor APIs, libpng, zlib,
ESP-IDF) are in `web/wasm/fw/public/s31-mosaico-demo-NOTICE.txt`. The application is this
repository's, MIT.

No setting is there for the emulator: the BSP's 3.3 V soft start (`CONFIG_BSP_VCC_3V3_RAMP_MS`,
50 ms, an LEDC fade waited for with its end interrupt) runs on the model as on the chip. The demo
has not been run on a physical board.

## Run

    esp32sim-s31 --board mosaico --flash-mb 16 --boot rom --rom esp32s31_rev0_rom.elf \
      --bootloader web/wasm/fw/public/s31-mosaico-demo-bootloader.bin \
      --ptable web/wasm/fw/public/s31-mosaico-demo-ptable.bin \
      --app web/wasm/fw/public/s31-mosaico-demo.bin --imu-motion 1 --web 8080

`--imu-motion 1` runs the board's 30 s handling script (tilt, flip, shake), which turns the
magnetometers' field with the board, so the needle and the ball move; without it the board lies
still. In a script, `touch 300 200 1` / `touch 300 200 0`, `press ai 150` and `press boot 150`
drive the inputs; the golden `mosaico_demo_s31` (`cli/tests/goldens.rs`) scripts them all.

In the browser the manifest sets `imu_motion: 1`; the page's two buttons are the AI button and the
Boot key, and its indicator shows the motor.
