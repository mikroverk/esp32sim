# C6 WiFi station

The smallest firmware that exercises WiFi end to end on an ESP32-C6: scan, join, take a DHCP
lease, ping the gateway, then report the signal. It is the bring-up specimen for C6 WiFi in the
emulator (`docs/wifi-c6-plan.md`): the same binary runs on the board and in `esp32sim-c6`, and
the two console logs are compared.

Every step is one console line that starts with a fixed word (the values here are examples, not a captured run):

    station: STARTED
    station: SCAN found=7
    station: SCAN 1 ssid="home" channel=6 rssi=-48 auth=WPA2 bssid=aa:bb:cc:dd:ee:ff
    station: CONNECT attempt=1 ssid="home" ESP_OK
    station: CONNECTED channel=6 bssid=aa:bb:cc:dd:ee:ff
    station: GOT_IP ip=192.168.1.23 mask=255.255.255.0 gw=192.168.1.1
    station: PING seq=1 time=4 ms
    station: PING done sent=5 received=5
    station: STATUS state=connected rssi=-49 channel=6
    station: DISCONNECTED reason=201 NO_AP_FOUND

`r` on the console restarts the chip (`RESTART requested on the console`): a software restart
keeps the USB link up, so a listener that is already attached sees the run from its first line,
which the reset button does not give it. A disconnect is retried after two seconds, forever. `STATUS` repeats every five seconds whatever
the state (`connecting`, `associated_no_ip`, `connected`, `disconnected`), so the log is never
silent: a network that associates but withholds DHCP shows as `associated_no_ip`.

## The screen

On the Waveshare ESP32-C6-LCD-1.47 the same state is shown as 20 x 10 characters in landscape:
the network, the state (with the disconnect reason and its number), IP, gateway, signal, the last
ping, and the two strongest networks of the scan. There is no graphics library: a doubled 8 x 8
font, one SPI transfer per changed row, nothing at all while the text is unchanged. The panel is up
before the radio starts, so a failure in WiFi bring-up still leaves `WIFI INIT...` readable.

`CONFIG_STATION_LCD_FLIP` turns the text 180 degrees. `CONFIG_STATION_LCD=n` removes the display
code from the run entirely, which is what a register trace wants.

## Build

ESP-IDF **5.5.4**:

```sh
. ~/esp/esp-idf-v5.5.4/export.sh
cd examples/c6-wifi-station
idf.py menuconfig        # "C6 WiFi station": your network's SSID and passphrase
idf.py build
idf.py -p /dev/cu.usbmodem101 flash monitor
```

The defaults (`esp32sim` / `esp32sim-pass`) are the emulator's virtual access point, the same as
`examples/wifi-station` uses on the S3. Your own credentials go into the ignored `sdkconfig`, and
into the binary: do not share a build made with real ones. An empty SSID scans every ten seconds
and never joins; an empty passphrase joins an open network.

The trace build has no display traffic and debug logging from the WiFi and WPA libraries:

```sh
idf.py -B build-trace -DSDKCONFIG=sdkconfig.trace \
  '-DSDKCONFIG_DEFAULTS=sdkconfig.defaults;sdkconfig.trace.defaults' build
```

## In the emulator

Build with the default network (the emulator's access point) next to your own configuration:

```sh
idf.py -B build-emu -DSDKCONFIG=sdkconfig.emu '-DSDKCONFIG_DEFAULTS=sdkconfig.defaults' build
```

and from the repository root:

```sh
B=examples/c6-wifi-station/build-emu
target/release/esp32sim-c6 --boot rom --flash-mb 4 --board waveshare-c6-lcd147 --console usb \
  --bootloader $B/bootloader/bootloader.bin --ptable $B/partition_table/partition-table.bin \
  --app $B/c6_wifi_station.bin --elf $B/c6_wifi_station.elf --stub bb_init=0 \
  --wifi ssid=esp32sim,psk=esp32sim-pass --max-seconds 14 --tft-png /tmp/station.png
```

`--stub bb_init=0` is what every C6 radio run needs (the PHY's baseband calibration wants analog
hardware). The unmodified WiFi library then does what it does on the board:

    station: STARTED
    station: SCAN found=1
    station: SCAN 1 ssid="esp32sim" channel=6 rssi=-40 auth=WPA2 bssid=02:53:49:4d:00:01
    station: CONNECT attempt=1 ssid="esp32sim" ESP_OK
    station: CONNECTED channel=6 bssid=02:53:49:4d:00:01
    station: GOT_IP ip=10.0.2.15 mask=255.255.255.0 gw=10.0.2.2
    station: PING seq=1 time=0 ms
    station: PING done sent=5 received=5

`--debug wifi-frames` shows every 802.11 frame in both directions and the WPA2 handshake,
`--debug net` the DHCP, ARP and ICMP behind it; `--wifi ssid=esp32sim` alone is an open network
(build with an empty passphrase for that). Without `--wifi` there is no network and the run ends in
`DISCONNECTED reason=201 NO_AP_FOUND`, as on the board. The PNG is the panel's native portrait
scan, so the landscape text is sideways in it. `external_wifi_station_c6` in
`cli/tests/goldens.rs` pins this run (`C6_WIFI_STATION_BUILD` names the build directory).

## C3 regression input

The same station source also builds for ESP32-C3 with ESP-IDF 5.5.4. Disable the
C6 board display and select C3 in a separate configuration:

```sh
printf 'CONFIG_IDF_TARGET="esp32c3"\nCONFIG_STATION_LCD=n\n' > /tmp/c3-station.defaults
idf.py -B build-c3 -DIDF_TARGET=esp32c3 -DSDKCONFIG=sdkconfig.c3 \
  '-DSDKCONFIG_DEFAULTS=sdkconfig.defaults;/tmp/c3-station.defaults' build
```

From the repository root, run the WPA2 scan/join/DHCP/five-ping golden test:

```sh
C3_WIFI_STATION_BUILD="$PWD/examples/c6-wifi-station/build-c3" \
  cargo +1.99.0 test --release -p esp32sim --test goldens external_wifi_station_c3 -- --exact
```

Set `ESP32SIM_ROM_DIR` to the directory containing `esp32c3_rev3_rom.elf` if it is
not installed with ESP-IDF. The build keeps the project's `c6_wifi_station.bin`
and `.elf` names; only the target changes. No stub or board model is needed.
The test fails with the required input name when its build directory is missing;
CI excludes it with `--skip external_`. Its station lines are pinned in
`tests/golden/wifi-station-c3.station.txt`. This is firmware validation, not a
comparison with a physical C3 radio.

## In the browser

The WebAssembly build runs it too, on the page's Waveshare panel. The firmware is not committed,
so this is a local manifest (`web/wasm/fw/` ignores everything but the public demos): copy the
three parts of `build-emu` to `web/wasm/fw/local/` as `c6-wifi-bootloader.bin`,
`c6-wifi-ptable.bin` and `c6-wifi_station.bin`, and write `web/wasm/fw/c6-wifi-station.json`:

```json
{
 "board": "waveshare-c6-lcd147",
 "flash_mb": 4,
 "psram_mb": 0,
 "wifi": "ssid=esp32sim,psk=esp32sim-pass",
 "display_rotate": 270,
 "stubs": [
  "bb_init=0"
 ],
 "symbols": {
  "bb_init": "0x<address of bb_init in your build>"
 },
 "files": {
  "rom": "esp32c6_rev0_rom.elf",
  "bootloader": "local/c6-wifi-bootloader.bin",
  "ptable": "local/c6-wifi-ptable.bin",
  "app": "local/c6-wifi_station.bin"
 },
 "seconds": 14,
 "expect": "PING done sent=5 received=5"
}
```

`riscv32-esp-elf-nm build-emu/c6_wifi_station.elf | grep ' bb_init$'` gives the address. Then
`tools/wasm-build.sh`, `node tools/wasm-test.mjs c6-wifi-station` (it expects the five pings), and
`python3 -m http.server -d web 8790` with `http://127.0.0.1:8790/run.html?wasm&fw=c6-wifi-station`.
There is no NAT in the browser: the lease, the gateway and its pings are all there is.

## Provenance

`Vernon_ST7789T/` is Espressif's ST7789T panel driver (Apache-2.0), as shipped in Waveshare's demo
for this board. `font8x8_basic.h` is Daniel Hepper's public-domain 8 x 8 font
(github.com/dhepper/font8x8). The pin numbers are the board's schematic. Everything else is new.
