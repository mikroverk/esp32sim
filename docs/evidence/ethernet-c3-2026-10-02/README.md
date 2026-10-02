# Ethernet relay and C3 station validation

EX202 checks a new Ethernet transport contract, not execution speed. The source
is `b4acf2366a602dcd78f6b5738756a2c1fd1b5cea`, based on upstream
`dddb128052dca15250e2169b92ab73c4d87f524c`. The behavior reference was fork
`221080ffccfa829106b398f896653535853c76c8`, inspected with `git show` and
`git diff` before implementation. No dependency on that fork or classic ESP32
PR #168 was added. EX047's scheduler contract is unchanged.

## Results

`cargo build --release`, `cargo test --workspace`, `tools/wasm-build.sh` and
`git diff --check` passed. Workspace totals are 466 passed, 0 failed, 22 existing
ignored tests. The ignored tests require external assets or tools; no new test
was ignored. The privacy check also passed after staging this evidence.

The new tests exercise C3 calibration and AES DMA registers, three-chip guest DMA
DHCP relay, default DHCP with NAT attached on S3/C6, queue bounds, mode switching,
reset persistence and the WASM input boundary. Native HTTP runs use the existing
NAT with no transport flag, so the default path is also checked end to end.

| Firmware/run | Serial output and packet checks | Result |
| --- | --- | --- |
| C3 open, native, 30 guest seconds | `status=3 ip=10.0.2.15`, HTTP 200, body `upstream-net-ok` | Pass |
| C3 WPA2-PSK, native, 30 guest seconds | Same IP, status, HTTP code and body | Pass |
| S3 open, native, 10 guest seconds | Same IP, status, HTTP code and body | Pass |
| C6 open, native, 10 guest seconds | Same IP, status, HTTP code and body; `bb_init=0` | Pass |
| S3 WASM, host DHCP | One DISCOVER and one REQUEST drained; two replies injected; guest reports IP; 423,201,536 cycles / 23,237,653 instructions | Pass |
| C3 WASM, host DHCP | Same packet and IP checks; 284,481,788 cycles / instructions | Pass |
| C6 WASM, host DHCP | Same packet and IP checks; 284,001,084 cycles / instructions; `bb_init=0` | Pass |

The WASM runs use the normal production JIT host and stop after the connected/IP
serial line. Their host responder implements only DHCP. The sketch's later HTTP
request is outside those runs. Native HTTP uses a local Python server, not a
public service. CLI instruction totals include idle accounting and are not a
count of busy instructions. Wall times in the retained native summaries are
single correctness-run observations, with no controlled load or speed claim.

## Reproduce

Provide `NET_URL` as a URL reachable from host sockets, for example
`http://<host-address>:18765/`. The virtual gateway `10.0.2.2` is not an alias for
host loopback. Set `ROM_DIR` to the Espressif ROM ELF directory. Firmware uses
Arduino-ESP32 3.3.8 with the pinned PlatformIO platform in `platformio.ini`.

From the repository root, create a temporary sketch project and host server:

```sh
export NET_URL='http://<host-address>:18765/'
export ROM_DIR='/path/to/esp-rom-elfs'
P=/tmp/esp32sim-net-reproduce
mkdir -p "$P/src" "$P/http"
cp docs/evidence/ethernet-c3-2026-10-02/platformio.ini "$P/"
cp docs/evidence/ethernet-c3-2026-10-02/main.cpp "$P/src/"
printf 'upstream-net-ok\n' > "$P/http/index.html"
python3 -m http.server 18765 --bind 0.0.0.0 --directory "$P/http"
```

In another terminal with the same variables, build all environments together.
The original native/WASM runs used the same sketch, with only `NET_PSK` changing
between open and WPA2. S3 and C6 board defaults require 8 MiB flash; C3 uses 4 MiB.

```sh
pio run -d "$P" -e c3 -e wpa2 -e s3 -e c6
cargo build --release
cargo test --workspace
tools/wasm-build.sh
B="$P/.pio/build"
target/release/esp32sim-c3 --rom "$ROM_DIR/esp32c3_rev3_rom.elf" \
  --flash-image "$B/c3/firmware.factory.bin" --elf "$B/c3/firmware.elf" \
  --wifi ssid=esp32sim --max-seconds 30 --no-reboot
target/release/esp32sim-c3 --rom "$ROM_DIR/esp32c3_rev3_rom.elf" \
  --flash-image "$B/wpa2/firmware.factory.bin" --elf "$B/wpa2/firmware.elf" \
  --wifi ssid=esp32sim,psk=esp32sim-pass --max-seconds 30 --no-reboot
target/release/esp32sim --board none --boot rom --flash-mb 8 \
  --rom "$ROM_DIR/esp32s3_rev0_rom.elf" --flash-image "$B/s3/firmware.factory.bin" \
  --elf "$B/s3/firmware.elf" --wifi ssid=esp32sim --max-seconds 10 --no-reboot
target/release/esp32sim-c6 --flash-mb 8 --rom "$ROM_DIR/esp32c6_rev0_rom.elf" \
  --flash-image "$B/c6/firmware.factory.bin" --elf "$B/c6/firmware.elf" \
  --wifi ssid=esp32sim --stub bb_init=0 --max-seconds 10 --no-reboot
node docs/evidence/ethernet-c3-2026-10-02/relay.mjs "$PWD" s3 \
  "$ROM_DIR/esp32s3_rev0_rom.elf" "$B/s3/firmware.factory.bin" "$B/s3/firmware.elf"
node docs/evidence/ethernet-c3-2026-10-02/relay.mjs "$PWD" c3 \
  "$ROM_DIR/esp32c3_rev3_rom.elf" "$B/c3/firmware.factory.bin" "$B/c3/firmware.elf"
node docs/evidence/ethernet-c3-2026-10-02/relay.mjs "$PWD" c6 \
  "$ROM_DIR/esp32c6_rev0_rom.elf" "$B/c6/firmware.factory.bin" "$B/c6/firmware.elf" bb_init=0
node tools/check-evidence-privacy.mjs
git diff --check
```

Stop the HTTP server and delete the temporary `.pio` build directory after
recording results. The original images and ELFs were preserved outside Git in
`/tmp/up-net-firmware/{open,wpa2,s3,c6}` before deleting the build directories.
Their hashes and the ROM/native/WASM artifact hashes are in `artifacts.json`.
Changing the caller's URL or build path can change firmware/ELF hashes.

## Negative results and limits

- The first C3 open HTTP attempt returned `NET http=-1 body=` because the sketch
  targeted `http://10.0.2.2:18765/`. The emulator does not map that address to host
  loopback. Rebuilding with the actual host address returned HTTP 200.
- Before AES DMA was wired, C3 WPA2 stalled at PC `0x420add4c`,
  `aes_hal_wait_done+0xc`. The 30-second run retired 4,800,000,062 accounted
  instructions with no IP report. A 3-second diagnostic stopped at the same PC.
  C3 GDMA address/interrupt translation and the AES DMA transfer fixed it.
- The first S3 and C6 runs used 4 MiB flash. Both reported
  `Detected size(4096k) smaller than the size in the binary image header(8192k). Probe failed.`
  They reset before the sketch. S3 stopped after 6,608,711 cycles; C6 after
  8,264,325. The first WASM harness allowed resets to restart its cycle limit;
  those two runs were interrupted without a verdict. The harness now uses a
  bounded number of slices and rejects resets. Using 8 MiB fixed both runs.
- C6 still prints `error: pll_cal exceeds 2ms!!!` and needs the existing
  `bb_init=0` stub. No C6 calibration change is claimed.
- PlatformIO initially raised `PermissionError` while accessing its package
  cache; an authorized build outside the filesystem sandbox succeeded.
- An initial `cargo check -p esp32sim-cli` failed with
  `package ID specification esp32sim-cli did not match any packages`.
  `cargo check --workspace` passed; the package is named `esp32sim`.
- PlatformIO removed earlier environment build folders after its configuration
  changed. A final C3 rerun failed with `No such file or directory`. The open
  image was already preserved; rebuilding and preserving WPA2 restored the
  final checks. No missing-file attempt was counted as firmware validation.

No real C3/S3/C6 radio capture, RF model, WPA3, roaming, TLS or throughput test was
performed. C3 shares DHCP/DNS/SNTP/NAT implementations, but the new C3 firmware
specimen checks DHCP and HTTP, not DNS/SNTP separately. Existing protocol tests
remain green. Raw frames are limited to 1518 bytes and queues to 64 frames;
transport changes are intended before guest connections, not as TCP migration.

## Evidence curation

Serial CRLF was normalized to LF after the staged whitespace check rejected it.
The retained serial files omit local ROM paths, NAT resolver addresses and
register/interrupt dumps. Emulator-generated station identifiers are synthetic.
The build recipe accepts the HTTP target from the caller and contains no host
address. `artifacts.json` records original log hashes and curated hashes; measured
values and pass/fail checks were preserved. These omissions prevent reconstruction
of the local network configuration, but do not change the packet or HTTP result.
No binary, private capture, process inventory or application inventory was
committed. All evidence files were manually reviewed and are below 50 KB.
