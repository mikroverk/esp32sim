# WiFi SoftAP

The smallest firmware that runs the guest as a WiFi access point: start it, then report who joins,
what the DHCP server leases them and who leaves. It is the specimen for the emulator's virtual
station (`--wifi-sta`, `docs/wifi-plan.md`): the same binary runs on a classic ESP32 board, where a
phone joins it, and in `esp32sim`, where the virtual station does. Built for the S3, C3, C6 and
S31, it is the specimen on those chips too.

Every event is one console line that starts with a fixed word (the values here are examples, not
a captured run):

    softap: STARTED ssid="esp32sim-ap" channel=6 auth=WPA2 bssid=24:6f:28:00:11:23
    softap: JOINED mac=02:53:54:41:00:01 aid=1
    softap: LEASED ip=192.168.4.2 mac=02:53:54:41:00:01
    softap: STATUS stations=1
    softap: LEFT mac=02:53:54:41:00:01 aid=1 reason=3

`STATUS` repeats every five seconds, so the log is never silent.

The access point is WPA2-PSK with protected management frames optional and no SAE, as
Arduino-ESP32 3.x's `WiFi.softAP` configures it; an empty passphrase makes it open.

## Build

ESP-IDF **5.5.4**:

```sh
. ~/esp/esp-idf-v5.5.4/export.sh
cd examples/wifi-softap
idf.py set-target esp32
idf.py build
cp build/bootloader/bootloader.bin ../../web/wasm/fw/public/classic-softap-bootloader.bin
cp build/partition_table/partition-table.bin ../../web/wasm/fw/public/classic-softap-ptable.bin
cp build/wifi_softap.bin ../../web/wasm/fw/public/classic-wifi_softap.bin
```

`sdkconfig.defaults` turns on `CONFIG_APP_REPRODUCIBLE_BUILD`, sets 4 MB of flash and leaves out
SAE. Two clean builds in different build directories produced identical binaries
(SHA-256, ESP-IDF v5.5.4 with GCC esp-14.2.0_20260121):

    af19a316afca45afda4a9f6acde35beb516fee1b751101e04c86448bc5638af8  classic-wifi_softap.bin
    9e63604f9601cfd49fbeed2deb5da27a9f6a96459d31d601b493bb108b28df93  classic-softap-bootloader.bin
    7f00b6c042a89b15b0cac534f82ed988caf29278ff5700b0c511eb1b5bb7c820  classic-softap-ptable.bin

The licences linked into them are in
[the notice](../../web/wasm/fw/public/classic-softap-NOTICE.txt).

The S3, C3 and C6 images are the same source built the same way with `idf.py set-target esp32s3`
(`esp32c3`, `esp32c6`), copied to `web/wasm/fw/public/{s3,c3,c6}-softap-bootloader.bin`,
`{chip}-softap-ptable.bin` and `{chip}-wifi_softap.bin`. Two clean builds in different build
directories produced identical binaries (ESP-IDF v5.5.4, GCC esp-14.2.0_20260121):

    356a755c305e43617fd0c569db6ecfd08548e0737286b2fd8163360697329e83  s3-wifi_softap.bin
    6d43fceaee2912d867944abdf755cdb726a6ba7fa95cf801be38f20ec017cc75  s3-softap-bootloader.bin
    58ccc8772c0e5f55aef3e23923c8bd02b66a3ce27e2063ff3536edb8fa2f392f  c3-wifi_softap.bin
    c2ea22671538d14e0c46b4f84e6d3f07912e307dc56c5311d2dc3a20c69f6a37  c3-softap-bootloader.bin
    7c0e11b13b47f09bd68bab6ee197be00431e742e3d9f7028d2e38022dbbc92b3  c6-wifi_softap.bin
    f55b5524535b030592dc05c4541f58db4f2cbc291e76d154cabcdb71dc01d16c  c6-softap-bootloader.bin

The partition tables are the classic's, byte for byte. Their licences are in
[their notice](../../web/wasm/fw/public/softap-NOTICE.txt). The S31 image
(`esp32s31/firmware/s31-wifi_softap.bin`, ESP-IDF v6.1) is built and pinned by
`tools/fetch-s31-firmware.sh` with `tools/s31-softap/sdkconfig.defaults` (the ESP-Mosaico's 16 MB
flash and USB Serial/JTAG console).

The defaults (`esp32sim-ap` / `esp32sim-pass`, channel 6) are what the golden's station joins:

```sh
target/release/esp32sim --chip esp32 --board esp32dev --boot rom --rom web/wasm/fw/esp32_rev300_rom.elf \
  --bootloader web/wasm/fw/public/classic-softap-bootloader.bin \
  --ptable web/wasm/fw/public/classic-softap-ptable.bin \
  --app web/wasm/fw/public/classic-wifi_softap.bin \
  --wifi-sta ssid=esp32sim-ap,psk=esp32sim-pass --max-seconds 6
```

The golden (`cli/tests/goldens.rs`, `wifi_softap_classic`) runs that from the repository root:

```sh
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo test --release -p esp32sim --test goldens wifi_softap_classic -- --ignored
```

On the other chips the console is on USB Serial/JTAG, for example the C6:

```sh
target/release/esp32sim-c6 --rom esp32c6_rev0_rom.elf --boot rom --flash-mb 4 --console usb \
  --bootloader web/wasm/fw/public/c6-softap-bootloader.bin \
  --ptable web/wasm/fw/public/c6-softap-ptable.bin \
  --app web/wasm/fw/public/c6-wifi_softap.bin \
  --wifi-sta ssid=esp32sim-ap,psk=esp32sim-pass --max-seconds 6
```

`wifi_softap_s3`, `wifi_softap_c3`, `wifi_softap_c6` and `wifi_softap_s31_mosaico` run them.
