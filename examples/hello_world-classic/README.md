# Classic ESP32 hello

Stock ESP-IDF hello_world, built for esp32dev with ESP-IDF 5.5.4 and
Xtensa GCC 14.2.0_20260121. PlatformIO's pioarduino platform is pinned to
55.03.38-1 in platformio.ini. The example source is CC0-1.0.

From this directory, with PlatformIO Core 6.1.19:

```sh
pio run
cp .pio/build/esp32dev/bootloader.bin ../../web/wasm/fw/public/classic-hello-bootloader.bin
cp .pio/build/esp32dev/partitions.bin ../../web/wasm/fw/public/classic-hello-ptable.bin
cp .pio/build/esp32dev/firmware.bin ../../web/wasm/fw/public/classic-hello_world.bin
```

`CONFIG_APP_REPRODUCIBLE_BUILD=y` removes timestamps and maps source paths;
CMake fixes the project version to `1`. Two clean builds in different source
and output directories produced identical binaries. Hashes are in
[EX223 inputs](../../docs/evidence/classic-core/inputs.json). The linked
licences are in [the notice](../../web/wasm/fw/public/classic-hello-NOTICE.txt).

The CI golden boots the fetched ECO3 ROM through this bootloader into the
application, and pins console and interrupt totals/per-source counts:

```sh
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release \
  -p esp32sim --test goldens hello_world_classic -- --ignored
```

Run that command from the repository root. No board or local SDK is needed
for the test. Rebuilding the firmware needs the pinned SDK/toolchain above.
