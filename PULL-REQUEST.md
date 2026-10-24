# `--hostfwd`: inbound port forwarding for the emulated network

A server inside the guest is not reachable from the host: `--net nat` only carries guest-initiated
flows out. Firmware that serves HTTP (or anything else) on the emulated link can therefore only be
exercised through its console. This adds QEMU-compatible inbound forwarding:

```sh
esp32sim --chip s3 --boot rom ... --wifi ssid=demo,psk=demo-pass \
         --hostfwd tcp:127.0.0.1:8080-:80
curl -sS http://127.0.0.1:8080/
```

## What it does

`--hostfwd SPEC` is repeatable and takes `tcp:[HOSTADDR:]HOSTPORT-[:GUESTADDR:]GUESTPORT`. The bind
address defaults to loopback, `0.0.0.0` is honoured only when written out and an empty address is
rejected; the guest side may name the station or stay empty; one host port per specification. Binding
happens while the command line is validated, so a port already in use fails before the guest boots.

The forward is a host of its own on the virtual subnet — `10.0.2.4`, MAC `02:53:49:4d:00:04` — so the
guest sees an ordinary same-subnet client with one ephemeral source port per connection (from 49152
up). `net.rs` gains one ARP responder arm for that address and one TCP demux arm; DHCP (10.0.2.2), the
resolver (10.0.2.3) and the station (10.0.2.15) are untouched, and everything not addressed to the
peer still reaches `nat.rs` unchanged.

The connection state machine is the NAT's, inverted rather than duplicated: `nat::Tcp` already keeps
guest bytes in `to_host` and host bytes in `unacked` regardless of who opened the connection, so an
inbound flow differs only in where its socket comes from (the listener's `accept` instead of a dial
worker) and in the MAC frames are sourced from (a per-flow `src_mac`, which also makes the NAT's
`segment` position-independent). `nat::Tcp` gains `segment_with(options)` so the SYN can carry an MSS
option and a retransmission repeats it, plus a `pump` shared by both sides. The NAT's own tests are
unchanged apart from the new field in their fixture.

Behaviour: cumulative ACK, one retransmission queue for SYN/data/FIN, the guest's advertised window,
MSS-sized segments, FIN/half-close and `TIME_WAIT`; 64 flows in total, 120 s idle timeout, 300 ms
retransmit. At the cap the listener still accepts and closes the connection, so a client fails at once
instead of queueing. A connection that arrives before the station has associated waits (up to 30 s) for
the access point to report its MAC; a guest `RST` closes the host socket and is counted; a segment
addressed to an unused peer port is refused with a `RST` built the way RFC 793 asks; a guest reboot
drops every forwarded flow.

No threads and no per-byte wake-ups: the listener and every flow socket are accepted, read and
written once per `NET_POLL_US` tick, next to the NAT's own socket poll, so inbound traffic reaches the
guest only as frames on the normal 802.11 receive path and the guest's execution stays
cycle-deterministic. With no client connected the tick costs one non-blocking `accept` per listener
and changes no emulated state.

## Tests and evidence

- Ten unit tests in `esp-soc/src/hostfwd/tests.rs`: grammar (including the rejected forms), the MSS
  option and its retransmission, handshake and data both ways, half-close into `TIME_WAIT`, guest
  `RST`, refusal of an unknown peer port, the wait for the station, the flow cap and a guest reset.
- End-to-end on an unmodified released ESP32-S3 image: `GET /` returned a 43 104-byte page and
  `GET /json` a JSON object; 32 parallel GETs all byte-identical to a baseline; five connections
  aborted mid-request and one half-closed client; 41 of 41 connections closed, 0 flows left active,
  0 segments without a flow, no guest reset. The forwarder's byte counts match the client's to the
  byte, which is what a 200 status line alone would not prove.
- Determinism: with `--hostfwd` configured and no client, two runs produce identical guest
  instruction counts. With a client attached the count varies because a real socket's bytes arrive at
  wall-clock time — the same property the NAT already has for real peers.
- Receipt: [docs/evidence/inbound-hostfwd](docs/evidence/inbound-hostfwd/README.md), experiment row
  EX231.

A secondary attempt to reach a third-party BASIC firmware's web IDE through the forward did not get
there and is recorded as such in the receipt (it software-resets itself before printing anything; the
forward was not implicated).

## Checks run

- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings`
- `ESP32SIM_ROM_DIR=<roms> cargo test --release --workspace -- --include-ignored --skip external_`
- `node tools/check-evidence-privacy.mjs`
- The four pre-existing scenarios of an external firmware bench, plus a fifth that starts a forward,
  fetches a page and asserts a string from it: all green. (The bench itself is not part of this
  repository; its scripts are referenced from the receipt.)

## Not in this change

UDP forwarding, window scaling/SACK/congestion control (the NAT has none either), multicast, and a
guest address other than the station's. `std` has no `SO_LINGER` on stable Rust, so a connection
refused by the guest is closed with FIN: the client sees end-of-file rather than `ECONNRESET`.
