# WiFi hardware emulation (full, unmodified firmware)

Goal (chosen 2026-08-25): run the **unmodified** Espressif WiFi stack — `esp_wifi` + the closed
`libpp`/`libnet80211`/`libphy` blobs — so firmware associates with an access point and moves IP
traffic, with **no application changes and no shim**. The PHY math is *not* emulated; we fake the
radio/analog registers so the blob's calibration passes, and model the **MAC** (the descriptor
rings and interrupt events) plus a **virtual access point** the MAC "hears".

This is reverse-engineering, done against a specimen: `examples/wifi-station` (the IDF station
example, open network, SSID `esp32sim`). Espressif never documented these peripherals; the register
map matches the classic ESP32's as reverse-engineered by **esp32-open-mac** (their `0x3ff73000` is
our `0x60033000`). Everything below was learned by tracing the blob's own accesses with
`--regstat`, `--trace-fn`, `ESP_EMU_DEBUG_WIFI` and disassembly of the ROM/library functions.

## Status

**Firmware joins the virtual AP and gets an IP address — open *and* WPA2-PSK — with the unmodified
blob and no firmware changes** (`--wifi ssid=esp32sim[,psk=...]`):

```
wpa: WPA: Key negotiation completed with 02:53:49:4d:00:01 [PTK=CCMP GTK=CCMP]
wifi:connected with esp32sim, aid = 1, channel 6, BW20, bssid = 02:53:49:4d:00:01
wifi station: got ip:10.0.2.15
```

What is modelled:
- PHY calibration loops satisfied with faked done-bits; scan, open-system auth, association.
- The 802.11 MAC (TX queue registers, RX descriptor ring, interrupt events) — see below.
- **Receive lists handed over one at a time**, as Arduino-ESP32 writes them: the DMA follows the
  `next` pointers, stops when the next descriptor is not software-armed, and restarts at
  `BASE_RX_DSCR` (0x088) when software hands it the next short list. The IDF station example writes
  the base once and keeps a long ring; an Arduino-ESP32 application (IDF 5.1) arms four descriptors at a
  time, and without the hand-over its station never authenticated — 314 of 315 frames were dropped
  ([decisions](decisions.md), [receipt](evidence/wifi-rx-handover/README.md)).
- A **virtual AP** (`esp-soc/src/wifi.rs`): beacons, probe responses, auth/assoc, and the WPA2
  four-way handshake (PMK from the passphrase, PTK derivation, MIC, GTK delivered AES-key-wrapped).
- The **AES accelerator** (`Aes` in `esp-periph/src/aes.rs`, DMA and block mode) — the supplicant unwraps the
  group key with it, so without this peripheral WPA2 stops dead at message 3.
- Crypto primitives (`esp-periph/src/crypto.rs`): SHA-1, HMAC-SHA1, PBKDF2, the 802.11 PRF, AES for
  every key length in both directions, AES key wrap — all checked against RFC/FIPS/802.11i vectors.
- A **virtual network** (`esp-soc/src/net.rs`): DHCP, ARP, ICMP echo, a DNS responder and an SNTP
  server that hands out the host clock, so `esp_netif` reaches `IP_EVENT_STA_GOT_IP` and firmware
  waiting for time gets it.
- A **user-mode NAT** (`esp-soc/src/nat.rs`, `--net nat`, on by default): TCP and UDP flows are
  terminated in the emulator and relayed over ordinary host sockets, the way Contiki-NG's NAT64
  does it — no libslirp, no root, no tun device. Guest name lookups go to the host's own resolver.
- The **RSA/MPI accelerator** (`Rsa` in `esp-periph/src/rsa.rs`) and **SHA over GDMA including SHA-384/512**.
  mbedTLS routes every public-key operation and every certificate digest through them, so without
  both, TLS hangs in the driver's polling loop or fails certificate verification.

**HTTPS works end to end.** The esp32-screen energy panel boots, joins WPA2, takes a DHCP lease,
syncs its clock, resolves `www.elprisetjustnu.se`, fetches two days of prices over TLS (200, 13.6 kB,
96 slots each) and polls the real Home Assistant on the LAN — its energy history, entity states and
control tiles all live:

```
hass: HA reachable, light.pool_pool = off
prices: -> status 200, 13603 bytes ... fetched 2026-08-25: 96 slots
energy: today: 56.3 kWh over 24 h
```

Bulk traffic is **not** encrypted: the emulated MAC presents plaintext framed as CCMP would be
(protected bit, 8-byte CCMP header with the right key id, 8 bytes of MIC space), which is what
firmware sees when real hardware encrypts and decrypts in place.

Bugs found on the way to a TLS session, each one a place where the model was plausible but wrong:

- **`0x818` on the RSA block is an idle status, not the interrupt latch.** The ISR clears the
  interrupt, then the result path waits for `0x818` to read non-zero — latching it deadlocked
  every interrupt-driven modular exponentiation.
- **IP payloads must be trimmed to the header's total-length field.** The 802.11 frame carries a
  4-byte FCS, which the NAT was feeding to the peer as TCP payload; the guest's real request then
  arrived at a sequence number the connection had already moved past.
- **mbedTLS hashes certificates through GDMA, not the block interface** — and reaches for SHA-384,
  which needs the 64-bit core. Digest-shaped garbage came back as `PK verify failed`.
- **The AES accelerator's CTR mode** (block mode 3) was being executed as ECB, which the server
  answered with a fatal alert rather than an error we could read.

Not yet working:
- Multicast and mDNS, so `esp-web.local` and Home Assistant discovery do not cross; use IP
  addresses. TCP and UDP servers in the guest are reachable with `--hostfwd`.
- Hardware AES-GCM (block mode 6) and the DS/HMAC peripherals; mbedTLS has not asked for them yet.
- Roaming, power save, 802.11n rates, multiple stations, WPA3/SAE, PMF.
- The guest as an access point beyond the first steps (below): AP+STA and more than one station.

## The guest as an access point (`--wifi-sta`)

On the classic ESP32, S3, C3, C6 and S31 a guest in `WIFI_MODE_AP` already works with the MAC as
modelled: libpp sends its beacons, probe and authentication responses and EAPOL frames through
the TX queues like any other frame (queue 0 on the classic). What it needs is something on the air that answers, and received frames flagged for
the right interface. `--wifi-sta` puts a virtual station (`esp-soc/src/wifi/sta.rs`) where the
access points would be: it follows the guest's beacons, authenticates and associates, takes the
supplicant's side of the WPA2 four-way handshake (the PTK from the guest's ANonce, the MIC on
message 3 checked before message 4 goes out), and takes a lease from the guest's DHCP server.
Data is framed as CCMP the way `VirtualAp` frames it. Group frames need no group key either: the
MAC hands them over in the clear, framed as CCMP, or for group frames in WPA/WPA2 mixed mode as
TKIP (its 12-byte Michael MIC and ICV where CCMP's MIC is 8), so the GTK in message 3 is not
unwrapped.

The station leaves a network it cannot join alone and the end-of-run line says why: no Privacy bit
when it has a passphrase (or the other way round), WEP or WPA (Privacy without an RSN element),
WPA3-SAE only, protected management frames required. It listens again after a refusal, a
deauthentication or disassociation addressed to it or to every station, 3 s without a beacon, or a
beacon whose timestamp is lower than the last one's. ESP-IDF v5.5.4's `esp_wifi_stop` sends each
station a disassociation (reason 2) and a deauthentication, so a guest that stops and starts its
access point is joined again from its next beacon. It stops trying after three refusals in a row
with the same authentication or association status, until the guest reboots. A wrong passphrase is
not one of them: message 3 of the handshake never comes, the station gives up after 1 s and tries
again a second later, every 2 s or so (`examples/wifi-softap`: 5 attempts in 10 s, a join in the
guest's log each time). It takes the access point's group cipher (CCMP or TKIP, as WPA/WPA2 mixed
mode announces) and needs CCMP among its pairwise ciphers. It drops the broadcasts the access point
repeats to the BSS that it sent itself, as a station does.

A guest in `WIFI_MODE_APSTA` whose station scans is joined too: its access point leaves the
channel for the scans (21 beacons in 12 s against about 117), with gaps under the 3 s the station
waits.

Joined, the station keeps its place. An access point disassociates a station it has not heard
from for its inactive time: `examples/wifi-softap` (ESP-IDF v5.5.4, 300 s by default) logged
`wifi:inactive timer: … leave` and disassociated a station that sent nothing after its lease with
reason 4 at 302 s, having sent it nothing but beacons in between, so no poll to answer. When the
station has sent the access point nothing for 8 s, it sends a null data frame (type 2, subtype 4,
to the DS). Every frame counts, its own DHCP and ARP and the relay's, so a host program that talks
needs none. 8 s is under the shortest inactive time ESP-IDF's SoftAP takes, 10 s
(`esp_wifi_set_inactive_time` returns ESP_ERR_INVALID_ARG below it): set to 10 s, the example
disassociated a silent station 12 s after its last frame and one sending a null frame every 11 s,
and kept one sending every 10 s. With the default, the station stayed joined for 650 s with 81
keep-alives. A BSS Max Idle Period element (IEEE 802.11-2020 9.4.2.79) in the association response
shortens the interval to half its period, never lengthens it; when it says "protected keep-alive
required", the keep-alive is a protected data frame of ethertype 0x0800 with no payload, as
wpa_supplicant 2.11 sends (a null frame on an open network, which has no key). Keep-alives carry
nothing for the guest's network and are not in the capture. ESP-IDF v5.5.4's SoftAP, with
`CONFIG_ESP_WIFI_BSS_MAX_IDLE_SUPPORT` and `bss_max_idle_cfg` set, sends the element only to a
station whose association request carries one, which this station's does not, so against it the
8 s apply. With a request changed locally to carry one, the guest answered with the smaller of the
two periods, disassociated a silent station about 2 s after it, kept one that followed it, and with
"protected keep-alive required" counted the protected data frames and not the null frames.

As the host on the guest's network, the station renews its lease (RFC 2131 4.4.5): at T1 (option
58, or half the lease) a REQUEST to the server, unicast, with `ciaddr` set and neither option 50 nor
54; at T2 (option 59, or seven-eighths) the same REQUEST broadcast; at the lease's end it gives the
address up, ARP stops answering for it, and it starts again with DISCOVER, as after a NAK. T1, T2
and the end count from when the exchange's first REQUEST went out, not from the ACK (RFC 2131
4.4.1); retransmissions keep its `xid`. Renewing and rebinding take a new `xid` each, so a late ACK
to a renewing REQUEST is discarded and a rebinding ACK counts from the first rebinding REQUEST.
ESP-IDF v5.5.4's DHCP server (`components/lwip/apps/dhcpserver/dhcpserver.c`) leases for 120
minutes by default and acknowledges a REQUEST without option 50 whose `ciaddr` is the address it
leased to that MAC, unicast or broadcast; its ACK carries option 51 and neither 58 nor 59. With
the lease set to 1 minute, the example's station renewed every 30 s and kept 192.168.4.2, and the
guest logged each ACK as an assignment. A lease without option 51 is kept and never renewed; a
lease, T1 or T2 under 1 s is taken as 1 s. In relay mode the host program takes its own lease:
with the relay switched on, the station gives up its own lease and DHCP, so nothing goes out under
the program's MAC, and with the relay off again it starts DHCP anew. The end-of-run line counts
keep-alives and renewals (rebinds among them) when there are any; the keep-alives are among its
frames to the access point, not beside them.

Host ports reach the guest's access point through the station (`--hostfwd` beside `--wifi-sta`):
the NAT of `--wifi`'s network goes behind the station instead (`StationLink::set_nat`), and its
forwarded connections and datagrams come from the station's lease and MAC, to the DHCP server that
leased it (option 54 of its ACK) or to the address given. It follows the lease: without one the
forwards wait, and when it goes or changes (the station left, the guest rebooted or stopped its
access point) the forwarded connections are closed. It carries nothing else: the guest's own
connections and datagrams to the station are not relayed. Without `--hostfwd` there is no NAT,
and nothing of it runs.

Not yet: probe requests, so an access point with a hidden SSID is not found; a second station;
AP+STA with both sides on the air. In relay mode the host program is the station:
without four-address frames it sends from and receives at the station's MAC, and a frame it relays
before the station has joined is refused; one the station takes and cannot carry, because it left
the access point in between, is counted in the end-of-run line. The CLI's `--wifi-sta` and
`SocBus::attach_wifi_station` attach a station; the board page and the wasm crate do not yet.

The received frame's flags in `rx_ctrl` word 0 select the interface. In ESP-IDF v5.3.6's classic
libpp, `wDev_ProcessRxSucData` keeps a frame when interface 1 is enabled and bit 29 is set, or
interface 0 and bit 28, or interface 2 and bit 30, and drops it otherwise; `ppRxProtoProc` takes
the interface from the same bits. The guest writes its station MAC to address slot 0 (0x040) and
its access point's, one higher, to slot 1 (0x048). A frame whose first address is slot 1's gets bit
29 alone; every other frame keeps the flags it had (bit 28, and 29 for unicast). The meaning is
inferred from the disassembly, as no header documents it, and was then compared with silicon
([receipt](evidence/wifi-softap-rx-match/README.md)): on a classic ESP32 rev 1.0 in
`WIFI_MODE_APSTA`, every frame addressed to the SoftAP carried bits 31 and 29 and not 28, every
unicast frame to the station (joined to another network) bits 31 and 28 and not 29, wildcard probe
requests (for both interfaces) 28 and 29 together, and frames of other networks neither. A match
needs the first address and the BSSID both, and the source not to be the interface's own address:
the broadcasts an access point repeats to its BSS from the station got no bit, which is what the
virtual station does with them. The model differs from silicon in three places, which libpp takes
and the model keeps, since no golden runs the classic station and a change would go untested:
silicon sets bit 31 on every frame received and the model leaves it clear; unicast to the station
gets 28 and 29 where silicon sets 28 alone; and every broadcast gets 28, also other networks'
beacons (`--wifi-nearby`, a further `--wifi`), which silicon gives no bit.

Verified with ESP-IDF v5.3.6's `softAP` example (open, and WPA2-PSK without SAE and with PMF
optional, as Arduino-ESP32 3.3.9's `WiFi.softAP` configures it) and an Arduino-ESP32 3.3.9
application: the firmware logs the station joining and its DHCP server leasing 192.168.4.2. The
`wifi_softap_classic` golden pins `examples/wifi-softap`. With WPA3-SAE the guest refuses the
association (status 43), as the station offers WPA2-PSK only.

The S3, C3, C6 and S31 MACs (`esp-soc/src/wifi/mac.rs` for the S3 and C3, the C6's and S31's own)
flag received frames in the same `rx_ctrl` word 0 bits (`StationLink::rx_match`). No slot
registers are read there: with a virtual station on the air, a frame whose first address is the
BSSID the station took from the guest's beacons gets bit 29 alone, and every other frame keeps
bit 28, and 29 for unicast, as before. With bits 28 and 29 both set on the station's frames, the
S3's softAP never answered its authentication (three attempts in 6 s, no response); with bit 29
alone the S3, C3 and C6 (ESP-IDF v5.5.4) and the S31 (ESP-IDF v6.1) logged the join and the lease
within the first seconds. Not compared with silicon on these chips. `examples/wifi-softap` built
for each is pinned by `wifi_softap_s3`, `wifi_softap_c3`, `wifi_softap_c6` (the console and counts
as on the classic) and `wifi_softap_s31_mosaico` (the firmware's lines).

## Scope

Getting *unmodified* firmware to "joined + internet" meant walking the blob's state machine to
CONNECTED with frames a real AP would send — no register or flag shortcuts it — and then giving the
resulting Ethernet traffic somewhere to go. Both halves are done; the shim and OpenCores-Ethernet
routes sketched in docs/networking-plan.md were never needed and are not built.

## What was reverse-engineered

MAC register file at **`0x60033000`** (block 0x33) / `0x60034000` (0x34), WDEV at `0x60035000`:

| Register | Meaning |
| --- | --- |
| `0x088` `WIFI_BASE_RX_DSCR` | RX descriptor ring base (hardware fills from here) |
| `0x08c/0x090` | next / last RX descriptor |
| `0x084` bit 0 | RX descriptor reload (restart at base) |
| `0xc3c` / `0xc40` | MAC interrupt events / clear (bit 7 TX done, bits 14/24 RX data) |
| `0xcb0` / `0xcac` | per-queue TX complete / clear |
| `0xca8` / `0xca4` | per-queue TX error / clear |
| `0xd08 − 8·q` `MAC_TX_PLCP0[q]` | queue q descriptor addr; bit 31 triggers TX |
| `0xd14` | `hal_init` handshake (write bit 1, poll bit 0) |
| `0x040/0x060` (per slot) | MAC address / BSSID filters |
| `0x0d8` | RX policy |
| WDEV `0x0c/0x10/0x14/0x18/0x1c` | TSF counter: latch/load and the 64-bit value |
| WDEV `0x118/0x11c` | power interrupt events / clear |

DMA descriptor (esp32-open-mac `dma_list_item`, **confirmed on silicon 2026-08-25** via JTAG on the
Atech board running this specimen): `size:12 length:12 _:6 has_data:1 owner:1`, then `packet` and
`next` pointers. A filled RX descriptor reads `dw0=0xc0..` — the hardware sets has_data (bit 30) and
**leaves owner (bit 31) set**; the register pointers (`0x088/8c/90`) hold the low 20 bits over
`0x3FC0_0000`, while the in-descriptor `packet`/`next` are full addresses. `rx_last` carries a `0x01`
prefix (bit 24). The 48-byte `rx_ctrl` header begins with the signed RSSI in the low byte of word 0. RX buffer = a 48-byte `wifi_pkt_rx_ctrl` header (rssi, rate, channel,
timestamp, `sig_len`, `rx_state`) + the 802.11 frame + 4-byte FCS; word 0 top bits are the
frame-valid / filter-match flags `wDev_ProcessRxSucData` gates on.

## Hardware ground truth

The real Atech board (ESP32-S3, same silicon) is used as the oracle: flash this specimen, let it
receive real beacons off the air, then over the built-in USB-JTAG (`openocd-esp32` + gdb) `halt` and
read the live WiFi MAC registers and RX descriptor ring — `hw/difftest*.sh` show the openocd/gdb
setup. This confirmed the descriptor bit layout (owner stays set), the masked register-pointer format,
and the 48-byte `rx_ctrl` header. Reflashing is reversible: [atech-firmware](https://github.com/joakimeriksson/atech-firmware) rebuilds the synth,
or `esptool write_flash 0 hw/atech/flash-8M.bin` restores the original dump byte-for-byte.

## Tools added for this work

- `--regstat FILE` — per (address, pc, r/w) access counts with the resolved symbol.
- `--trace-fn PREFIX` — log each entry to functions matching a prefix, with args and caller.
- `--stub SYMBOL[=val]` — synthetic return at a function entry.
- `--wifi SPEC` — attach a virtual AP.
- `--poke` script action; `ESP_EMU_FAKE_READ=addr:or[:and],…` runtime register overrides.
- `ESP_EMU_DEBUG_WIFI` (register trace) / `ESP_EMU_DEBUG_WIFI_FRAMES` (frame decode).

References: esp32-open-mac (github.com/esp32-open-mac/esp32-open-mac, `main/hardware.c`,
`main/mac.c`) and its blog (zeus.ugent.be/blog/23-24/open-source-esp32-wifi-mac/); Ebiroll's and
esp32-open-mac's QEMU forks for the classic ESP32 (Apache-2.0, consulted for behaviour only).
