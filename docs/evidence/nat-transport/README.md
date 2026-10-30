# EX258: a pluggable host transport for the user-mode NAT

Base: upstream/main `2e36224d`. The final source is the commit containing this receipt.
Inputs, hashes and the exact commands are in [inputs.json](inputs.json); the checks that ran
and their results in [checks.json](checks.json).

`Nat` terminated guest flows directly on `std::net` sockets, so only a host with sockets could
carry the guest's traffic. A WebSocket relay for the WASM build (roadmap item 9, #303) needs the
same flows over a different network. This puts the host side behind a trait, with nothing else
changing for `--net nat`:

- `NatTransport` (`esp-soc/src/nat/transport.rs`): `tcp_connect` / `tcp_state` (`Connect::Pending`,
  `Open`, `Failed(ErrorKind)`) / `tcp_read` / `tcp_write` / `tcp_shutdown` / `tcp_close(abort)`,
  `udp_open` / `udp_send` / `udp_recv` / `udp_close`, `resolver()`, and the optional `resolve` /
  `resolved` (`Resolve::Addrs(addrs, ttl)` or `Failed(rcode)`) for a transport that looks names up
  itself. Reads and writes follow non-blocking `std::io` rules.
- `StdTransport` (`nat/std_transport.rs`) is the old code moved as is: `std::net` sockets, a
  connect worker thread per SYN under the process-wide `ConnectPermit` limit of 8, the resolver
  from `/etc/resolv.conf` (1.1.1.1 without one). `Nat::new(log)` is
  `Nat::with_transport(log, Box::new(StdTransport::new()))`.
- The flow's private socket enum became `FlowState { Connecting(id), Connected(id), TimeWait,
  Closed }`. The NAT calls `tcp_close` exactly once per flow (`abort` on a guest RST, a failed
  connect, a host read / write error, `abort_all`; otherwise on TIME_WAIT entry or idle expiry)
  and `udp_close` once per UDP flow (eviction or idle expiry). `tcp_flow_counts` (#288) counts
  `FlowState` the way it counted the socket enum.
- `Nat::dns_query` hands a guest A / AAAA query to a transport that resolves names itself (at
  most 32 waiting, the oldest gives way, 30 s expiry) and answers from the address the guest
  asked, with the query's transaction id; `false` (the std transport) keeps relaying the datagram
  to `resolver()`. The answer is built by `net::packet::dns_reply`, moved out of
  `VirtualNet::dns`, which uses it too (`dns_question` parses the question for both).
- `Nat::abort_all` drops every flow, resetting the transport's; `StationLink::surviving_reboot`
  calls it, since the rebooted guest's sockets are gone.
- `VirtualNet::local_ntp` (default false) answers SNTP from the network clock even with a NAT,
  for a transport that cannot reach an NTP server. It composes with `epoch=`.

The one visible change on the native path: a failed connect logs the error kind
(`connection refused`, `address not available`) instead of the OS message. Nothing runs per
tick: `Nat::poll` runs only while a NAT is attached (`--net nat` with `--wifi`), which the hello
demos and goldens never do. Goldens are unchanged.

## Tests

`esp-soc/src/nat/tests.rs`: the 14 existing socket tests run unchanged on `StdTransport` (SYN /
data / FIN retransmission and cumulative ACK, partial and wrapping ACKs, in-order and idempotent
FIN, bounded guest buffer, short writes and WouldBlock, FIN after host write drain, SYN-ACK
resend, host reads bounded by the window, zero-window probe, connect permits, TIME_WAIT eviction,
UDP capacity, malformed TCP headers, UDP peer filtering). 13 new, against a scripted mock
transport: a SYN opens a transport flow and guest data is written to it; WouldBlock closes the
guest window without losing bytes; transport EOF becomes a guest FIN and the close is graceful;
a refused connect resets the guest and forgets the flow; a full transport drops the SYN until its
retransmission; a guest RST aborts the transport flow; `abort_all` resets every flow; a guest
reboot aborts the NAT's flows; a resolving transport answers DNS from the guest's resolver
address; a failed lookup is NXDOMAIN and an empty one has no records; DNS is relayed when the
transport does not resolve; at most 32 lookups wait and the oldest gives way; `local_ntp` answers
SNTP with a NAT.

`nat/tests/counts.rs` (#288's eight tests) asserts the same counts as before. The two that
injected a connect closure (`tcp_in_with_connect`, which this change removes with the socket
enum) now script the mock: a refused admission, and a ready or refused connect that a snapshot
must not consume. The four that need a real host socket (a guest RST, a disconnected connector,
a host I/O failure, half-close and TIME_WAIT) run on `StdTransport`.

## Firmware

[wifi-net-probe.ino](wifi-net-probe.ino), Arduino-ESP32 3.3.12: scan, join the open network
`esp32sim`, DHCP, `WiFi.hostByName("example.org")`, SNTP from `pool.ntp.org`, then
`GET http://example.org/`. Run with `--wifi ssid=esp32sim --net nat --debug net` for 40 emulated
seconds on main `2e36224d` and on this branch, twice each, on a desktop with internet access and
`nameserver 8.8.8.8` as the resolver file (commands in inputs.json):

| Run | Console | NAT report |
| --- | --- | --- |
| main 1, 2 | `ip=10.0.2.15 gw=10.0.2.2 dns=10.0.2.3`, `dns: example.org -> 8.47.69.7` (run 2: `8.47.69.10`), `time: 2026-10-28 07:36:26 UTC`, `http GET http://example.org/ -> 200, 577 bytes` | 1 TCP connection (0 failed), 3 UDP flows, 278 bytes out, 1263 bytes in; 1 frame ignored |
| branch 1 | the same lines (`8.47.69.7`, `07:36:31 UTC`) | 1 TCP connection (0 failed), 3 UDP flows, 248 bytes out, 1169 bytes in; 0 frames ignored |
| branch 2 | the same lines (`8.6.112.7`) | 1 TCP connection (0 failed), 3 UDP flows, 278 bytes out, 1263 bytes in; 1 frame ignored |

DNS, SNTP and HTTP went through the transport to the real network on both builds; the time is
the host's wall clock. The NAT logs match line for line except in branch run 1. In the other
three runs lwIP sent its `pool.ntp.org` query twice on the same port (30 bytes more out), both
answers came back (94 bytes more in), and the guest answered the late one with an ICMP port
unreachable (the `1 frames ignored` of the network report); in branch run 1 the first answer was
in time, and the host delivered the HTTP response as one 966-byte segment instead of 961 + 5.
Instruction counts vary with that timing: 87.4-87.8 M + 52.6-53.7 M on both builds.

## CPU comparison

Not run: this change touches neither the bus, `device_set!`, interrupt routing nor per-tick
work. Without `--wifi` there is no virtual network and no NAT, so none of the moved code runs on
the hello demos.

## Limits

- `StdTransport` has no portable abortive close: `tcp_close(id, true)` closes like a graceful
  close (as before).
- The resolver file lookup is unchanged (`/etc/resolv.conf`, else 1.1.1.1). Windows resolves the
  path against the current drive; the firmware runs above used a drive mapped (`subst`) to a
  directory holding `etc/resolv.conf`.
- The self-resolving and `local_ntp` paths are exercised by the mock transport only; no
  transport other than `StdTransport` is in this change.
- Inbound connections are not part of the trait yet: #278 (`--hostfwd`) keeps its listeners in
  `Nat`. On the trait they would be one more call, a host-initiated flow the transport hands the
  NAT, with the flow waiting for the guest's SYN-ACK.
- Pure host-side code: no register or silicon claim.
