# EX213: C3 scan and connection events

Base: upstream/main `017af524`, including merged PR #195. This extends EX211's
controller-driven advertising with SCAN_REQ/SCAN_RSP, format-3 connection events,
CSA#1, SN/NESN, guest supervision timeout and a virtual central that replies to
LL_VERSION_IND and reads a characteristic by UUID. It reuses the merged enable,
exchange-memory mapping, FIFO, observer, script dispatch and hex/AD helpers.

## Contract and limits

The guest controller owns RX descriptors, TX descriptors, event entries and
interrupt handling. Each event produces one END. A TX descriptor is released
only after ACK; retransmissions retain the packet. The central's fixed anchors
must lie inside the guest receive window. It does not move its clock to rescue a
missed window. Host scan/connect/read settings are grouped and survive controller initialization
and machine reboot. Scan requests and central data packets retain the transmitted
bytes between phases; peripheral TX reuses its cached payload without cloning.
The ROM loader backfills the separate Bluetooth initializer ROM source from the
ELF's `.data_btdm`, allowing the guest to restart advertising without losing UUIDs.

One unencrypted 1M CSA#1 connection is supported, with the central selecting a
30 ms interval, 2 s supervision timeout, hop 5 and all 37 data channels. ATT uses
MTU 23. No pairing, encryption, CSA#2, PHY update or L2CAP fragmentation claim.
Unsupported guest procedures report an error. Air times, receive windows and
IRQ/ownership transition timing are model choices or inferred from the C3 rev3
ROM and IDF 5.5.5 controller, not measured RF behavior.

New LC/EM fields are inferred unless a code comment identifies a probe-b field
check. There is no public Espressif LC descriptor header cited as authority for
these fields. ROM function names and offsets are beside each field access.
The probe's public configuration register names were checked in the installed
Arduino-ESP32 3.3.11 headers: IDF v5.5.5
`components/soc/esp32c3/register/soc/syscon_reg.h` lines 144/152 define
`SYSCON_WIFI_CLK_EN_REG`/`SYSCON_WIFI_RST_EN_REG`, and
`components/soc/esp32c3/register/soc/system_reg.h` lines 564/572 define
`SYSTEM_BT_LPCK_DIV_INT_REG`/`SYSTEM_BT_LPCK_DIV_FRAC_REG`.
The fixture uses IDF v5.5.5; the historical Arduino-ESP32 3.3.11 probe also uses
IDF 5.5.5. No IDF 4.4 controller compatibility is claimed.

Connect, UUID read and central silence use the existing script/command channel;
there are no separate connect/read/relative-stop CLI flags or WASM stop export.
Absolute script times cover the supervision-timeout workload. For example,
`--ble full --script connection.txt` with:

```text
0.5 ble connect
0.7 ble read-uuid 4fafc201-1fb5-459e-8fcc-c5c9c331914b beb5483e-36e1-4688-b7f5-ea07361b26a8
3.1 ble central-stop
```

## Inputs and reproduction

[inputs.json](inputs.json) pins the committed server source/binary, reused
bootloader/partition table and C3 rev3 ROM. The source and pinned toolchain recipe
are in [examples/c3-ble-server](../../../examples/c3-ble-server/README.md).
Two-directory reproducibility is a retained fixture-build result; this port does
not claim a fresh firmware rebuild. NOTICE names the linked libraries. The retained server ELF's SHA-256 appears
at byte 176 of the committed app. `riscv32-esp-elf-nm --defined-only "$SERVER_ELF"`
finds no defined `mbedtls` or `psa_crypto` symbols. [Symbol counts and ELF hash](linked-symbols.json)
record that check; the ELF is not required by CI and is not committed.

```sh
mkdir -p .bleb-scratch/tmp .bleb-scratch/empty-home
TMPDIR="$PWD/.bleb-scratch/tmp" tools/fetch-demo-assets.sh --no-linux
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" TMPDIR="$PWD/.bleb-scratch/tmp" \
  cargo +1.99.0 test --release -p esp32sim --test ble \
  full_ble_server_reconnects_in_ci -- --ignored
python3 docs/evidence/ble-c3-connection/mutate.py
python3 docs/evidence/ble-c3-connection/hardware/check_compare.py
```

The native fixture covers empty-first timeout, read-first timeout, and normal
LL termination, followed by a second connection and UUID read. It requires
unchanged advertising and scan-response payloads, exact console output and
interrupt totals/per-source counts. Six new `ble-server-c3-*` golden files pin
those outputs; existing goldens are unchanged. Accounted-cycle counts are not
used as evidence of connection correctness.

[mutations.json](mutations.json) records each mutation and the test that failed.
The runner restores the source after every mutation and rejects compilation
failures as mutation kills. The hardware comparator has separate malformed-input
and static-field mutation checks.

## Hardware comparison

[probe-b](hardware/README.md) retains the historical unchanged-image comparison,
its source sketch, input/capture hashes, comparator and compact normal/timeout
summaries. Hardware and emulator both connected/read twice and resumed advertising.
Static format/activity/channel-map and RX-ring fields agree under the stated
masks; PHY/CSA/hop differ by central configuration. CS+24/+86 live status remains
unmodeled. No hardware was accessed for this port. The current emulator was also run against
the same hashed ProbeB images and compared with the retained hardware captures;
`hardware/current-*.json` records those results.

The summaries omit device addresses, packet bytes and serial text. Raw hardware
capture hashes identify the original inputs but the captures are not public.
Independent recovery of omitted fields is impossible from these summaries.
No device identifiers or private repository references are needed to rerun the
committed synthetic checks or CI firmware workload.

## Verification

[checks.json](checks.json) records commands and results on Rust 1.99.0, with
[source hashes](source-hashes.json) identifying the checked implementation.
Both native and wasm Clippy pass with warnings denied. The workspace passes
with an empty HOME and only ROM input configured: 666 passed under CI policy.
Plain workspace tests pass with no ESP32SIM inputs: 642 passed, 42 ignored.
The eight required production WASM demos, advertising ABI and connection ABI
pass; [wasm.json](wasm.json) pins the module hash and reconnect results.
32 deliberate mutations fail their named tests. Privacy and source diff whitespace checks pass. The exact console goldens
retain guest CRLF and trailing spaces; those intentionally trigger Git whitespace
checks and are excluded from the source-only check. JIT implementation code is unchanged, so no JIT differential run was needed.

```sh
cargo +1.99.0 clippy --workspace --all-targets -- -D warnings
cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
cargo +1.99.0 test --release --workspace
RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh
node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel
node wasm/tests/ble-api.mjs web/wasm/esp32sim.wasm web/wasm/fw/public web/wasm/fw/esp32c3_rev3_rom.elf
node wasm/tests/ble-connection.mjs web/wasm/esp32sim.wasm web/wasm/fw/public web/wasm/fw/esp32c3_rev3_rom.elf
node tools/check-evidence-privacy.mjs
```

For the empty-HOME runs, preserve the toolchain locations in `CARGO_HOME` and
`RUSTUP_HOME`, set HOME to `.bleb-scratch/empty-home`, and TMPDIR to
`.bleb-scratch/tmp`. No firmware paths come from that HOME. External Arduino
server tests remain ignored and named `external_*`; their inputs are not CI fixtures.

## CPU comparison

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 954f2a68 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured on `771b56f91a208353f8f768e06a32b441406611b9` against main `954f2a68`; the branch was later rebased onto `954f2a68` with no code change. User CPU seconds.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C3 hello | 2.5622 (2.5402–2.5785) | 2.5497 (2.5046–2.5990) | -0.49% | 3/7 | 4800000000 | 4.91 |
| S3 hello | 26.6897 (25.3917–29.0711) | 26.5465 (25.7646–27.3782) | -0.54% | 3/7 | 1789819657 | 6.07 |

### C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.522524 | 2.556374 | 4.91/4.60/4.91 | 4.91/4.91/4.91 | accepted |
| 1 | 2 | M→B | 2.548344 | 2.549680 | 4.60/4.60/4.60 | 4.60/4.55/4.60 | accepted |
| 2 | 3 | B→M | 2.546778 | 2.559879 | 4.55/4.27/4.55 | 4.55/4.55/4.55 | accepted |
| 3 | 4 | M→B | 2.569238 | 2.550684 | 4.27/4.27/4.27 | 4.27/4.33/4.33 | accepted |
| 4 | 5 | B→M | 2.568473 | 2.598981 | 4.33/4.30/4.33 | 4.33/4.33/4.33 | accepted |
| 5 | 6 | M→B | 2.540249 | 2.536239 | 4.30/4.30/4.30 | 4.30/4.03/4.30 | accepted |
| 6 | 7 | B→M | 2.562198 | 2.505018 | 4.03/4.19/4.19 | 4.03/4.03/4.03 | accepted |
| 7 | 8 | M→B | 2.578463 | 2.504644 | 4.19/4.19/4.19 | 4.19/4.10/4.19 | accepted |

### S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 25.700724 | 26.046650 | 5.00/7.66/7.90 | 4.10/5.00/5.08 | discarded: load >7 |
| warmup | 2 | B→M | 26.149283 | 26.069809 | 4.40/4.04/4.40 | 4.96/4.40/4.96 | accepted |
| 1 | 3 | M→B | 25.391733 | 25.764575 | 4.04/3.77/4.12 | 3.77/4.35/4.77 | accepted |
| 2 | 4 | B→M | 25.639041 | 26.011693 | 7.57/11.20/11.20 | 4.35/7.57/7.57 | discarded: load >7 |
| 2 | 5 | B→M | 26.689726 | 26.546507 | 4.55/5.90/6.07 | 4.97/4.55/4.98 | accepted |
| 3 | 6 | M→B | 26.513916 | 26.691071 | 4.81/4.24/4.83 | 4.24/4.16/4.27 | accepted |
| 4 | 7 | B→M | 27.670317 | 26.370161 | 4.91/5.00/5.17 | 4.16/4.91/4.91 | accepted |
| 5 | 8 | M→B | 29.071090 | 26.857799 | 4.69/5.48/5.48 | 5.48/5.38/5.54 | accepted |
| 6 | 9 | B→M | 26.688934 | 27.378237 | 5.32/5.53/5.78 | 4.94/5.32/5.74 | accepted |
| 7 | 10 | M→B | 26.962300 | 26.327239 | 4.60/4.76/5.08 | 4.76/5.34/5.66 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above. Raw output files are preserved.

All added radio state is inside the existing optional
boxed BLE state. UUID discovery is full-controller-only, so the existing HCI
Peer/Session fields and sizes are unchanged; no fields were added to the bus/peripheral hot structures. The
disabled tick path, optional-device registration and instruction dispatch are
unchanged. Observation formatting stays behind the merged lazy observer gate.

