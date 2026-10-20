# Networking

Goal: firmware that needs the network — the Waveshare autopling web UI / `/api/pling`, the
esp32-screen Home Assistant panel, Atech cloud events — runs in the emulator and talks to
real hosts (Home Assistant, price APIs, a browser on the Mac), with no root privileges.

**Status: done, over emulated 802.11.** Firmware associates with a virtual access point through
the unmodified Espressif blob (docs/wifi-plan.md), and the Ethernet traffic that comes out of the
MAC is handled by two layers in front of the host network:

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
