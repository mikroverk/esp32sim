#!/bin/sh
set -eu
# Arduino-ESP32 3.3.11, bundled IDF 5.5.5 and esp-rv32/2601 GCC 14.2.0.
: "${ARDUINO_DATA:?Set ARDUINO_DATA to the Arduino15 package directory}"
: "${BUILD_DIR:?Set BUILD_DIR to an empty build directory inside the checkout}"
: "${CHIP:?Set CHIP to c3 or c6}"
case "$CHIP" in c3|c6) ;; *) exit 2 ;; esac
SOURCE=$(CDPATH= cd -- "$(dirname -- "$0")/Probe" && pwd)
mkdir -p "$BUILD_DIR" "${BUILD_DIR}-tmp"
BUILD_DIR=$(CDPATH= cd -- "$BUILD_DIR" && pwd)
export TMPDIR="${BUILD_DIR}-tmp" SOURCE_DATE_EPOCH=0
FLAGS="-ffile-prefix-map=$ARDUINO_DATA=/arduino -ffile-prefix-map=/$ARDUINO_DATA=/arduino -ffile-prefix-map=$SOURCE=/src -ffile-prefix-map=$BUILD_DIR=/build"
arduino-cli compile --clean --fqbn "esp32:esp32:esp32$CHIP" --build-path "$BUILD_DIR" \
    --build-property "compiler.c.extra_flags=$FLAGS" \
    --build-property "compiler.cpp.extra_flags=$FLAGS" \
    --build-property "compiler.S.extra_flags=$FLAGS" "$SOURCE"
# The flash image contains the ELF hash. Remove debug-only host paths before calculating it.
"$ARDUINO_DATA/packages/esp32/tools/esp-rv32/2601/bin/riscv32-esp-elf-strip" --strip-debug \
    "$BUILD_DIR/Probe.ino.elf" -o "$BUILD_DIR/fixture.elf"
"$ARDUINO_DATA/packages/esp32/tools/esptool_py/5.3.1/esptool" --chip "esp32$CHIP" elf2image \
    --flash-mode dio --flash-freq 80m --flash-size 4MB --elf-sha256-offset 0xb0 \
    -o "$BUILD_DIR/fixture.bin" "$BUILD_DIR/fixture.elf"
