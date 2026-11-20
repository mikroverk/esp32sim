# EX267: the networks around the station from the page (#303)

Base: upstream/main `66d33d0d`. The final source is the commit containing this receipt.
Inputs, hashes and the exact commands are in [inputs.json](inputs.json); the checks that ran
and their results in [checks.json](checks.json).

The CLI's `--wifi-nearby` (EX237) puts networks around the station: a scan lists them with their
channel, signal and security, and nothing can join them. The browser build has the virtual access
point (`esp32sim_wifi`) but no way to put networks around it, so a page cannot show a sketch that
scans, picks a network by signal or reports one it cannot reach. This is that export, on the call
the option makes:

- `SocBus::wifi_nearby` (`esp-soc/src/soc.rs`, default: no WiFi): `StationLink::set_beacons` on
  the link `attach_wifi` put the access point on, on the S3, C3, C6 and classic ESP32, returning
  the networks it left out with the reason, or an error without the access point. The CLI keeps
  calling `set_beacons` on the link directly.
- `MachineApi::wifi_nearby` (`wasm/src/machine.rs`): the bus's, and for the P4 with its C6
  co-processor the C6's, as `attach_wifi` does there.
- `esp32sim_wifi_nearby(e, ptr, len)` (`wasm/src/s3.rs`, beside `esp32sim_wifi`): UTF-8 text with
  one `--wifi-nearby` spec per line (blank lines skipped; no lines for none), parsed by
  `BeaconAp::parse_all` as the option's specs are, then handed over, replacing the networks there
  were. After `esp32sim_wifi` (a new one starts with none around), before or after boot. A network
  with the access point's own SSID or BSSID is left out as the option leaves it out, with
  `[emu] wifi nearby: 'NAME' left out: it has the access point's SSID` on `host_log`. 0, or 1 for
  text that is not UTF-8, a spec or list the option refuses (an unknown key, two networks on one
  BSSID, more than 32) or no access point, with the reason on `host_log` and the networks
  unchanged.

The CLI's behaviour is untouched. Nothing runs per tick: the networks answer the station's probe
requests, as with the option.

## Firmware

EX237's sketches, [wifi-net-probe.ino](../wifi-nearby/wifi-net-probe.ino) (a scan with channel,
signal and security, then a join of the board's open network) and
[wifi-home-probe.ino](../wifi-nearby/wifi-home-probe.ino) (`WiFi.begin("Home", …)` with a 20 s
timeout), the same S3 builds (hashes in inputs.json match EX237's). From the mask ROM with the
open access point `ssid=Jumperly` (the sketch's network, as in EX237):

- **CLI:** `--wifi ssid=Jumperly --net none --wifi-nearby …`, UART0;
- **page:** [wifi-nearby-run.mjs](wifi-nearby-run.mjs) under Node 24 with the production
  `esp32sim.wasm`: `esp32sim_wifi` with `ssid=Jumperly`, then `esp32sim_wifi_nearby` with the
  same specs at the plan's time, and `esp32sim_run` with the wall clock as `unix_ms` (the CLI's
  SNTP answers from the wall clock too).

| Run | Networks around | UART0 |
| --- | --- | --- |
| scan, 12 s ([plan-scan.json](plan-scan.json)) | EX237's Home (ch 1, -60), Cafe (ch 11, -75, open), Office (ch 6, -82, `bssid=02:00:00:00:00:42`), before boot | CLI and page identical, 21 lines, the `time:` line's wall clock aside: `scan: 4 network(s)`, `Jumperly rssi=-40 ch=6 open`, `Home rssi=-60 ch=1 secured`, `Cafe rssi=-75 ch=11 open`, `Office rssi=-82 ch=6 secured`, then the join, DHCP, DNS, SNTP time and the refused HTTP GET of `--net none` |
| join, 30 s ([plan-join.json](plan-join.json)) | Home (ch 1), before boot | CLI and page identical, 12 lines: `not connected: status=6` after the sketch's 20 s |
| after boot, 12 s ([plan-after-boot.json](plan-after-boot.json)), page only | Home, `ssid=Jumperly,chan=3`, Cafe, at 0.01 s | `[emu] wifi nearby: 'Jumperly' left out: it has the access point's SSID` on `host_log`; `scan: 3 network(s)`: Jumperly, Home, Cafe |
| during the scan, 12 s ([plan-during-scan.json](plan-during-scan.json)), page only | Home (ch 1), Cafe (ch 11), at 0.3 s | `scan: 2 network(s)`: Jumperly and Cafe |

In the last run the list arrives while the scan is under way. Every probe request gets an answer
from every network around, carrying the network's channel; Home on channel 1 is not listed and
Cafe on channel 11 is, as when the scan had already left channel 1 at 0.3 s (the scan ends 0.2 s
earlier than with Home answering, 2.642 s against 2.843 s). The option sets its list before the
run, so this case is the page's only.

The outputs are in [runs/](runs/): `cli-<run>.txt` (the CLI's stdout) and `page-<run>.txt`, the
last two with the emulated time at which the page took each line.

## Tests

`wasm/src/s3.rs`, `wifi_nearby_reaches_the_access_point_of_every_chip_with_a_radio`, for the S3,
C3, C6, classic ESP32 and the P4 with its C6: refused before `esp32sim_wifi`; then three specs
with CRLF, a blank line and the access point's own SSID reach that chip's access point (the C6's
beside the P4) without it; one BSSID twice, an unknown key, text that is not UTF-8 and 33 networks
are refused and leave the networks; 32 are taken; no lines clears them; a new `esp32sim_wifi`
starts with none. The P4 alone has no access point to take them.

Eleven mutations each fail the test: the list not handed to `MachineApi::wifi_nearby`, the specs
parsed one by one without `parse_all`'s checks, text that is not UTF-8 read as empty, lines not
trimmed, blank lines kept, no access point answered 0, the P4's override removed (the P4's own bus
answers), and each chip's `SocBus::wifi_nearby` setting nothing (S3, C3, classic, C6). The
left-out line on `host_log` is shown by the after-boot run only.

## CPU comparison

Not run: no bus, `device_set!`, interrupt routing or per-tick code; the export only calls what
the CLI option calls.

## Limits

- The ABI only: the bundled page (`web/wasm/worker.js`, its manifest) does not call the export
  yet; a `wifi_nearby` worker op beside `wifi` is a follow-up.
- What EX237 says of the networks around holds: beacon-only, answering probe requests, not
  joinable.
- Firmware ran on the S3 only, as in EX237; the C3, C6, classic and the P4's C6 share
  `StationLink::set_beacons`, and the test checks the list reaches each of their access points.
- The S31 keeps the default (an error): it is not in the browser build, and the CLI's
  `--wifi-nearby` does not reach it either.
- With several access points (a further `--wifi`, CLI only; the page's `esp32sim_wifi` puts up
  one) the networks around are the first one's, and one with any access point's SSID or BSSID is
  left out, as `StationLink::set_beacons` has it.
- The firmware images are not committed; their hashes are in inputs.json.
