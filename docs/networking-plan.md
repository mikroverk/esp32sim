# Networking

Goal: firmware that needs the network — the Waveshare autopling web UI / `/api/pling`, the
esp32-screen Home Assistant panel, Atech cloud events — runs in the emulator and talks to
real hosts (Home Assistant, price APIs, a browser on the Mac), with no root privileges.

**Status: done, over emulated 802.11.** Firmware associates with a virtual access point through
the unmodified Espressif blob (docs/wifi-plan.md) — and, with `--wifi-join`, the emulated MAC joins
the access point the guest itself brings up — and the Ethernet traffic that comes out of the MAC is
handled by two layers in front of the host network:

- `esp-soc/src/net.rs` — the emulated subnet 10.0.2.0/24 (station 10.0.2.15, gateway 10.0.2.2,
  resolver 10.0.2.3): ARP, DHCP, ICMP echo, DNS and an SNTP server serving the host clock.
- `esp-soc/src/nat.rs` — `--net nat` (the default): everything addressed past the gateway is
  terminated in the emulator and relayed over ordinary host sockets, which is how Contiki-NG's
  NAT64 does it. A guest SYN becomes a `TcpStream::connect` on a worker thread, guest payload is
  written to that socket, socket reads come back as segments the emulator sequences, acknowledges
  and retransmits; UDP flows are a bound `UdpSocket` per (port, destination) with a reply path
  and an idle reaper. Name lookups are forwarded to the host's own first resolver from
  `/etc/resolv.conf`. `--net none` refuses outbound traffic instead (immediate RST).
- `esp-soc/src/hostfwd.rs` — `--hostfwd`: host listeners whose connections are carried *into* the
  guest's stack, using the NAT's connection state machine in the other direction (below).

No libslirp, no tun device, no entitlement, no root. TLS works: the panel fetches
`https://www.elprisetjustnu.se` and polls Home Assistant on the LAN. That needed the RSA/MPI
accelerator, SHA over GDMA (including SHA-384) and AES-CTR — see docs/peripherals.md.

## Inbound: `--hostfwd`

A server inside the guest is reachable from the host through `--hostfwd`, repeatable and spelled
the way QEMU spells it:

```sh
esp32sim ... --wifi ssid=demo,psk=demo-pass --hostfwd tcp:127.0.0.1:8080-:80
curl -sS http://127.0.0.1:8080/
```

`esp-soc/src/hostfwd.rs` is the counterpart of the NAT and uses the same connection state machine
inverted: a non-blocking `TcpListener` on the host, one `nat::Tcp` flow per accepted connection,
`SYN` toward the guest instead of the NAT's dial-and-`SYN|ACK`. The forward is a host of its own on
the virtual subnet — **10.0.2.4**, MAC `02:53:49:4d:00:04` — so `net.rs` answers ARP for that
address and hands every segment addressed to it to the forward; DHCP, the resolver and the station
keep their addresses. The guest sees an ordinary same-subnet client with one ephemeral source port
per connection (from 49152 up).

What it does, in the terms the NAT already documents: cumulative ACK, one retransmission queue for
SYN/data/FIN, the guest's advertised window, 1400-byte segments for 1500-byte frames, FIN/half-close,
`TIME_WAIT`, and a bounded 5840-byte receive queue per flow. Limits: 64 flows (the listener still
accepts and closes the extra connection, so a client fails at once instead of hanging), 120 s idle
timeout, 300 ms retransmit. A connection that arrives before the guest has associated waits up to
30 s for the station's MAC; a guest that answers `RST` closes the host connection, as a closed port
should. No threads: the listener and every flow socket are polled once per `NET_POLL_US` tick, so
inbound traffic reaches the guest only as frames on the normal 802.11 receive path.

Not carried: UDP (a datagram forward needs no per-flow state, so it is a separate small feature),
multicast, and a guest address other than the station's.
## The guest as access point: `--wifi-join`

The other direction of the same link. Plenty of firmware is configured through its *own* access
point — it boots, beacons a soft AP on `192.168.4.1`, and expects a human to join that AP and open
its web UI. The virtual AP above has no counterpart for that, because it puts the guest on the air
as the station; `--wifi-join` swaps the roles instead: the emulated MAC becomes a station that
joins whatever access point the guest brings up.

```sh
esp32sim ... --wifi-join '' --hostfwd tcp:127.0.0.1:8080-192.168.4.1:80
curl -sS http://127.0.0.1:8080/
```

`esp-soc/src/wifi/join.rs` is the station half of the air, and it is the mirror image of
`VirtualAp` line for line:

- **Scan.** Every beacon and probe response is read for the BSSID, the SSID, the DS parameter set
  (channel), the capability field and the RSN element. A probe request goes out every 100 ms while
  nothing matching has been heard. `--wifi-join` filters with `ssid=`, `psk=` and `bssid=`; with no
  filter the first AP that beacons is joined, and with several APs on the air the first match wins
  (the filters exist to make that deterministic rather than to scan comprehensively).
- **Join.** Open-system authentication, association with the AP's own RSN element echoed back, then
  either DHCP (open network) or the four-way handshake.
- **WPA2 as the supplicant.** Message 1 brings the ANonce, message 2 answers with the SNonce, the
  RSN element and a MIC over the 802.1X frame; message 3's MIC is verified before the pairwise key
  is installed, and message 4 closes the exchange. The PMK, PTK, MIC and key-wrap primitives are
  `esp_periph::crypto`, the same ones the authenticator side uses — the roles are inverted, the
  crypto is not duplicated. The test in `wifi/join/tests.rs` runs the supplicant against the
  model's own `VirtualAp`, so the two halves have to agree.
- **DHCP client.** DISCOVER with the broadcast flag, REQUEST for what was offered, and the ACK's
  `yiaddr`, mask, router and DNS become the link's lease.
- **ARP, ICMP.** The guest resolves the leased address before it can answer anything; the join
  answers for it, and replies to echo requests addressed to it.

Two details are worth knowing because they cost the most to find:

- **The receive descriptor's filter bits are role-dependent.** A station hears group traffic as
  match 0 (bit 28) and its own unicast as both matches (28 and 29). An access point hears
  management frames from stations it has never seen, and its hardware reports those as matched by
  address alone: bit 29, *without* bit 28. Firmware in the AP role drops a frame that sets both, so
  the join link cannot reuse the station convention — with both bits set a soft AP answers nothing
  at all (no probe response, no authentication), with bit 29 alone it answers everything.
  `StationLink::rx_match` is where the role decides.
- **The link's clock is the guest's.** Everything here is driven by frames the guest transmits and
  by emulated time; there is no host socket in the join itself, which is why a run with `--wifi-join`
  and no client is deterministic.

The network backend keeps its shape: `VirtualNet` runs in a client role (`client`, `client_mac`)
where the DHCP server, gateway, resolver and NTP of the emulated subnet do not exist, ARP and DHCP
are answered by the join, and the only thing left on the host side is a forwarded connection. On
that link the forward's peer is the address the guest leased, and its guest address is the AP
itself — `--hostfwd tcp:127.0.0.1:8080-192.168.4.1:80` (or the station address, or nothing, on a
`--wifi` link, which still refuses any other address).

Not modelled: 802.11n/ac rates (frames are built as legacy), channel switching, a scan that reports
every AP, PMF/802.11w, and TKIP. A guest that runs both a station and its own AP can only be joined
through the AP one.

## Not there yet

- **Multicast/mDNS**: not carried, so `esp-web.local` and Home Assistant discovery do not resolve;
  use IP addresses.
- **Real LAN presence**: a `--net tap`/vmnet backend would give the guest an address on the real
  network (and mDNS with it), at the cost of root or the macOS vmnet entitlement.
- **RST on refusal**: `std` has no `SO_LINGER` on stable Rust, so a connection refused by the guest
  is closed with FIN; the client sees end-of-file rather than `ECONNRESET`.

## Alternatives considered

- **libslirp** (QEMU's user-mode NAT) instead of the Rust NAT: a C dependency and an FFI thread for
  behaviour we needed only a subset of. The subset turned out to be a few hundred lines.
- **An `esp_wifi` shim or the OpenCores Ethernet MAC** (`CONFIG_ETH_USE_OPENETH`, which IDF ships a
  driver for) instead of emulating the 802.11 MAC: cheaper, but it needs a firmware config change,
  so binaries would no longer be the ones that run on the board. Emulating the MAC kept "unmodified
  firmware" true; neither route is built.
