# Persistent flash (`--flash-file`, `--flash-out`)

**Question.** A guest keeps its own configuration in flash: NVS with WiFi credentials, display type and
pin assignment, calibration, a filesystem. Every run today starts from the same image and nothing is
written back, so a firmware that was configured once is back at its boot defaults next time — the
"configure once, then test" workflow does not exist. Can the emulator keep the flash a guest writes, so
that the next run starts where the previous one stopped, and without leaving a half image behind when a
run is killed?

**Answer.** Yes. `--flash-file F` loads the array when the file holds an image and writes it back at
exit — atomically, through a temporary file and a rename — and also when `SIGINT`/`SIGTERM` interrupts
the run. `--flash-out F` dumps the final array without changing the run. Proven on an unmodified
released ESP32-S3 firmware, split over runs:

0. **Two runs of one build** (no join needed): the guest booted in station mode from a configured
   flash file, was renamed through its own page (`POST /_kfg_`, `STANAME=bench-two`), and the next run
   — the file and `--wifi ssid=bench-two` and nothing else — came up associated to `bench-two` and
   served its page. That is the whole "configure once, then test" loop; it is section
   [One build, two runs](#one-build-two-runs).
1. **Run A** booted the guest with no saved configuration. It came up in access-point mode, served its
   own configuration page, and its own save path stored `STANAME=heishamon-bench&STAPASS=bench-password`
   in NVS. The emulator wrote the 16 MiB array to `flash-a.bin` at exit (`SIGTERM`, exit status 130).
2. **Run B** started from `flash-a.bin` and nothing else but `--wifi
   ssid=heishamon-bench,chan=6,psk=bench-password`: the guest came up **in station mode**, printed
   `Connecting to heishamon-bench` / `Connected to heishamon-bench` / `IP address: 10.0.2.15`, completed
   the WPA2 handshake with the virtual access point (36 AES-CCMP blocks), took a DHCP lease, and its web
   UI answered through `--hostfwd`. Its own configuration read-back reports the two values run A saved.
3. **Control**: the same firmware, the same images and the same access point on the air, but a flash
   file that was never configured. The guest switched to access-point mode, sent no frame to the
   emulated AP (`state Idle`, 0 data frames) and never associated — so run B's station mode is the
   flash, not the emulator.

A guest that writes nothing leaves the file byte-identical (`cli/tests/flash_file.rs`). A guest that does
write changes it: two run-B runs of the same image hash differently, because the guest writes its own
boot state every time.

## Code

- `esp-soc/src/flash.rs` — the file: `read` (refuse a file larger than the array, pad a shorter one with
  `0xff`), `write` (temporary file beside the target, `sync_all`, rename, directory sync best effort).
  Seven unit tests: round trip at the configured size, `0xff` padding, an empty file as a fresh flash, a
  too-large file refused, a missing file is an error, a replaced image leaves no temporary file, a
  temporary file from a killed run is never read back.
- `esp-soc/src/soc.rs` — `SocBus::read_flash(offset, len) -> Option<&[u8]>` and `flash_len()`; the model
  could only be written to before, never read. One-line implementations for the classic ESP32
  (`esp32/src/soc.rs`), S3 (`esp32s3/src/soc.rs`), C3 (`esp32c3/src/soc.rs`) and C6
  (`esp32c6/src/soc.rs`); `Machine::read_flash`/`flash_len` forward them.
- `cli/src/lib.rs` — `--flash-file`, `--flash-out`, the load/save precedence, `save_flash`, and the
  signal flag. The handler only stores an `AtomicBool` (`signal(2)` declared directly; the workspace has
  no dependencies), and a run that owns a flash file is split into 16 M-instruction chunks so the flag is
  read — a run without one still makes a single `Machine::run` call, so instruction counts and goldens
  are unchanged.
- Tests: `cli/tests/flash_file.rs` (seed + padding + write-back, byte-identical repeat, oversize
  refusal, `--flash-out`, precedence, `SIGTERM` still writes and exits 130) and a cross-chip
  `read_flash`/`flash_len` contract in `cli/tests/soc_contracts.rs`. Both are hermetic: a hand-built C6
  app image that spins, no mask ROM, no firmware build.
- `docs/cli.md` ("Persistent flash"), `docs/decisions.md` (four entries: flash is state, `0xff` padding,
  rename not in-place, a handler can only set a flag).

## Measurements

Host: Linux x86_64, 4 cores, `cargo build --release`, no dependencies added. Emulator: this change on
`main` at `63670bb` (`nvs-flash`).

Guest firmware: a released, unmodified ESP32-S3 image — app `app.bin`
`645a7e932be51aac212081890ece17ef56fceee829c2e060883525b942389109`, bootloader `bootloader.bin`
`7c2892c8bea5762e3c9f44cfbae154d01f2da93dea97f9238fea4b02a08d35ed`, partition table `ptable.bin`
`24db2c95993e99f85fb925b8d8d722b113e907b1a7d8597656d233415944c5c6`, `boot_app0.bin`
`f94c5d786a7a8fab06ac5d10e33bf37711a6697636dc037559ea19cc410a17f0`, S3 mask ROM `esp32s3_rev0_rom.elf`
`c0ce0f338d1de1bdc6efbef1591779a2a42c1ab7d759d3c6ae8ae63a7dd34cfd`. 16 MB flash, 8 MB PSRAM,
`--board none`.

Redactions: the guest's image and boot artifacts are named `app.bin`, `bootloader.bin` and `ptable.bin`,
and its own banner line reads `<firmware banner>`. Hashes, sizes, offsets and byte counts are unchanged,
so the measured artifacts remain identifiable. The access-point names (`heishamon-bench`, `bench-two`)
are arbitrary strings this bench wrote through the guest's page; they are kept because they are the
bytes in NVS and the lines in the console, and the measurement is about them surviving.

Two features the bench needs are not on `main`, so each run used a **local** merge that is not part of
this change and is not for pushing:

| run | build | why |
| --- | --- | --- |
| A | `nvs-flash` + `softap-join-bench` (`--wifi-join`, guest soft-AP) | the guest serves its page on its own access point `192.168.4.1`; without the join nothing can reach it |
| B, control | `nvs-flash` + `hostfwd` | `--hostfwd` is its own change; run B's proof is that the page answers through it |

### One build, two runs

The proof the bench needs, with `--flash-file` and `--hostfwd` and no helper branch: the guest is in
station mode, so its page is reachable through the forward, and the page's own save path is what has
to reach the file.

```sh
# run 1 — the file a previous run left, an access point, and a forward
esp32sim --chip s3 --boot rom --rom esp32s3_rev0_rom.elf --flash-mb 16 --psram-mb 8 --board none \
  --flash-file flash.bin --wifi ssid=heishamon-bench,chan=6,psk=bench-password \
  --hostfwd tcp:127.0.0.1:18103-:80 --max-seconds 60
```

```
[emu] flash: 16777216 bytes from flash.bin (--flash-file)
Connecting to heishamon-bench
Connected to heishamon-bench
IP address: 10.0.2.15

POST /_kfg_ (before):     200: STANAME='heishamon-bench'
POST /_kfg_ (save 'bench-two'): 200
read back:                STANAME='bench-two'
[emu] stop: interrupted by a signal — ... 6.921s emulated, 14.3 Minsn/s
[emu] flash: wrote 16777216 bytes to flash.bin (--flash-file)     exit status 130
```

Run 2 is the same command with `ssid=bench-two`: the guest has nothing but the file and that access
point, so associating to `bench-two` can only come from the name the page wrote in run 1.

```
[emu] flash: 16777216 bytes from flash.bin (--flash-file)
Connecting to bench-two
.........
Connected to bench-two
IP address: 10.0.2.15

POST /_kfg_ (empty): 200: STANAME='bench-two' STAPASS='bench-password'
GET  /config        200, 4223 B gzip
GET  /              200, 2870 B
[emu] wifi: 49 frames sent by the station, 135 received (1 dropped: no descriptor); AP: 67 beacons,
      22 probe responses, 22 data frames from the station, state Associated
[emu] net: 1 DHCP leases, 1 ARP replies, ...
[emu] crypto: 36 AES blocks, 53936 SHA blocks, 0 RSA/MPI operations
[emu] hostfwd: 5 host connections (4 established, 1 refused by the guest, ...), 567 bytes in, 8007 bytes out
[emu] stop: interrupted by a signal — 20.4s wall = 10.0 Minsn/s
```

| file | sha256 | NVS (`0x9000..0xe000`) not `0xff` |
| --- | --- | --- |
| before run 1 (run A's output) | `a25b02cdebe89103fda25c36b54e11d66930303ad3f12d3f2cefa6d02ddd0805` | 2 823 B |
| after the page saved | `b2f29bac95d2af116ced24d4e46050e838a98fb90963acf74ca29c2b819917df` | — |
| after run 2 (the guest writes its boot state) | `7b34fe54073136b40e4cbcefacb98e6cb4035e8f11e19f645ab7f2a309063465` | 2 917 B |

`bench-two` sits at `0x9e40` and `bench-password` at `0x9980`, both inside the NVS partition; the old
name is still at `0x9940` as NVS's previous entry until the page is rewritten. Build: `nvs-flash`
`ff4baa0` merged with `hostfwd` `05aec5d` (local `nvs-hostfwd-local`, not for pushing).

### Run A — the guest saves its settings through its own UI

```
esp32sim --chip s3 --boot rom --rom esp32s3_rev0_rom.elf --flash-mb 16 --psram-mb 8 --board none \
  --bootloader bootloader.bin --ptable ptable.bin \
  --app app.bin --flash-at 0xe000=boot_app0.bin \
  --flash-file flash-a.bin --wifi-join '' \
  --hostfwd tcp:127.0.0.1:18101-192.168.4.1:80 --max-seconds 40
```

Guest console, then the page's own protocol — `POST /_kfg_` with an empty body reads the current
configuration (`RequestParameters` in the page), a POST of the whole set writes it (`cmdSave`):

```
Switched to AP Mode ESP46:1B:F6:75:DC:E0
IP address: 192.168.4.1

GET  /config            200, 4223 B gzip (12833 B decoded)
POST /_kfg_ (empty)     200: VER=<firmware banner> &STANAME=&STAPASS=&APNAME=&...
POST /_kfg_ (save)      200: ...&STANAME=heishamon-bench&STAPASS=bench-password&...
POST /_kfg_ (read back) 200: ...&STANAME=heishamon-bench&STAPASS=bench-password&...
GET  /config?restart=yes    connection closed: the guest restarts
```

`SIGTERM`, as a run without `--max-seconds` would need:

```
[emu] signal: stopping to write the flash image
[emu] stop: interrupted by a signal — core0 72733199 + core1 51358352 insns in 13.2s wall = 9.4 Minsn/s;
      emulated 17.227s (4134526767 cycles); 259756 exceptions, 42485 interrupts
[emu] flash: wrote 16777216 bytes to flash-a.bin (--flash-file)
exit status 130
```

### The file

| property | value |
| --- | --- |
| size | 16 777 216 bytes, exactly `--flash-mb 16` |
| sha256 | `a25b02cdebe89103fda25c36b54e11d66930303ad3f12d3f2cefa6d02ddd0805` |
| NVS (`0x9000..0xe000`) | 2 823 bytes not `0xff` (a factory-fresh flash has 0) |
| `heishamon-bench` | offset `0x9940` — inside NVS |
| `bench-password` | offset `0x9980` — inside NVS |

### Run B — nothing but the file and an access point

```
esp32sim --chip s3 --boot rom --rom esp32s3_rev0_rom.elf --flash-mb 16 --psram-mb 8 --board none \
  --flash-file flash-a.bin --wifi ssid=heishamon-bench,chan=6,psk=bench-password \
  --hostfwd tcp:127.0.0.1:18102-:80 --max-seconds 60
```

No `--bootloader`, `--ptable`, `--app` and no `--flash-image`: the firmware is what the file holds. The
guest's console, its own configuration, and the emulator's accounting:

```
[emu] flash: 16777216 bytes from flash-a.bin (--flash-file)

Chip revision 0 Freq 240
STARTING
Connecting to heishamon-bench
.............
Connected to heishamon-bench
IP address: 10.0.2.15

POST /_kfg_ (empty) 200: ...&STANAME=heishamon-bench&STAPASS=bench-password&...
GET  /config        200, 4223 B gzip — the station-mode page
GET  /              200, 2870 B

[emu] wifi: 49 frames sent by the station, 135 received (1 dropped: no descriptor); AP: 67 beacons,
      22 probe responses, 22 data frames from the station, state Associated
[emu] net: 1 DHCP leases, 1 ARP replies, 0 DNS answers, ...
[emu] crypto: 36 AES blocks, 53936 SHA blocks, 0 RSA/MPI operations
[emu] hostfwd: 5 host connections (4 established, 1 refused by the guest, 0 refused for want of a slot,
      0 abandoned), 1 closed, 0 active, 4 in TIME_WAIT, 0 segments without a flow, 567 bytes in,
      8013 bytes out
```

The four-way handshake is the 36 AES blocks; the lease is the DHCP count; `state Associated` and the 22
data frames are the link. 6.9 emulated seconds, 14.8 s wall, 13.8 Minsn/s.

### Control — a flash that was never configured stays in access-point mode

The same images, the same `--wifi` access point on the air, the same forward, but `--flash-file
flash-fresh.bin` (which did not exist, so the run started from a factory-fresh flash and was seeded
from `--bootloader`/`--ptable`/`--app`):

```
Switched to AP Mode ESP46:1B:F6:75:DC:E0
IP address: 192.168.4.1

[emu] wifi: 153 frames sent by the station, 151 received (1 dropped: no descriptor); AP: 152 beacons,
      0 probe responses, 0 data frames from the station, state Idle
[emu] hostfwd: 0 host connections (0 established, ...), 0 bytes in, 0 bytes out
```

No probe request, no association, no lease: with no saved credentials the guest never looks at the
access point the emulator offers, so run B's station mode came out of NVS.

### A guest that writes nothing

`cli/tests/flash_file.rs::a_firmware_that_writes_nothing_leaves_the_file_byte_identical` boots the same
app image twice with the same `--flash-file` and compares SHA-256 before and after: equal. The same file
picks up the guest's writes when there are any — two run-B runs of the same image hash differently
(`308011de…` then `b9339568…`), because the guest writes its own boot state each time.

## Tests

```
cargo clippy --workspace --all-targets -- -D warnings                          # clean
ESP32SIM_ROM_DIR=roms cargo test --release --workspace -- --include-ignored --skip external_
                                                                               # 998 passed, 0 failed
cargo test -p esp32sim --test flash_file                                       # 8 passed
cargo test -p esp32sim --test soc_contracts                                    # 6 passed
cargo test -p esp-soc --lib flash                                              # 7 passed
```
Goldens were not regenerated: nothing on a run without `--flash-file` changes, including the report line
(`report` takes an `Option<Stop>` so an interrupted run can say so).

## Limits

- The file is the whole array: 16 MiB for a 16 MB chip, written in full at every exit. A save costs one
  write of that size; there is no per-region or incremental write.
- A file longer than the array is refused; it is never truncated, because truncating a real dump would
  silently drop the tail (otadata or a filesystem often sit there).
- The signal is polled between 16 M-instruction chunks: the write lands within about a second of the
  signal at 20 Minsn/s. `SIGKILL` cannot be handled, by design — the atomic rename is what protects the
  previous image there.
- `--flash-file` and `--flash-out` are independent of the join and of `--hostfwd`; the bench needed
  those two only to reach the guest's page.

## Reproduction

[nvs-flow.py](nvs-flow.py) is the whole bench: the emulator runs, the guest's page, its form protocol,
and the checks. It takes the guest images from the environment and the emulator paths from the command
line. `save` is the one-build loop (write a new name through the page, then boot with it); `run-a` and
`control` need `--wifi-join`, which is a different change.

```sh
APP=app.bin BOOTLOADER=bootloader.bin PTABLE=ptable.bin BOOT_APP0=boot_app0.bin \
ROM=$ESP32SIM_ROM_DIR/esp32s3_rev0_rom.elf \
python3 nvs-flow.py save --nvs-bin <emulator with --flash-file and --hostfwd> --flash-file flash.bin
NEW_SSID=bench-two python3 nvs-flow.py run-b --nvs-bin <the same build> --flash-file flash.bin  # SSID=SSID of the file
python3 nvs-flow.py run-a --join-bin <emulator with --wifi-join> --flash-file flash-a.bin
python3 nvs-flow.py run-b --nvs-bin <this build> --flash-file flash-a.bin
python3 nvs-flow.py control --nvs-bin <this build> --flash-file flash-fresh.bin
```
