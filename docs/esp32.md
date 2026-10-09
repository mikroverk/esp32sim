# Classic ESP32

The `esp32` crate models the dual-core LX6 classic ESP32 and loads the ECO3 mask ROM. It boots the real
rev-3 mask ROM through the second-stage bootloader into an unchanged application.
The shared Xtensa core enables DFP context registers only for this target.
Double-precision arithmetic and PSRAM are not modeled.

```sh
cargo +1.99.0 build --release -p esp32sim
./target/release/esp32sim --chip esp32 --board esp32dev --boot rom \
  --rom web/wasm/fw/esp32_rev300_rom.elf \
  --bootloader web/wasm/fw/public/classic-hello-bootloader.bin \
  --ptable web/wasm/fw/public/classic-hello-ptable.bin \
  --app web/wasm/fw/public/classic-hello_world.bin --max-seconds 3 --no-dump
```

`hello_world_classic` compares the ROM banner, bootloader and application output
and interrupt totals/per-source counts against committed goldens. The ROM comes from Espressif's
ROM ELF package; set `ESP32SIM_ROM_DIR` to its absolute directory for tests.
The three firmware binaries were built from the existing CC0 hello_world source
with ESP-IDF 5.5.4, the pioarduino 55.03.38-1 platform and the esp32dev board.
See the [reproducible build recipe](../examples/hello_world-classic/README.md).

The core includes DRAM, IRAM, RTC RAM, flash MMUs, DPORT interrupt routing and
secondary-core control, eFuse, SPI flash, three UARTs, GPIO/IO_MUX, RTC control,
and timer groups including LACT. `none` and `esp32dev` select the bare module.
The default board option also selects the bare module for this chip.
WASM accepts the `esp32` chip identifier. Boot applications through ROM.

T0/T1 and LACT use 64-bit counters and the shared timer stepping logic.
LACT sleep-time RTC stepping and timer-group watchdog execution are not modeled. Peripheral extensions are separate changes in the stack.

Classic peripheral extensions include I2C0/1, sixteen LEDC channels,
SPI2/3 with controller-local DMA, and eight RMT channels. SPI and RMT feed
the existing board/display/WS2812 interfaces. `esp32dev-loopback` supplies a loopback board fixture. Hardware fade and RMT receive
are not implemented; transaction-level SPI has no bit-level timing.

Classic Wi-Fi uses the shared `StationLink` access point, network and Ethernet
relay. An inline DPORT clock gate guards service. MAC/PHY register behavior is inferred;
calibration completion is modeled without RF arithmetic or hardware timing.

Classic ADC1/ADC2 accept the existing `adc` and `adcwave` script inputs and
host raw-count API. DAC25/26 expose nominal DC millivolts in the report.
`0 touchpad 4 1` touches GPIO4's capacitive pad; `0 touchpad 4 0` releases it.
This is separate from panel coordinates supplied by `touch`. ADC calibration
and touch counts are nominal models; see EX227 for limits.

Classic `--ble` uses the shared VHCI/HCI controller with an ELF containing
controller symbols and function sizes. It installs a guest FreeRTOS task
using the same windowed-Xtensa trampoline as S3. This is a virtual controller,
with the shared one-link, MTU-23 and no-pairing limits; see EX228.
