# Receive match flags for the station and the access point (classic ESP32, S3, C3, C6; silicon)

The first measurement (2026-11) was on the classic ESP32, one interface at a time; the second
([AP+STA on four chips](#apsta-on-four-chips), 2026-11-26) repeated it on the classic ESP32, S3,
C3 and C6 with a phone joined to each side at once, and is what the model's flags now follow.

**Question.** The classic ESP32's libpp takes the interface a received frame is for from bits 28 to
30 of the receive header's word 0 (`rx_ctrl`), as read from `wDev_ProcessRxSucData` and
`ppRxProtoProc` in ESP-IDF v5.3.6. The model sets bit 28 on every frame and bit 29 on unicast. A
guest running as an access point dropped the virtual station's frames with those flags, and took
them with bit 29 alone (`--wifi-sta`, `esp32/src/wifi.rs::wifi_rx_fill`). What does silicon set?

**Answer.** Each bit is one interface's match. A match needs the first address to be the
interface's own MAC or a group address, the BSSID to be the interface's or the wildcard, and the
source not to be the interface's own address:

- frames addressed to the SoftAP's MAC (address slot 1) carried bit 29 alone: 3638 of 3638;
- frames addressed to the station's MAC (slot 0), with the station joined to another network,
  carried bit 28 alone: 440 of 440, and so did that network's beacons (432);
- wildcard probe requests (first address and BSSID broadcast), for both interfaces, carried 28 and
  29 together;
- frames of other networks carried neither, and neither did the broadcasts the joined network's
  access point repeated with the station's own address as their source (10). The address compared
  with the BSSID is A3 in a management frame and, inferred from 802.11 addressing since the probe
  counts A1 and A3 only, A2 in a data frame from the distribution system.

Silicon set bit 31 on every frame received; the model leaves it clear. The model's flags for a
frame to the SoftAP (bit 29 alone, `--wifi-sta`) are silicon's. Its flags for a unicast frame to
the station (28 and 29) were not: silicon sets 28 alone. libpp takes both, and the model's station
flags were left as they were at first: no golden runs the classic station, so a change there would
have gone untested. The AP+STA measurement below gave the change its grounds on every chip.

## How it was measured

`rxprobe.c` runs a SoftAP (WPA2-PSK, channel 6) in `WIFI_MODE_APSTA` with promiscuous reception on,
management and data frames. For each frame it counts the top nibble of `rx_ctrl` word 0, read raw
from the promiscuous callback's buffer, by frame type and by what the first address (A1) and the
third (A3) are: the SoftAP's MAC, the station's MAC, a group address, the BSSID the station joined,
or someone else's. `join WORD PASSPHRASE` on the console scans, joins the first network whose SSID
contains WORD, and once it has a lease sends the gateway a datagram every 200 ms, so that the
network sends the station unicast frames.

Built with ESP-IDF v5.5.4 (`espressif/idf:v5.5.4`, GCC esp-14.2.0_20260121) for the classic ESP32
with `CONFIG_ESPTOOLPY_FLASHSIZE_4MB=y` and `CONFIG_ESP_WIFI_SOFTAP_SAE_SUPPORT=n`. Board: a WEMOS
LOLIN32, ESP32-D0WDQ6 revision v1.0, 40 MHz crystal. The counts are in `counts.json`.

| Run | Probe source SHA-256 | Application image SHA-256 |
|---|---|---|
| `softap`: a phone joined to the SoftAP for about a minute and took a lease from its DHCP server; the station started, not connected | `4618658082b2bc78594e48416d2db4700711998c008ae97f3c84eb85b5b0724a`, an earlier revision without `join`, the datagrams or the joined-BSSID kind | `76f2379502b62b94a79ef01cdfff39f87bfdce7a62d534ab10d90b99c0c422a8` |
| `station`: the station joined a phone's hotspot on channel 6 for 90 s; nothing joined to the SoftAP | `bba9029cf8a3815627a54afd7d749ca76a85fdd2be8f01e37e4bd7aee15fe92e` | `6161187696b624321d968f3206f208cca4fdb34fb47376f66c1806b511d4e57b` |

The `rxprobe.c` of these two runs (SHA-256
`54660de04ba5c4788e8b5a7b62424e4d89d6754b6fa828f9337eeb60a8b657fc`) is the `station` run's source
with its header comment corrected; the code is the same. It reproduces the `softap` run too:
without `join` it counts what the earlier revision counted, with the joined-BSSID kind empty. The
`rxprobe.c` here is a later revision (below); built for the classic ESP32 with a UART console and
without `tx`, it runs the same code.

Run `softap`:

| Frame | A1 | A3 | bits 31..28 | Frames |
|---|---|---|---|---|
| data | SoftAP | SoftAP | `1010` | 3508 |
| data | SoftAP | group | `1010` | 112 |
| management | SoftAP | SoftAP | `1010` | 16 |
| probe request | SoftAP | SoftAP | `1010` | 1 |
| data | SoftAP | other | `1010` | 1 |
| probe request | group | group | `1011` | 2940 |
| beacon | group | other | `1000` | 913 |
| management | other | other | `1000` | 889 |
| data | group | other | `1000` | 491 |
| data | other | other | `1000` | 236 |
| probe request | other | other | `1000` | 109 |
| management | group | other | `1000` | 57 |

Run `station`:

| Frame | A1 | A3 | bits 31..28 | Frames |
|---|---|---|---|---|
| data | station | other (the source) | `1001` | 423 |
| management | station | other | `1001` | 13 |
| management | station | joined BSSID | `1001` | 4 |
| beacon | group | joined BSSID | `1001` | 432 |
| management | other | joined BSSID | `1000` | 458 |
| data | group | station (the source) | `1000` | 10 |
| probe request | group | group | `1011` | 664 |
| beacon | group | other | `1000` | 397 |
| management | other | other | `1000` | 162 |
| data | group | other | `1000` | 115 |
| data | other | other | `1000` | 72 |
| beacon | group | other | `1001` | 51 |
| probe request | other | other | `1000` | 23 |
| data | group | other | `1001` | 2 |
| management | group | other | `1000` | 1 |

A3 is the BSSID in management frames only. In a data frame from the distribution system it is the
source, and the BSSID is A2: "data, station, other" are the joined network's frames to the station,
and the 2 group data frames with bit 28 are, by the same inference, its broadcasts from a source
other than its access point. The 51 beacons with bit 28 and an A3 of "other" are the joined
network's: 28 were counted before `WIFI_EVENT_STA_CONNECTED`, which is when the probe learns the
BSSID, and 23 in the 3 s window in which it came.

## Limits

- Promiscuous reception was on. It may change which frames the MAC passes up. It did not change
  the flags of frames for either interface: the frames the guest's stack needs carried one bit.
- One board, one revision, one ESP-IDF version (v5.5.4). The disassembly the model's flags came
  from is v5.3.6's libpp; the emulator runs the guest's access point on both.
- The board's and the phones' MAC addresses, and the hotspot's name, are left out.

## AP+STA on four chips

**Question.** The S3, C3 and C6 models flag frames as the classic's did (bit 29 alone for the
guest's access point, 28 and 29 for unicast to the station), and their station goldens rested on
that without a measurement of their silicon. With the guest's access point and station both on the
air, as `--wifi` and `--wifi-sta` together put them, which bits does each chip set?

**Answer.** The same on all four, and the same as the classic's runs above: a frame addressed to
the SoftAP carries bit 29 alone, a unicast frame to the station bit 28 alone, the joined network's
beacons bit 28, wildcard probe requests 28 and 29 together, and bit 31 is set on every frame. Not
one frame for either interface carried the other's bit. The C6's `esp_wifi_rxctrl_t` names bits 28
to 31 `rxmatch0` to `rxmatch3`, "whether the reception frame is from interface" 0 to 3 (ESP-IDF
v5.5.4, `components/esp_wifi/include/esp_wifi_he_types.h`); the S3's and C3's
`wifi_pkt_rx_ctrl_t` leaves them reserved, as the classic's does. Totals of the run with both sides
joined (`apsta` in `counts-apsta.json`):

| Chip | Seconds | to the SoftAP | unicast to the station | joined network's beacons | wildcard probe requests | broadcasts with the station as source |
|---|---|---|---|---|---|---|
| classic ESP32 | 113 | `1010` 413 of 413 | `1001` 562 of 562 | `1001` 735 | `1011` 829 | `1000` 11 |
| S3 | 121 | `1010` 396 of 396 | `1001` 602 of 602 | `1001` 749 | `1011` 390 | `1000` 10 |
| C3 | 113 | `1010` 273 of 273 | `1001` 538 of 538 | `1001` 405 | `1011` 396 | `1000` 10 |
| C6 | 102 | `1010` 787 of 787 | `1001` 579 of 579 | `1001` 98 | `1011` 159 | `1001` 1 |

The S3 also ran its SoftAP alone (`softap`), the C6 its station alone (`station`) and the C3 both,
with the same flags: the C6's station alone 5337 of 5337 unicast frames `1001` in 798 s, the C3's
86 of 86, and 524 and 283 frames to the C3's and S3's SoftAPs `1010`. The few `1001` frames to the
station in a `softap` run are probe responses to the station's own scans.

One difference between the chips: the broadcasts the joined network repeats with the station's own
address as their source got no bit on the classic ESP32, S3 and C3 and bit 28 on the C6 (1 frame
here, 23 in its `station` run), so the C6 does not leave out its own source. The model sends the
guest nothing of the kind.

Bit 31 is set on every frame with promiscuous reception on, other networks' too. The C6 calls it
interface 3's match, and libpp logs `wifi:ic_enable_sniffer` when promiscuous reception starts; an
S3 beacon read without promiscuous reception (`0x111b20ad`, `esp32s3/src/bus.rs`) has it clear.
Inferred, not measured: bit 31 is the sniffer interface's match, set only with promiscuous
reception on, so the model, which does not model that interface, leaves it clear.

### How it was measured

`rxprobe.c` here (SHA-256 `399dae7105e6b662e124d7ed3853508d07f48267bd0924be576d1ce3ff5b3953`) is
the revision above with two additions: a build with `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y`
(`sdkconfig.usbjtag`) reads the console from USB-Serial-JTAG, for boards with no USB-UART bridge;
and `tx N` on the console caps the transmit power at N quarter-dBm
(`esp_wifi_set_max_tx_power`). Neither touches the receive path. The C6 ran an intermediate
revision with the first addition only (SHA-256
`a695b4d261637b0398b426454c27bfb9506dcaf453e1a4b2ec8ade8ff48b5112`). Each was built with
`espressif/idf:v5.5.4` and `CONFIG_ESPTOOLPY_FLASHSIZE_4MB=y`, `CONFIG_ESP_WIFI_SOFTAP_SAE_SUPPORT=n`.

| Chip | Board's console | Probe source | Application image SHA-256 |
|---|---|---|---|
| ESP32-D0WDQ6 revision v1.0 | UART0, through a USB-UART bridge | `399dae71…` | `962ae5d4c026065f70542a787b5ee95c06d1a98b790d1de873704d7bf88babc0` |
| ESP32-S3 (QFN56) revision v0.2 | USB-Serial-JTAG | `399dae71…` | `da70271a99b81ce8b5160cbb62774afb4627b4e38db667689703db20ffe22ab8` |
| ESP32-C3 (QFN32) revision v0.4 | USB-Serial-JTAG | `399dae71…` | `b9cbdfeb2780e2edb760d874654cce3060c1ff2d1ed6618bcf600436cd452332` |
| ESP32-C6FH4 (QFN32) revision v0.2 | USB-Serial-JTAG | `a695b4d2…` | `6348cae56cfe5d3a2aea615ed72291c9b1aa2296a9916a826bba82c8b09765df` |

For each chip the station joined a phone's hotspot on channel 6 (`join`) and a second phone joined
the SoftAP (`rxctrl-probe`) and took a lease; the windows in `counts-apsta.json` run from the event
that started them (`STA CONNECTED`, `AP JOINED`) to the end of the run. The second phone left and
joined again once in the S3's, C3's and classic's `apsta` runs. On the C3 the station's frames
stopped about 60 s into its `station` run without a disconnection, the hotspot having gone from the
scans; its `apsta` run joined again. Host timestamps from a logger on the serial port delimit the
windows; the counts are the probe's own.

### Limits

- One board of each chip and one ESP-IDF version (v5.5.4); promiscuous reception on, as above.
- The C3 board did not authenticate at full transmit power (the authentication timed out twice,
  reason 2) and joined at 8.5 dBm (`tx 34`); the phone saw its SoftAP only at 13 dBm (`tx 52`).
  The receive flags do not depend on it.
- The S31 was not measured; the model gives it the same flags.
- MAC addresses, network names and the hotspot's passphrase are left out; the counts are by kind
  of address only.

## Reproducing

```sh
# from a directory holding rxprobe.c as main/rxprobe.c, with the CMake files of an IDF project
idf.py set-target esp32 && idf.py build && idf.py -p PORT flash monitor
# join a phone or a laptop to "rxctrl-probe" (passphrase "rxctrl-pass"), or type
# "join WORD PASSPHRASE" to join the board's station to a network; the console prints the counts
# every 3 s as: probe: <frame> a1=<kind> a3=<kind> bits31..28=<nibble> n=<count>
# a board with only its chip's USB port: the console on USB-Serial-JTAG
idf.py set-target esp32c6 && idf.py -D SDKCONFIG_DEFAULTS="sdkconfig.defaults;sdkconfig.usbjtag" build
# "tx 34" caps the transmit power at 8.5 dBm, for a board that does not join at full power
```
