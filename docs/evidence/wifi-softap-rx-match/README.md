# Receive match flags for the station and the access point (classic ESP32, silicon)

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
the station (28 and 29) are not: silicon sets 28 alone. libpp takes both, and the model's station
flags are left as they are: no golden runs the classic station, so a change there would go
untested.

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

The `rxprobe.c` here (SHA-256 `54660de04ba5c4788e8b5a7b62424e4d89d6754b6fa828f9337eeb60a8b657fc`)
is the `station` run's source with its header comment corrected; the code is the same. It
reproduces the `softap` run too: without `join` it counts what the earlier revision counted, with
the joined-BSSID kind empty.

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

## Reproducing

```sh
# from a directory holding rxprobe.c as main/rxprobe.c, with the CMake files of an IDF project
idf.py set-target esp32 && idf.py build && idf.py -p PORT flash monitor
# join a phone or a laptop to "rxctrl-probe" (passphrase "rxctrl-pass"), or type
# "join WORD PASSPHRASE" to join the board's station to a network; the console prints the counts
# every 3 s as: probe: <frame> a1=<kind> a3=<kind> bits31..28=<nibble> n=<count>
```
