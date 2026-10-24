# The guest as access point (`--wifi-join`)

**Question.** The model puts the guest on the air as a *station* behind a virtual access point, so
firmware that is configured through its own soft AP — it boots, beacons an AP, and serves its setup
page on `192.168.4.1` — could not be reached at all: nothing on that link is on `10.0.2.0/24`, and
`--hostfwd` refused any guest address but the station's. Can the roles be inverted, so the emulated
MAC joins the access point the guest runs, and can a host connection then reach a server behind it?

**Answer.** Yes. `--wifi-join` puts the emulated MAC on the air as a station of the guest's AP:
beacons are heard, open-system authentication and association complete, the WPA2 four-way handshake
runs with the emulator as the *supplicant*, and the guest's own DHCP server leases an address. On
that link `--hostfwd tcp:127.0.0.1:PORT-192.168.4.1:80` carries host connections to the guest's web
server, and the whole configuration flow of a soft-AP firmware — its page, its form POST, its
restart — runs from the host with no serial console involved. The same run then drove the guest's
own WebSocket REPL to configure and draw on the emulated panel, and `--tft-png` captured the frame.

## Code

- `esp-soc/src/wifi/join.rs` — `JoinConfig` (`--wifi-join` grammar) and `Supplicant`: scan, auth,
  association, the four-way handshake in the supplicant role, a DHCP client, ARP/ICMP for the leased
  address, and 802.11 data framing to the DS (protection bit, CCMP header and MIC space once the keys
  are installed).
- `esp-soc/src/wifi.rs` — `StationLink` gains the second link (`attach_join`, `join`, `on_air`,
  `channel`, `log`, `rx_match`); `next_rx`/`nothing_due`/`station_tx` serve whichever side of the air
  exists, and a guest reboot re-joins.
- `esp-soc/src/net.rs` — `VirtualNet` gains a client role: on a join link the emulated subnet's DHCP
  server, gateway, resolver and NTP do not exist, and the only thing left on the host side is a
  forwarded connection (`peer_tcp`).
- `esp-soc/src/hostfwd.rs` (on the inbound-forward branch, see *Branches* below) — `Spec` may name a
  guest address, and `HostFwd` learns the peer address and the guest address from the lease.
- `esp-soc/src/soc.rs`, `cli` — `attach_wifi_join` for the four WiFi chips; `--wifi-join SPEC`
  (excludes `--wifi`); the forward is parsed with no station address on a join link.

## Measurements

Host: Linux x86_64 (4 cores), emulator built with `cargo build --release`, no JIT for the S3. Guest
firmware (unmodified, released): app `645a7e932be51aac212081890ece17ef56fceee829c2e060883525b942389109`,
bootloader `7c2892c8bea5762e3c9f44cfbae154d01f2da93dea97f9238fea4b02a08d35ed`, partition table
`24db2c95993e99f85fb925b8d8d722b113e907b1a7d8597656d233415944c5c6`, `boot_app0`
`f94c5d786a7a8fab06ac5d10e33bf37711a6697636dc037559ea19cc410a17f0`, S3 mask ROM
`esp32s3_rev0_rom.elf` `c0ce0f338d1de1bdc6efbef1591779a2a42c1ab7d759d3c6ae8ae63a7dd34cfd`; 16 MB
flash, 8 MB PSRAM, `--board atech14` (the ST7735 panel the emulator models), `--wifi-join ''`.

### The join, frame by frame

`--debug wifi-frames`, 12 emulated seconds:

```
[wifi] TX slot 0 ... 189 bytes beacon (0/8) a1=ff:ff:ff:ff:ff:ff a2=46:1b:f6:75:dc:e0 a3=46:1b:f6:75:dc:e0 ssid='ESP46:1B:F6:75:DC:E0'
[wifi] RX -> desc 0x3fcf7c34 54 bytes probe-req (0/4) a1=ff:ff:ff:ff:ff:ff a2=02:53:49:4d:00:11 a3=ff:ff:ff:ff:ff:ff
[wifi] TX slot 0 ... 178 bytes probe-resp (0/5) a1=02:53:49:4d:00:11 a2=46:1b:f6:75:dc:e0 ...
[wifi-join] heard AP 'ESP46:1B:F6:75:DC:E0' bssid 46:1b:f6:75:dc:e0 channel 1 (open)
[wifi] RX AUTH/ASSOC -> desc ... 30 bytes auth (0/11) a1=46:1b:f6:75:dc:e0 a2=02:53:49:4d:00:11 a3=46:1b:f6:75:dc:e0
[wifi] TX slot 0 ... 34 bytes auth (0/11) a1=02:53:49:4d:00:11 a2=46:1b:f6:75:dc:e0
[wifi] RX AUTH/ASSOC -> desc ... 66 bytes assoc-req (0/0) a1=46:1b:f6:75:dc:e0 a2=02:53:49:4d:00:11 ssid='ESP46:1B:F6:75:DC:E0'
[wifi] TX slot 0 ... 53 bytes assoc-resp (0/1) a1=02:53:49:4d:00:11 a2=46:1b:f6:75:dc:e0
[wifi-join] associated
[hostfwd] TCP 192.168.4.1:80 ... (the guest's HTTP server answering the host's request)
[wifi-join] DHCP offer 192.168.4.2
[wifi-join] lease 192.168.4.2 mask 255.255.255.0 router 192.168.4.1 dns 192.168.4.1
[wifi-join] joined 'ESP46:1B:F6:75:DC:E0' as 192.168.4.2
```

### The receive-header verdict that made it work

The same run with the station convention (filter bits 28 *and* 29) and with bit 29 alone, at the
same 12 emulated seconds:

| `rx_match` for the guest's AP | probe answered | authentication answered | association |
| --- | --- | --- | --- |
| bits 28 + 29 (the station's convention) | no | no | never |
| bit 29 alone | yes | yes | yes, then DHCP offer and lease |

## Limits

- TCP only, through the forward; the join itself carries ARP, ICMP, DHCP and EAPOL.
- Legacy (non-HT) frames only, one BSSID, no channel switching, no PMF/TKIP.
- The guest's own address is the one the forward uses; a second host on the guest's subnet is not
  modelled.
- Only the AP interface can be joined; a guest that runs a station *and* an AP is joined through the
  AP.
