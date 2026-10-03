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
`http://127.0.0.1:18765/`. The virtual gateway `10.0.2.2` is not an alias for
host loopback. Set `ROM_DIR` to the Espressif ROM ELF directory. Firmware uses
Arduino-ESP32 3.3.8 with the pinned PlatformIO platform in `platformio.ini`.

From the repository root, create a temporary sketch project and host server:

```sh
export NET_URL='http://127.0.0.1:18765/'
export ROM_DIR='/path/to/esp-rom-elfs'
P=/tmp/esp32sim-net-reproduce
mkdir -p "$P/src" "$P/http"
cp docs/evidence/ethernet-c3-2026-10-02/platformio.ini "$P/"
cp docs/evidence/ethernet-c3-2026-10-02/main.cpp "$P/src/"
printf 'upstream-net-ok\n' > "$P/http/index.html"
python3 -m http.server 18765 --bind 127.0.0.1 --directory "$P/http"
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

## PR #170 review follow-up

Baseline `6feac19acfb2128e50ae1fb1a2f56eb1df4131ef`; corrected source
`df0f5c118c6f33ba2eca25c12d88397a58e0ce51`. This extends EX202 with the
requested firmware golden, interrupt-bit and relay backpressure contracts, and
CPU measurements. FE_IQ now evaluates completion on register access at the same
80th APB edge, including odd CPU start cycles. It has no tick or deadline hook.
The EX047 scheduling quantum is unchanged.

The C3 GDMA `Device` adapter goes through MMIO logging and observation. Independent
one-hot interrupt tests cover raw, enable, status, clear and matrix routing on
all three channels. Swapping IN DONE/SUC_EOF bits makes the test fail. The relay
test keeps its queue full across ticks, checks descriptor recycling and airtime,
and fails when each chip's relay path drains all queued frames. S3 reboot keeps
the external AP and network, including NAT, while resetting MAC queues. Tests
check that fix and count oversize/full-queue TX drops on all three chips.

`review-checks.json` records source/firmware hashes and results. Both required
Clippy commands passed with Rust 1.99.0 and warnings denied. The release workspace
suite passed 480 tests, zero failed/ignored, with 14 external tests filtered.
All eight requested WASM manifests passed. Existing goldens are unchanged; only
`wifi-station-c3.station.txt` was added. No JIT code changed.

The C3 station input reuses `examples/c6-wifi-station`, with the LCD disabled,
ESP-IDF 5.5.4, and the default synthetic WPA2 network. It scans, joins, receives
`10.0.2.15`, and receives five of five gateway ping replies. The new golden was
created once and then checked without `UPDATE_GOLDENS`. The IDF build recipe is
in that example's README. This run used installed PlatformIO packages:

```sh
P=/tmp/pr170-station
mkdir -p "$P"
cp -R examples/c6-wifi-station/main "$P/"
cp examples/c6-wifi-station/CMakeLists.txt "$P/"
cat > "$P/sdkconfig.defaults" <<'CONFIG'
CONFIG_IDF_TARGET="esp32c3"
CONFIG_ESPTOOLPY_FLASHSIZE_4MB=y
CONFIG_PARTITION_TABLE_SINGLE_APP_LARGE=y
CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y
CONFIG_ESP_WIFI_SOFTAP_SUPPORT=n
CONFIG_STATION_LCD=n
CONFIG
cat > "$P/platformio.ini" <<'CONFIG'
[env:c3]
platform = espressif32
board = esp32-c3-devkitm-1
framework = espidf
board_build.sdkconfig_defaults = sdkconfig.defaults
[platformio]
src_dir = main
CONFIG
UV_CACHE_DIR=/tmp/pr170-uv pio run -d "$P"
B=/tmp/pr170-station-input
mkdir -p "$B/bootloader" "$B/partition_table"
cp "$P/.pio/build/c3/bootloader.bin" "$B/bootloader/bootloader.bin"
cp "$P/.pio/build/c3/partitions.bin" "$B/partition_table/partition-table.bin"
cp "$P/.pio/build/c3/firmware.bin" "$B/c6_wifi_station.bin"
cp "$P/.pio/build/c3/firmware.elf" "$B/c6_wifi_station.elf"
C3_WIFI_STATION_BUILD="$B" ESP32SIM_ROM_DIR="$ROM_DIR" \
  cargo +1.99.0 test --release -p esp32sim --test goldens external_wifi_station_c3 -- --exact
```

The installed platform version was `55.3.39+sha.cbc3349`; using another installed
`espressif32` can change the build. The receipt pins the framework/toolchain
versions and binary hashes. The first build failed on the sandbox's UV cache
permissions, then on ESP-IDF's sysctl access; the authorized build succeeded.
The first focused-test compile lacked the `Bus` trait import; adding the import
fixed it. These failures are not counted as passing checks.

For CPU measurements, build each source with `cargo +1.99.0 build --release -p
esp32sim`, preserve its `esp32sim-c3` executable, then run from the repository root:

```sh
python3 docs/evidence/ethernet-c3-2026-10-02/bench-c3.py \
  /path/to/before /path/to/after "$ROM_DIR/esp32c3_rev3_rom.elf" > /tmp/c3-timing.json
```

The script runs c3-hello for 30 emulated seconds, three times per executable in
alternating order. CPU time is child user plus system time. Every run must report
4,800,000,000 accounted instructions, and all console hashes must match. Counts
include idle accounting; they are not retired busy instructions. No benchmarks
from this task overlap this final batch. Other machine activity is uncontrolled;
aggregate load is retained without process names or host identities.

Final alternating CPU seconds, before/after by pair: 2.622587/2.660453,
2.648567/2.679748, 2.651733/2.667746. Medians are 2.648567/2.667746,
**0.72% more CPU time** after the review changes. Each run has 4,800,000,000
accounted instructions and the same console SHA-256. FE_IQ's scheduling work is
removed, but this whole-change comparison does not establish a speedup or isolate
that change from the GDMA wrapper/code layout. One-minute system load was
10.56–11.29; this is one in-use machine, with no confidence interval.

The earlier non-alternating batch is retained in `review-initial-timing.json`:
before 2.560053, 2.469549, 2.578199 CPU seconds; initial candidate 3.534965,
3.142974, 2.754620. Its strong drift motivated the alternating follow-up rather
than deleting the negative result. All six also reported 4,800,000,000 instructions.
The initial candidate preceded the odd-cycle APB alignment correction; binary
hashes distinguish it from the final source. Raw logs remain outside Git in
`/tmp`; retained receipts contain no personal paths, process inventories or host
identities. No original historical artifact hashes were replaced.

## Scoped idle follow-up

EX202 measures this PR's unused-feature overhead. FE_IQ remains unclocked and has
no deadline. Pending DMA/TX/AP work is cached after MMIO, completion and host AP
configuration. The existing SPI-work branch also gates feature work. WiFi source
0 is cached at those event boundaries instead of polling the MAC during every
interrupt scan. The C3 bus still returns 1 from every tick, as main does; SPI runs
before device clock advancement, as before this PR.

`idle_rounds_skip_feature_work` checks idle state, TX activation/completion and
WiFi IRQ acknowledgement. `configured_ap_keeps_work_scheduled_across_reboot`
checks the first AP beacon after reset. Both Clippy commands, 482 release workspace
tests (zero ignored, 14 external tests filtered), the eight requested WASM
manifests and the external C3 WPA2/DHCP/five-ping golden pass. Existing goldens are
bit-identical. No JIT implementation changed.

The supplied regression (main 2.430 s, PR #170 2.481 s) remains in
`idle-supplied.json`; that receipt did not identify the exact PR revision.
`scoped-initial.json` retains the incomplete first scoped attempt: it exceeded
load 3 after a run and was stopped. Its candidate did not yet cache the WiFi IRQ.

The timing recipe adapts the supplied `/tmp/bench-quiet.py`: identical firmware
arguments, child user+system CPU time, one excluded warmup per arm and seven
interleaved rounds with alternating arm order. `quiet-bench.py` gates each run on
`uptime` one-minute load below 2.5 (headroom below the required limit of 3) and rejects the batch if a post-run load reaches
3. Build each revision in its own source and target directory:

```sh
CARGO_TARGET_DIR=/tmp/main-target cargo +1.99.0 build --release -p esp32sim --manifest-path /path/to/main/Cargo.toml
CARGO_TARGET_DIR=/tmp/published-target cargo +1.99.0 build --release -p esp32sim --manifest-path /path/to/published/Cargo.toml
CARGO_TARGET_DIR=/tmp/after-target cargo +1.99.0 build --release -p esp32sim --manifest-path /path/to/after/Cargo.toml
python3 docs/evidence/ethernet-c3-2026-10-02/quiet-bench.py /path/to/firmware \
  published=/tmp/published-target/release/esp32sim \
  main=/tmp/main-target/release/esp32sim after=/tmp/after-target/release/esp32sim
```

The assembly comparison found two costs introduced by the new feature path:
inlining its large routine added stack saves to idle ticks, and the new fields
at the front of `Peripherals` displaced existing fields. The final source keeps
feature work out of line, shares one device-tick call and places new devices
after the existing peripherals, with the MAC state boxed. Unclocked DMA/MAC work uses the already advanced
bus cycle count; SPI still runs before clocked devices advance. Main's scheduler
and interrupt rescan behavior are unchanged.

The other scoped candidates are retained: `scoped-excluded.json` (source
`00a7502`) crossed load 3; `scoped-noinline.json` (`e65fba5`) completed below load 3
but measured +0.90% against main; `scoped-inline.json` records the ineffective
wrapper inline annotation and crossed load 3; `scoped-single-tick.json` measured
+1.08% below load 3; `scoped-layout.json` records the reordered fields but crossed
load 3. Uncommitted variants include their exact patches. These are not accepted
measurements of the final source. `scoped-bool.json` retains the next quiet
candidate (`e79933d`): +0.84%, above the limit, before boxing the MAC state.

Seven interleaved c3-hello runs per arm after warmup, 30 emulated seconds each: published `6feac19` median 2.552817 s CPU (2.469527–2.601530), main `ed34b22` 2.536643 s (2.388238–2.578642), after `143114f` 2.547884 s (2.385592–2.588366). After is +0.44% versus main with overlapping ranges, within the requested +0.5% limit. Each arm used its own Cargo target directory. Every measured start had one-minute load 1.98–2.25; every recorded finish was below 2.25. All 21 runs match the console hash, 4,800,000,000 instructions/cycles, zero exceptions and 3,028 interrupts.

`scoped-final.json` retains the accepted samples, warmups, revisions, firmware and
binary hashes, command and conditions. Its diagnostic patch records 75,011,991
ticks, 75,256,512 scans (the unchanged main behavior), and zero feature-work calls.
Apply that patch to a disposable export and build separately to reproduce counts;
the instrumented executable is never used for CPU timing.

Receipts retain revisions, commands, firmware and binary hashes, numeric samples,
aggregate load and anonymous OS/architecture details. No process inventory or
personal paths are retained. Measurements concern native CPU time, not RF or
browser performance. Concurrent jobs can still affect timing below the load gate.
