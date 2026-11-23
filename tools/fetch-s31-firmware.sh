#!/bin/sh
# Build the ESP32-S31 images the tests boot, from source, and pin them by SHA-256 (no published
# S31 artifacts exist yet, so this builds instead of downloading).
#   . $IDF_PATH/export.sh   # ESP-IDF v6.1, commit below
#   tools/fetch-s31-firmware.sh [DIR]      default esp32s31/firmware
# s31-hello-*: $IDF_PATH/examples/get-started/hello_world, unmodified, plus
#   tools/s31-hello/sdkconfig.defaults (reproducible build, fixed project version, no build date).
# s31-wifi-*: examples/c6-wifi-station with tools/s31-wifi/main-CMakeLists.txt (no ST7789 driver)
#   and tools/s31-wifi/sdkconfig.defaults (the emulator's virtual access point, no display).
# s31-mosaico-board.bin: examples/mosaico-board as it is (its sdkconfig.defaults and the
#   dependencies.lock that pins espressif/bmm150 1.1.0, fetched by the component manager).
# s31-wifi_softap.bin: examples/wifi-softap with tools/s31-softap/sdkconfig.defaults (the Mosaico's
#   flash and console); its bootloader and partition table are the WiFi station's and hello's.
# s31-mosaico-demo-*: the page demo (web/wasm/fw/s31-mosaico.json), into DEMO_DIR: examples/mosaico-demo
#   (its sdkconfig.defaults and dependencies.lock) with three components of esp-mosaico/esp-mosaico-bsp
#   at BSP_COMMIT in its components/ (esp-mosaico-bsp, and the espressif__spi_nand_flash and
#   espressif__dhara it overrides), whose manifest asks for ESP-IDF >= 6.2:
#   tools/s31-mosaico-demo/esp-mosaico-bsp-idf-6.1.patch lets 6.1 build it, no code changes.
# Built in target/s31-*-build (not committed). A file already in DIR with the right hash is kept.
#   DEMO_DIR=...   default web/wasm/fw/public
set -eu
cd "$(dirname "$0")/.."
DIR=${1:-esp32s31/firmware}
DEMO_DIR=${DEMO_DIR:-web/wasm/fw/public}
BSP_REPO=https://github.com/esp-mosaico/esp-mosaico-bsp
BSP_COMMIT=841f6245850e84ec210ec1818db148e9c3172e4e
IDF_COMMIT=fff9895c82d744c7237be8847347bdd1b07c6643   # v6.1
# file, SHA-256, build output
HELLO="s31-hello-bootloader.bin d60f6fac20b132ffe97194e3f160cb575b0fc52cea744e0a14c06658973d0b68 bootloader/bootloader.bin
s31-hello-ptable.bin 7f00b6c042a89b15b0cac534f82ed988caf29278ff5700b0c511eb1b5bb7c820 partition_table/partition-table.bin
s31-hello_world.bin 6b965db02941e8db4895d27c2ad8eea1733e22ddf9a82ccd4d6f77ae62c6335c hello_world.bin"
WIFI="s31-wifi-bootloader.bin 3d4dbd111b594348ac8e284a646d4a6930aaa239fbdecf65157ae14d99211b66 bootloader/bootloader.bin
s31-wifi-ptable.bin 5295e8c42d6fb5f95e4e8a185932c6191d27cfc5078322475347e40e71af815a partition_table/partition-table.bin
s31-wifi_station.bin e0e0fcfa27528765915a337f1da904c99cf7fe2b63a88042d48e3d839c8fb0c3 c6_wifi_station.bin"
# Its bootloader and partition table come out byte for byte the WiFi build's and hello's, so the
# board set, and the SoftAP set below, reuse those files. Each build checks its own outputs against
# the same pins (build below), so one whose bootloader or ptable differ fails instead of using them.
BOARD="s31-wifi-bootloader.bin 3d4dbd111b594348ac8e284a646d4a6930aaa239fbdecf65157ae14d99211b66 bootloader/bootloader.bin
s31-hello-ptable.bin 7f00b6c042a89b15b0cac534f82ed988caf29278ff5700b0c511eb1b5bb7c820 partition_table/partition-table.bin
s31-mosaico-board.bin 542246f60b5b6f13dd1c18072ea158f496ebbf925edd33c1e757e1e0d1f11b20 mosaico_board.bin"
SOFTAP="s31-wifi-bootloader.bin 3d4dbd111b594348ac8e284a646d4a6930aaa239fbdecf65157ae14d99211b66 bootloader/bootloader.bin
s31-hello-ptable.bin 7f00b6c042a89b15b0cac534f82ed988caf29278ff5700b0c511eb1b5bb7c820 partition_table/partition-table.bin
s31-wifi_softap.bin 92fbbeeed3652226e84d3e8849a86c59c7ae3dcb817c04c0d4268a55bd663ae6 wifi_softap.bin"
DEMO="s31-mosaico-demo-bootloader.bin a0cf5ca20b34d2c4e2deaa0f8c16e23e2fa09cce77f0544db802a16bc74df830 bootloader/bootloader.bin
s31-mosaico-demo-ptable.bin 7f00b6c042a89b15b0cac534f82ed988caf29278ff5700b0c511eb1b5bb7c820 partition_table/partition-table.bin
s31-mosaico-demo.bin ec2c2f899836671907943a3ee55e2d06ec8c16a601a80596a8e8656bba25d00d mosaico_demo.bin"

# The reused files must carry the same pin in every set that names them.
for n in $(printf '%s\n%s\n' "$BOARD" "$SOFTAP" | awk '{print $1":"$2}'); do
  other=$(printf '%s\n%s\n' "$HELLO" "$WIFI" | awk -v f="${n%%:*}" '$1 == f {print $2}')
  [ -z "$other" ] || [ "$other" = "${n#*:}" ] || { echo "${n%%:*}: the board set pins ${n#*:}, the image it reuses is $other" >&2; exit 1; }
done

sha256_ok() {
  if command -v shasum >/dev/null 2>&1; then echo "$2  $1" | shasum -a 256 -c >/dev/null 2>&1
  else echo "$2  $1" | sha256sum -c >/dev/null 2>&1; fi
}
present() {  # IMAGES [DEST]
  for n in $(echo "$1" | awk '{print $1":"$2}'); do sha256_ok "${2:-$DIR}/${n%%:*}" "${n#*:}" || return 1; done
}
# The BSP's components at BSP_COMMIT into the project copy $1, patched for ESP-IDF v6.1.
bsp_components() {
  src=target/esp-mosaico-bsp
  if [ "$(git -C "$src" rev-parse HEAD 2>/dev/null)" != "$BSP_COMMIT" ]; then
    rm -rf "$src"
    git clone --quiet "$BSP_REPO" "$src"
    git -C "$src" checkout --quiet "$BSP_COMMIT"
  fi
  mkdir -p "$1/components"
  for c in esp-mosaico-bsp espressif__spi_nand_flash espressif__dhara; do cp -R "$src/components/$c" "$1/components/"; done
  patch -s -p1 -d "$1" < tools/s31-mosaico-demo/esp-mosaico-bsp-idf-6.1.patch
}
# build NAME SOURCE-DIR DEFAULTS-OR-EMPTY MAIN-CMAKELISTS-OR-EMPTY IMAGES [DEST [PREPARE]]
build() {
  : "${IDF_PATH:?source ESP-IDF v6.1 export.sh first}"
  got=$(git -C "$IDF_PATH" rev-parse HEAD)
  [ "$got" = "$IDF_COMMIT" ] || { echo "IDF_PATH is at $got, need $IDF_COMMIT (v6.1)" >&2; exit 1; }
  B=target/$1-build
  rm -rf "$B"; mkdir -p target
  cp -R "$2" "$B"
  rm -rf "$B/build" "$B/sdkconfig" "$B/sdkconfig.old" "$B/managed_components"
  [ -z "$3" ] || { rm -f "$B"/sdkconfig.*defaults; cp "$3" "$B/sdkconfig.defaults"; }
  [ -z "$4" ] || cp "$4" "$B/main/CMakeLists.txt"
  [ -z "${7:-}" ] || "$7" "$B"
  (cd "$B" && idf.py --preview set-target esp32s31 >/dev/null && idf.py build >/dev/null)
  dest=${6:-$DIR}
  mkdir -p "$dest"
  echo "$5" | while read -r name hash out; do
    sha256_ok "$B/build/$out" "$hash" || { echo "$name: SHA-256 mismatch in the $1 build ($(shasum -a 256 "$B/build/$out" | cut -d' ' -f1))" >&2; exit 1; }
    cp "$B/build/$out" "$dest/$name"; echo "built $name"
  done
}

if present "$HELLO"; then echo "S31 hello images in $DIR: 3 pinned files present"
else build s31-hello "${IDF_PATH:-}/examples/get-started/hello_world" tools/s31-hello/sdkconfig.defaults "" "$HELLO"; fi
if present "$WIFI"; then echo "S31 WiFi station images in $DIR: 3 pinned files present"
else build s31-wifi examples/c6-wifi-station tools/s31-wifi/sdkconfig.defaults tools/s31-wifi/main-CMakeLists.txt "$WIFI"; fi
if present "$BOARD"; then echo "S31 Mosaico board-test images in $DIR: 3 pinned files present"
else build s31-mosaico-board examples/mosaico-board "" "" "$BOARD"; fi
if present "$SOFTAP"; then echo "S31 SoftAP images in $DIR: 3 pinned files present"
else build s31-softap examples/wifi-softap tools/s31-softap/sdkconfig.defaults "" "$SOFTAP"; fi
if present "$DEMO" "$DEMO_DIR"; then echo "S31 Mosaico demo images in $DEMO_DIR: 3 pinned files present"
else build s31-mosaico-demo examples/mosaico-demo "" "" "$DEMO" "$DEMO_DIR" bsp_components; fi
