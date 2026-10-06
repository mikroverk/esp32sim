#!/bin/bash
# bench.sh WORKLOAD BIN_DIR ROM_DIR [MODEL]: one sample, printed as "instructions user_seconds".
# Run from the repository root. BIN_DIR holds release builds of esp32sim, esp32sim-c3 and
# esp32sim-c6; ROM_DIR the mask ROM ELFs; MODEL pocket-tank's model file (pocket only).
# WORKLOAD: wifi-s3 | wifi-c3 | wifi-c6 (the station goldens' runs, 14 s), s3hello (3000 s),
# c3hello | c6hello (30 s), pocket (30 s). The WiFi runs fail unless the station got through.
set -u
w=$1; d=$2; f=$3; p=web/wasm/fw/public
case "$w" in
  wifi-s3) args=("$d/esp32sim" --rom "$f/esp32s3_rev0_rom.elf" --board none);;
  wifi-c3) args=("$d/esp32sim-c3" --rom "$f/esp32c3_rev3_rom.elf");;
  wifi-c6) args=("$d/esp32sim-c6" --rom "$f/esp32c6_rev0_rom.elf" --stub 0x4207df40=0);;
  s3hello) args=("$d/esp32sim" --rom "$f/esp32s3_rev0_rom.elf" --boot rom --board none --bootloader "$p/hello-bootloader.bin" --ptable "$p/hello-ptable.bin" --app "$p/hello_world.bin" --max-seconds 3000);;
  c3hello) args=("$d/esp32sim-c3" --rom "$f/esp32c3_rev3_rom.elf" --boot rom --flash-mb 4 --bootloader "$p/c3-hello-bootloader.bin" --ptable "$p/c3-hello-ptable.bin" --app "$p/c3-hello_world.bin" --max-seconds 30);;
  c6hello) args=("$d/esp32sim-c6" --rom "$f/esp32c6_rev0_rom.elf" --boot rom --flash-mb 4 --bootloader "$p/c6-hello-bootloader.bin" --ptable "$p/c6-hello-ptable.bin" --app "$p/c6-hello_world.bin" --max-seconds 30);;
  pocket) args=("$d/esp32sim" --rom "$f/esp32s3_rev0_rom.elf" --boot rom --board waveshare-amoled18-v2 --flash-mb 16 --psram-mb 8 --bootloader "$p/pocket-tank-bootloader.bin" --ptable "$p/pocket-tank-ptable.bin" --app "$p/pocket-tank.bin" --flash-at "0x290000=${4:?pocket needs MODEL}" --max-seconds 30);;
  *) echo "unknown workload $w" >&2; exit 2;;
esac
case "$w" in wifi-*) c=${w#wifi-}; args+=(--boot rom --flash-mb 4 --console usb --bootloader "$p/$c-wifi-bootloader.bin" --ptable "$p/$c-wifi-ptable.bin" --app "$p/$c-wifi_station.bin" --wifi ssid=esp32sim,psk=esp32sim-pass --net none --max-seconds 14);; esac
out=$(mktemp); err=$(mktemp); trap 'rm -f "$out" "$err"' EXIT
/usr/bin/time -p "${args[@]}" --no-dump > "$out" 2> "$err" || { echo "run failed" >&2; exit 1; }
case "$w" in wifi-*) grep -q 'PING done sent=5 received=5' "$out" || { echo "the station did not get through" >&2; exit 1; };; esac
awk '/^\[emu\] stop/ { if (match($0, /core0 [0-9]+ \+ core1 [0-9]+/)) { split(substr($0, RSTART, RLENGTH), a, " "); i = a[2] + a[5] } else if (match($0, /— [0-9]+ insns/)) { split(substr($0, RSTART, RLENGTH), a, " "); i = a[2] } }
     /^user/ { u = $2 } END { print i, u }' "$err"
