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

PENDING

No CPU benchmarks were run. All added radio state is inside the existing optional
boxed BLE state. UUID discovery is full-controller-only, so the existing HCI
Peer/Session fields and sizes are unchanged; no fields were added to the bus/peripheral hot structures. The
disabled tick path, optional-device registration and instruction dispatch are
unchanged. Observation formatting stays behind the merged lazy observer gate.
