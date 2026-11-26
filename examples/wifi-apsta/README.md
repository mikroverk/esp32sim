# WiFi AP+STA

The guest as an access point and a station at once (`WIFI_MODE_APSTA`): its access point takes
clients and leases them addresses while its station joins another network, takes a lease there
and pings its gateway. It is the specimen for `--wifi` and `--wifi-sta` together
(`docs/wifi-plan.md`): `--wifi-sta`'s virtual station joins the guest's access point while the
guest's station joins `--wifi`'s.

Every event is one console line that starts with `apsta: ` and a fixed word (the values here are
from a run of the classic ESP32 build in the emulator):

    apsta: STARTED ssid="esp32sim-ap" channel=6 auth=WPA2 bssid=24:6f:28:00:11:23
    apsta: JOINED mac=02:53:54:41:00:01 aid=1
    apsta: LEASED ip=192.168.4.2 mac=02:53:54:41:00:01
    apsta: UPLINK CONNECTED ssid="esp32sim" channel=6
    apsta: UPLINK GOT_IP ip=10.0.2.15 gw=10.0.2.2
    apsta: PING done sent=5 received=5
    apsta: STATUS stations=1 uplink=2

`LEFT` and `UPLINK DISCONNECTED` report the other way; `STATUS` repeats every five seconds. The
access point is `examples/wifi-softap`'s (WPA2-PSK, PMF optional, no SAE, channel 6 until the
station joins, when ESP-IDF moves it to the uplink's); the station joins `esp32sim` /
`esp32sim-pass`, the emulator's `--wifi ssid=esp32sim,psk=esp32sim-pass`. `menuconfig` ("WiFi
AP+STA") changes either side and the number of pings.

## Build

ESP-IDF **5.5.4**, with `examples/wifi-softap`'s `sdkconfig.defaults` (reproducible build, 4 MB of
flash, no SAE):

```sh
. ~/esp/esp-idf-v5.5.4/export.sh
cd examples/wifi-apsta
idf.py set-target esp32      # esp32s3, esp32c3, esp32c6
idf.py build
cp build/wifi_apsta.bin ../../web/wasm/fw/public/classic-wifi_apsta.bin   # s3-, c3-, c6-
```

The bootloader and partition table come out byte for byte as `examples/wifi-softap`'s for each
chip, so the images run with `web/wasm/fw/public/{classic,s3,c3,c6}-softap-bootloader.bin` and
`…-softap-ptable.bin`. The committed images, one build each (ESP-IDF v5.5.4, GCC
esp-14.2.0_20260121):

    91edbc46bb89ab596d04a4b77d6c5fe4b0e666797023c4bacfe66e3a95a20e71  classic-wifi_apsta.bin
    469a3e4da43100bafc41936d4ced7a42ba821466509394aa0f0b888db7b6898b  s3-wifi_apsta.bin
    45a93b7f0ae199968ea483cbddca9ad97db6593cee86a29a40c1102b2322de25  c3-wifi_apsta.bin
    b846de415b58126c1609e7dc2fb7f7d69499e635a7db969b690d2d88dcc46138  c6-wifi_apsta.bin

The licences linked into them are in [the notice](../../web/wasm/fw/public/apsta-NOTICE.txt).

## Run

```sh
target/release/esp32sim --chip esp32 --board esp32dev --boot rom --rom web/wasm/fw/esp32_rev300_rom.elf \
  --console uart0 \
  --bootloader web/wasm/fw/public/classic-softap-bootloader.bin \
  --ptable web/wasm/fw/public/classic-softap-ptable.bin \
  --app web/wasm/fw/public/classic-wifi_apsta.bin \
  --wifi ssid=esp32sim,psk=esp32sim-pass --wifi-sta ssid=esp32sim-ap,psk=esp32sim-pass --max-seconds 10
```

On the S3, C3 and C6 the console is on USB Serial/JTAG (`--console usb --flash-mb 4`, and
`--board none` on the S3), as for `examples/wifi-softap`. Each of the four prints the lines above
within 10 emulated seconds. The goldens (`cli/tests/goldens.rs`, `wifi_apsta_classic`,
`wifi_apsta_s3`, `wifi_apsta_c3`, `wifi_apsta_c6`) run them from the repository root:

```sh
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo test --release -p esp32sim --test goldens wifi_apsta -- --ignored
```
