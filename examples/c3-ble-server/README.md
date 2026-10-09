# C3 BLE GATT server

A small NimBLE server for controller-level scan, connection, supervision timeout
and UUID read tests. It advertises service `4fafc201-1fb5-459e-8fcc-c5c9c331914b`
and sends `BLE Server Example` in SCAN_RSP. Its readable characteristic
`beb5483e-36e1-4688-b7f5-ea07361b26a8` returns `Hello World says Neil`.
A disconnect callback prints the reason and restarts advertising.

Build with ESP-IDF v5.5.5 and its recommended RISC-V GCC
esp-14.2.0_20260121, using the [advertiser's setup](../c3-ble-advertiser/README.md).

```sh
. "$IDF_PATH/export.sh"
idf.py -B build -DIDF_TARGET=esp32c3 build
cp build/ble_server.bin ../../web/wasm/fw/public/c3-ble-server.bin
```

The bootloader and partition table are byte-identical to the advertiser's
committed `c3-ble-bootloader.bin` and `c3-ble-ptable.bin`. The server binary was
rebuilt in two distinct source/build directories with identical bytes.
The app version is fixed to 1 and reproducible builds remove timestamps and
source paths. [Inputs and hashes](../../docs/evidence/ble-c3-connection/inputs.json)
and [binary licences](../../web/wasm/fw/public/c3-ble-NOTICE.txt) are retained.

Run from the repository root:

```sh
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release \
  -p esp32sim --test ble full_ble_server_reconnects_in_ci -- --ignored
node wasm/tests/ble-connection.mjs web/wasm/esp32sim.wasm web/wasm/fw/public \
  web/wasm/fw/esp32c3_rev3_rom.elf
```

CI fetches the ROM and runs both tests. No app ELF, controller hooks, local
Arduino build or radio hardware is needed. The native test covers empty-only
and ATT traffic before timeout, identical advertising/scan responses after it,
and a second connection/read. This verifies modeled radio behavior, not silicon
timing. Unsupported optional data-length extension can produce an informational
NimBLE HCI status; the server uses the default ATT MTU and 27-byte LL payload.
