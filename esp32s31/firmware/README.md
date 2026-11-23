# ESP32-S31 firmware

`tools/fetch-s31-firmware.sh` builds the images here from source with ESP-IDF v6.1
(`fff9895c82d7`) and checks their pinned SHA-256s. They are committed (CI has no IDF v6.1 to
build them; the script verifies them, and rebuilds them when a hash does not match).

- `s31-hello-*`: IDF's `hello_world` with `tools/s31-hello/sdkconfig.defaults`. See
  `docs/evidence/s31-upstream/README.md`.
- `s31-wifi-*`: `examples/c6-wifi-station` with `tools/s31-wifi/` (no display, console on USB
  Serial/JTAG, the emulator's virtual access point `esp32sim` / `esp32sim-pass` and no other
  credentials). Booted by `wifi_station_s31_mosaico` in `cli/tests/goldens.rs`.
- `s31-mosaico-board.bin`: `examples/mosaico-board` (its bootloader and partition table are
  `s31-wifi-bootloader.bin` and `s31-hello-ptable.bin`, byte for byte). Booted by
  `mosaico_board_s31` in `cli/tests/goldens.rs`.
- `s31-wifi_softap.bin`: `examples/wifi-softap` with `tools/s31-softap/sdkconfig.defaults` (16 MB
  flash, console on USB Serial/JTAG, the example's own access point `esp32sim-ap` /
  `esp32sim-pass` and no other credentials); its bootloader and partition table are
  `s31-wifi-bootloader.bin` and `s31-hello-ptable.bin`, byte for byte. Booted by
  `wifi_softap_s31_mosaico` in `cli/tests/goldens.rs`.
