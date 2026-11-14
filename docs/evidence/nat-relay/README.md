# EX268: the station's NAT carried by the page (#303)

Base: upstream/main `5466222a`. The final source is the commit containing this receipt.
Inputs, hashes and the exact commands are in [inputs.json](inputs.json); the checks that ran
and their results in [checks.json](checks.json).

#318 put the user-mode NAT's host side behind `NatTransport` as the step towards "a generic
WebSocket relay for the WASM build" (#303). This is the WASM side of that relay: a transport with
no sockets and no threads, whose operations the page takes and whose answers the page delivers,
from whatever network it has.

- `esp_soc::nat::queue` (`QueueTransport`, `QueueHandle`, `HostEvent`): the guest's flows become
  records (TCP_CONNECT, TCP_DATA, TCP_SHUTDOWN, TCP_CLOSE with abort, UDP_OPEN, UDP_DATA,
  UDP_CLOSE, RESOLVE), and the page's answers events (CONNECTED with a write window, FAILED with
  a reason, TCP_DATA, TCP_EOF, TCP_RESET, TX_CREDIT, UDP_DATA, RESOLVED, UNRESOLVED). The NAT's
  TCP (sequencing, retransmission, windows, TIME_WAIT) stays the NAT's: the transport answers its
  non-blocking calls from the queue, `WouldBlock` until the page has answered. Bounds per
  emulator: 16 TCP flows (a 17th SYN is dropped until its retransmission, as with a full
  `StdTransport`), 64 KiB waiting for the guest per flow, 256 KiB of untaken data (writes block
  past it), 16 datagrams per UDP flow. With name lookups on, the page resolves A and AAAA (#318's
  `dns_query`); any other query to 10.0.2.3 is answered here, empty.
- `StationLink::set_nat` and `SocBus::attach_nat` (S3, C3, C6, classic): give the network
  `attach_wifi` put up a NAT, or none, keeping the access point and the lease; with `local_ntp`
  SNTP stays local. The old NAT's flows are aborted first (`Nat::abort_all`), so its transport
  hears of each.
- Exports (`wasm/src/nat.rs`): `esp32sim_nat_relay(e, flags)` (on; the page resolves names; SNTP
  here), `esp32sim_nat_take` / `esp32sim_nat_ptr` (the records), `esp32sim_nat_event`. The P4
  with its C6 attaches it to the C6, as `attach_wifi` does. Switching the relay off or on again,
  or a new `esp32sim_wifi` (which drops the relay with its network), aborts the open flows; the
  next take hands over the old queue's untaken records, those `TCP_CLOSE`s last, before the new
  queue's. The records waiting from replaced queues are bounded: from the first record of a
  replaced queue that would bring them past 256 KiB (`OUT_MAX`), only that queue's closes
  (`TCP_CLOSE`, `UDP_CLOSE`) are kept, as the page may hold those flows open; it takes between
  switches to get the rest.

Without `esp32sim_nat_relay` nothing changes: the page's network stays the subnet without a NAT.
The CLI does not use any of it. Nothing runs per tick.

## Firmware

[wifi-net-probe.ino](../nat-transport/wifi-net-probe.ino) of EX258 (the same build, hashes in
inputs.json): join the open network `esp32sim`, DHCP, `WiFi.hostByName("example.org")`, SNTP from
`pool.ntp.org`, then `GET http://example.org/`, on the S3 for 40 emulated seconds, both from the
mask ROM:

- **page:** [nat-relay-run.mjs](nat-relay-run.mjs) under Node 24 with the production
  `esp32sim.wasm`: `esp32sim_wifi('ssid=esp32sim')`, `esp32sim_nat_relay(1 | 2)`, then after every
  1 ms run slice the records, served by Node's own `net`, `dgram` and `dns` (a stand-in for a page
  relaying them over a WebSocket), and their events before the next slice;
- **CLI:** the same images with `--wifi ssid=esp32sim --net nat` (`StdTransport`, #318), with
  `nameserver 8.8.8.8` as the resolver.

| | Console | Through the host |
| --- | --- | --- |
| page | `ip=10.0.2.15 gw=10.0.2.2 dns=10.0.2.3`, `dns: example.org -> 172.66.157.237`, `http GET http://example.org/ -> 200, 577 bytes`, then the time every 5 s (6 lines) | 1 TCP flow, 1 UDP flow (SNTP), 2 `RESOLVE` (the guest's A queries); 189 bytes out, 1014 in |
| CLI | the same lines (`104.20.26.136`; 7 time lines) | `1 TCP connections (0 failed), 3 UDP flows`, 278 bytes out, 1263 in |

The difference in bytes is the DNS: the CLI relays the guest's three DNS datagrams (lwIP repeats
its `pool.ntp.org` query) as UDP, 89 bytes out and 249 in, where the page answers the two lookups
itself; without them both sides moved 189 bytes out and 1014 in. The page's first SNTP answer
comes later in emulated time, as its lookups take wall time while the emulation runs on (40
emulated seconds took 7 s of wall time), hence one time line fewer. The logs are in [runs/](runs/).

## Tests

- `esp-soc/src/nat/queue.rs`, 11 tests: the record layout (little-endian, padded to 4); the 17th
  connect waiting for a flow to close; writes following the credit and the 256 KiB bound; TCP_DATA
  taking only what fits and EOF after the bytes; a reset after the received bytes, with no close
  record; a failed connect's reason and a silent close; lookups answered once and unknown ones
  ignored; 16 datagrams per UDP flow; other DNS queries answered here, empty; the event decoding;
  and a `Nat` over the queue connecting, relaying and aborting.
- `esp-soc/src/wifi/tests.rs`: `set_nat_needs_the_network_and_keeps_the_access_point`, and
  `set_nat_aborts_the_old_nats_flows` (an open flow's `TCP_CLOSE` with abort in the old queue).
- `wasm/src/nat.rs`, `a_replaced_relay_hands_over_its_records_and_the_closes`: after
  `esp32sim_nat_relay` again, and after a new `esp32sim_wifi`, the next take is the old flow's
  `TCP_CONNECT` and its `TCP_CLOSE` (abort), once.
  `left_over_records_are_bounded_and_the_closes_are_kept`: records that fill the bound exactly
  are kept and the close past it too; where a queue's open does not fit, the datagram after it
  (which would) is dropped with it and its close is kept, as is a TCP flow's close.
- `wasm/tests/abi.rs`, `nat_relay_needs_the_access_point_and_answers_unknown_flows_with_zero`:
  refused before `esp32sim_wifi`; nothing to take; events for flows and lookups that do not exist
  ignored, data for a forgotten flow taken and dropped; off again.

## CPU comparison

Not run: no bus, `device_set!`, interrupt routing or per-tick code. The NAT runs only with
`esp32sim_nat_relay` on, and the CLI path is untouched.

## Limits

- Outbound only. Connections from the page to the guest (the browser's `--hostfwd`) would be one
  more event onto #318's `tcp_accept`; not in this change.
- No relay server: the receipt serves the records from Node's sockets. A page needs its own
  WebSocket (or other) bridge, which this change does not define.
- No multicast: mDNS and other group traffic is not carried.
- One firmware, on the S3. The C3, C6 and classic share `StationLink` and the transport; their
  `attach_nat` was not run with firmware.
