#!/bin/bash
# bench-wifi.sh BIN_DIR CHIP ROM_DIR: user CPU seconds of the committed WiFi station run (14 s emulated).
# BIN_DIR holds esp32sim, esp32sim-c3 and esp32sim-c6 release binaries; run from the repository root.
d=$1; c=$2; F=$3; P=web/wasm/fw/public
case $c in s3) b=$d/esp32sim; x=(--rom $F/esp32s3_rev0_rom.elf --board none);; c3) b=$d/esp32sim-c3; x=(--rom $F/esp32c3_rev3_rom.elf);; c6) b=$d/esp32sim-c6; x=(--rom $F/esp32c6_rev0_rom.elf --stub 0x4207df7e=0);; esac
{ /usr/bin/time -p $b "${x[@]}" --boot rom --flash-mb 4 --console usb --no-dump --bootloader $P/$c-wifi-bootloader.bin --ptable $P/$c-wifi-ptable.bin --app $P/$c-wifi_station.bin --wifi ssid=esp32sim,psk=esp32sim-pass --net none --max-seconds 14 >/dev/null; } 2>&1 | awk '/^user/{print $2}'
