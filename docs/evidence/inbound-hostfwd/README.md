# Inbound port forwarding (`--hostfwd`)

**Question.** Until now the emulated guest could reach out (`--net nat`) but nothing could reach a
server *inside* the guest: firmware that runs a web server could only be exercised by driving its
UART or by watching what it sent outward. Does a host listener carried into the guest's own stack
behave like a client on the virtual subnet — same bytes, same connection lifecycle, no leaked state
— without disturbing the NAT, DHCP or ARP that were already there?

**Answer.** Yes. `--hostfwd tcp:127.0.0.1:PORT-:80` puts a host TCP listener's connections into the
guest as an ordinary same-subnet client, `10.0.2.4` (`02:53:49:4d:00:04`), and every byte the guest
sends arrives at the host socket unchanged. Measured on an unmodified released ESP32-S3 firmware
image: 33 requests returned the guest's 43 104-byte page (32 of them in parallel, all byte-identical
to a baseline), five connections aborted mid-request, a half-closed client, 41 of 41 connections
closed, **0 flows left active**, no segments for an unknown peer port, and no guest reset. The
forwarder's byte counts match the client's byte counts exactly, which is the part a 200 status line
would not have proved.

## Code

- `esp-soc/src/hostfwd.rs` — grammar (`Spec::parse`), the listener set, flow admission and the
  accept/pump/expire tick.
- `esp-soc/src/nat.rs` — no second state machine: `Tcp` gained a per-flow `src_mac`,
  `segment_with(options)` (so the SYN can carry an MSS option and a retransmitted SYN keeps it) and
  a `pump` that both directions of the emulator share. Behaviour for guest-initiated flows is
  unchanged; the NAT's own 14 tests pass unmodified apart from the new field in their fixture.
- `esp-soc/src/net.rs` — one extra ARP responder arm for the peer address, one TCP demux arm
  (`dip == 10.0.2.4`), and `poll` runs the forward after the NAT.
- `esp-soc/src/wifi.rs` — the access point's associated station MAC is handed to the forward every
  tick (a flow cannot be framed before association), the end-of-run report gains a `[emu] hostfwd:`
  line, and a guest reboot drops every forwarded flow.
- `cli` — `--hostfwd SPEC` (repeatable), bound during start-up so a busy port fails before the guest
  boots; `Soc::attach_hostfwd` for the four WiFi chips.

## Measurements

Host: Linux x86_64, emulator at the fork's `main` (`cfb710e`) plus this branch, no JIT for S3 (so the
guest's instruction stream is cycle-deterministic), bench run at ≈0.7–0.8× real time.

Guest firmware (unmodified, released): `HeishaMon.ino.esp32-v4.2.2.bin`
`sha256 627aa40588f9cf4c58ffe2cb1188ac47ada72b6ce2e9a4a0e2b80862a5f06099`,
bootloader `2382948f539c8753a3f56f2e9f03ab182803610c138e08e5d8e7fcc822bf9ba2`,
partition table `0a8b5720e7b77ff11f1462458c3a509dee79224e5279898f26d6a2e3ae0517b7`,
LittleFS image `78c4be5d9f8fe7f2979b5aa5411e130fa5716e8f322ef9e534ef625e4e99bdbf`,
S3 mask ROM `esp32s3_rev0_rom.elf` `c0ce0f338d1de1bdc6efbef1591779a2a42c1ab7d759d3c6ae8ae63a7dd34cfd`.

### Byte-exact delivery (`curl`, then the accounting line)

```
[emu] hostfwd: tcp:127.0.0.1:18080 -> 10.0.2.15:80 as 10.0.2.4
[hostfwd] 127.0.0.1:18080 -> guest 10.0.2.15:80 connecting (peer port 49152)
[hostfwd] guest 10.0.2.15:80 accepted (peer port 49152)
[emu] hostfwd: 1 host connections (1 established, 0 refused by the guest, 0 refused for want of a
  slot, 0 abandoned), 0 closed, 1 active, 0 segments without a flow, 79 bytes in, 43482 bytes out
```

The guest's response was 43 482 bytes (page 43 104 + headers) and the forwarded flow reported
exactly 43 482 bytes; `curl` received a body that matches the page the guest generated.

### Half-close: the guest's own truncation, not a dropped byte

Two requests of the same page in one run, one plain and one from a client that sends `FIN` right
after the request:

| client | bytes the client received | `bytes_to_host` for the run |
| --- | --- | --- |
| `Connection: close`, no half-close | 43 482 (headers + body) | |
| request then `shutdown(SHUT_WR)` | 137 (headers only) | |
| total | 43 619 | **43 619** |

The guest abandons the body when it sees the peer's `FIN`: its webserver closes any client whose
`WiFiClient::connected()` is false before the next state step (`src/common/webserver.cpp`, the
`WEBSERVER_CLIENT_*` loop; the guest prints its own `Closing webserver client: 10.0.2.4:…`). The
forwarder's count equals what the client received to the byte, so this is a property of firmware
under test, not of the relay — and it is exactly the kind of bug the forward exists to expose.

### Concurrency and abuse (60 s run, one host listener)

| phase | result |
| --- | --- |
| baseline `GET /` | 200, 43 104-byte body |
| `GET /json` | a JSON object (`{"heatpump":[{"Topic":"TOP0",…`) |
| 32 parallel `GET /` | 32/32 answered, 32/32 bodies byte-identical to the baseline |
| 5 connections aborted mid-request (`SO_LINGER 0` after half a request) | no effect on later requests |
| 1 half-closed client | headers received, body withheld by the guest (above) |
| accounting | `41 host connections (41 established, 0 refused by the guest, 0 refused for want of a slot, 0 abandoned), 41 closed, 0 active, 0 in TIME_WAIT, 0 segments without a flow, 3157 bytes in, 1447032 bytes out` |
| guest health | no `chip reset at`, still reporting `Reset reason: 1` (POWERON) |

The 41 flows are the baseline, the JSON, the 32 parallel gets, the 5 aborts, the half-close, and two
connections from the readiness probe; every one of them closed and none was left behind.

### Determinism

`--max-seconds 12`, the same bench image. Two families: the bench as normally run (`--net nat`, whose
outbound sockets are dialled on worker threads and answered in wall-clock time) and `--net none`,
which removes that wall-clock dependency while keeping WiFi, DHCP, ARP and the guest's own timers.

| run | `--net nat` | `--net none` |
| --- | --- | --- |
| no `--hostfwd`, run 1 | 201 462 696 | 201 459 067 |
| no `--hostfwd`, run 2 | 201 462 696 | 201 459 067 |
| `--hostfwd` configured, no client, run 1 | 201 419 836 | 201 459 067 |
| `--hostfwd` configured, no client, run 2 | 201 462 302 | 201 459 067 |

With the outbound NAT's host sockets out of the picture, all four runs produce **the same** guest
instruction count: configuring `--hostfwd` and leaving it idle changes nothing, and two identical runs
agree. In the NAT family the count moves by ≈43 000 instructions (0.02 %) between runs *with and
without* the option (an earlier series read 201 417 376 / 201 464 727 without it), because the guest
observes a refused outbound dial at a time decided by a host worker thread rather than by the
emulator. That property predates this change and is the reason the reproducible measurement is the
`--net none` family.

With a client attached the count legitimately varies for the same reason: a real socket's bytes
arrive at host wall-clock time, so they land at different emulated timestamps. The state machine
itself is a pure function of the segment sequence it receives, which is what the unit tests pin.
Host-side counters that vary with wall-clock time: none in the forward — its own counters (bytes,
flows, refusals) are updated only from emulator-tick work; the wall-clock dependent parts are the
kernel's socket delivery and the guest's reaction to it.

## Commands

```sh
# emulator (bench flags elided): a loopback listener onto the guest's port 80
esp32sim --chip s3 --boot rom ... --wifi ssid=bench,psk=bench-pass \
         --hostfwd tcp:127.0.0.1:18080-:80 --debug net --max-seconds 60

# host side, after the guest has its DHCP lease
curl -sS -H 'Connection: close' http://127.0.0.1:18080/ -o page.html
curl -sS http://127.0.0.1:18080/json

# the stress client: 32 parallel GETs, aborted connections, a half-closed client
python3 stress_client.py 127.0.0.1 18080 results/stress

# forwarder unit tests
cargo test -p esp-soc --release hostfwd:: nat::
```

## Annex RDS (secondary, time-boxed, did not fit)

`https://cicciocb.com/` publishes an ESP32 BASIC with an on-device web IDE. The vendor's flasher
(`https://flasher.cicciocb.com/dist/`) is a browser app that assembles a chip's flash set from
`updates.cicciocb.com`; for the ESP32-S3 it uses:

| piece | URL | size | sha256 |
| --- | --- | --- | --- |
| bootloader | `https://updates.cicciocb.com/boot/esp32s3/bootloader_dio_80m.bin` | 14 768 | `30190e7f3cc15fab5b8bfce98e7f84855f2fb486a4b0cf75c83bccd98b879bde` |
| app (OTA updater v2.0, 2025) | `https://updates.cicciocb.com/ota/Annex32-S3_qio_OTA_updater_v2.0_by_ciccioCB_(c)_2025.bin` | 517 984 | `27d97e9ef2501efd419977af03f61e47b4a487030e5a76147304ef80c7964115` |
| `boot_app0` | `https://updates.cicciocb.com/boot/boot_app0.bin` | 8 192 | `f94c5d786a7a8fab06ac5d10e33bf37711a6697636dc037559ea19cc410a17f0` |

The partition table is not published as a binary: the flasher generates it in JavaScript
(`allPartitions_V2`, `memorySizeMB "4MB"`: nvs `0x9000`, otadata `0xe000`, app0 `0x10000`+`0x260000`,
app1 `0x270000`+`0x90000`, ffat `0x300000`+`0x100000`). It was rebuilt byte-for-byte from that table
(177 bytes, the four entries plus the MD5 entry).

Where it stopped: the ROM loads the bootloader and jumps to the app (`entry 0x403c98d8`), the app
runs for about 9 s (its SHA accelerator works — the stop dump lands inside `ets_sha_process`), and
then it software-resets itself and loops: `9 914` `chip reset at … cause 0x3 (RTC_SW_SYS_RESET)` in a
40 s run, the same with 8 MB of emulated PSRAM (6 272 resets in 20 s, first at 5.8 s). It prints
nothing on UART0 or USB, and `[emu] net:` shows 0 DHCP leases, so it never joins the virtual access
point and there is no web UI to forward to.

Why that is not a forwarding result: reaching it would need two things this time box did not
cover — (a) a reason for the reset loop, which has no diagnostic output to work from (the app is
not the plain firmware but the vendor's "OTA updater" build, and nothing in the emulator's console
says whether it is a missing partition/filesystem or a peripheral the S3 model does not implement
yet), and (b) a station configuration: the flasher writes no NVS blob, so an Annex module boots as
its own soft AP on `192.168.4.1`, an address the forward deliberately does not use (the guest side is
the station, `10.0.2.15`). Forwarding a port into a guest that is not on our subnet proves nothing,
so the attempt was stopped here rather than papered over.

## Limits

- TCP only. A UDP forward needs no per-flow session and is a separate, smaller feature.
- 64 flows in total; beyond that the listener accepts and closes the connection (counted as
  `refused for want of a slot`) so a client fails at once instead of queueing in the backlog.
- No retransmission cap: a flow whose guest never answers lives until the 120 s idle timeout (or
  until the guest's RST). A guest reboot drops its flows immediately.
- Refusal is `FIN`, not `RST`: `std` exposes no `SO_LINGER` on stable Rust, so a client refused by
  the guest sees end-of-file rather than `ECONNRESET`.
- One station address. Naming a guest address other than `10.0.2.15` is rejected rather than
  silently rewritten.
- The window is fixed at 5840 bytes with an MSS option of 1400; no window scaling, no SACK, no
  congestion control — the same trade the NAT makes (docs/networking-plan.md).
