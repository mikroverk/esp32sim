# C3 BLE advertiser

A NimBLE broadcaster that runs the original ESP-IDF controller. It advertises
`esp32sim` and Battery Service UUID `180f` on channels 37, 38 and 39 with a
100 ms interval plus the controller's random advertising delay. General discovery
with nonconnectable mode selects ADV_SCAN_IND. The emulator observes its packets
passively; it does not send scan requests or transmit scan responses.

The committed app is under 400 KB. The test `ble_advertiser_c3` in
`cli/tests/goldens.rs` boots it with the CI-fetched C3 rev3 ROM, without an app ELF,
hooks or external firmware. It pins the observer lines and instruction count for
two modeled seconds, including wrap of the controller's 16-entry event table.

## Rebuild

Use ESP-IDF **v5.5.5**, commit `b774170ff46c393eeb5e495ea37936038d3f4f4f`,
including its pinned submodules, and its recommended RISC-V GCC
**esp-14.2.0_20260121**. Set `IDF_PATH` to that checkout and install its tools:

```sh
"$IDF_PATH/install.sh" esp32c3
. "$IDF_PATH/export.sh"
cd examples/c3-ble-advertiser
idf.py -B build -DIDF_TARGET=esp32c3 build
P=../../web/wasm/fw/public
cp build/bootloader/bootloader.bin "$P/c3-ble-bootloader.bin"
cp build/partition_table/partition-table.bin "$P/c3-ble-ptable.bin"
cp build/ble_advertiser.bin "$P/c3-ble-advertiser.bin"
```

`CONFIG_APP_REPRODUCIBLE_BUILD=y` removes compile timestamps and maps source paths.
CMake fixes the application version to `1`, independent of the checkout's Git
version. Two builds in different source and output directories gave identical
bootloader, partition table and application bytes. Hashes and tool versions are
in [the EX211 receipt](../../docs/evidence/ble-c3-advertising/inputs.json).
The binary licences are in
[c3-ble-NOTICE.txt](../../web/wasm/fw/public/c3-ble-NOTICE.txt).

From the repository root, after `tools/fetch-rom-elfs.sh web/wasm/fw`:

```sh
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release \
  -p esp32sim --test goldens ble_advertiser_c3 -- --ignored
node wasm/tests/ble-api.mjs web/wasm/esp32sim.wasm web/wasm/fw/public \
  web/wasm/fw/esp32c3_rev3_rom.elf
```

Only an intentional firmware/model change should regenerate these two new goldens.
This checks modeled advertising and interrupt progress; it does not validate RF
behavior or timing against hardware.
