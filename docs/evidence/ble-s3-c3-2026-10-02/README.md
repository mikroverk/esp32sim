# Virtual BLE on S3 and C3

The unchanged Arduino-ESP32 3.3.8 Server, Notify, Write and Scan examples pass on
S3 and C3. C6 is unfinished and rejects `--ble`. This change starts from upstream
`dddb128`; it does not require classic ESP32 support or the Schematik fork.

Implementation commit: `b8c5295535348f2fbafdde9e7389052994af5caa`.

The shared H4/HCI/ACL/ATT controller comes from local prototype `0755b8b`, whose
classic-chip evidence is at `a79fa7a`, experiment EX199. The classic adapter was
inspected but not imported. EX200 changes the host stack to NimBLE, adds LX7 and
RV32 calling glue, and tests callback scheduling on a single-core guest. The
fork at `221080f` was consulted as context, not used as an upstream dependency.

## Design and contract

`esp-soc::ble::Controller` exchanges H4 packets and supplies one virtual central
and one virtual Battery Service peripheral. Shared VHCI glue resolves controller
lifecycle, memory-release, power and packet functions from the application ELF.
The adapters provide memory bounds, callback validation and guest trampolines.
No Bluetooth MMIO block, product ABI or device catalogue was added.

At controller initialization, a checked portion of that function becomes a guest
trampoline. It calls `xTaskCreatePinnedToCore` and creates a FreeRTOS task that
polls the controller, invokes the guest callbacks, and calls `vTaskDelay`.
Callbacks execute normally and can schedule other tasks. Original flash bytes
are saved by physical offset and restored before reboot. Packet storage uses
512 bytes of otherwise unused controller BSS, preserved by memory-release hooks.
C3 function entries may be only two-byte aligned; the trampoline literals use
an aligned address inside the checked function extent.

The task yields one tick after each receive callback. Without this, C3's
high-priority callback task delivers reports before the host finishes enabling
its scan; the unchanged example returns zero devices. The initial failure is
retained in `negatives.json`. `scan-no-yield.patch` reconstructs this scheduling
condition against the final source. It is a reproduction aid, not a byte-identical
copy of the earlier uncommitted source. Apply it with `git apply --unidiff-zero` in an isolated checkout. The regression tests require a yield
between packet reception and the next send-ready callback.

The supported contract is one unencrypted LE connection, legacy advertising,
ATT MTU 23, discovery/read/write and notifications/indications. Writes are at most
20 bytes. No pairing, RF, connection-event timing or encryption is claimed.
Unsupported HCI commands return Unknown Command. NimBLE queries `202d`, `fc01`
and, after connecting, `2022`; these remain unsupported and did not prevent the
validated examples. Scan produces one immediate advertisement and, for active
scan, one response. Event masks and intervals do not simulate radio scheduling.
The task stays allocated until reboot. Matching, unstripped ELF symbols are required.

## Reproduction

Each temporary project copies the installed official `.ino` byte-for-byte to
`src/main.cpp`. `inputs.json` pins the source, configuration and package versions.
No sketch, sdkconfig or linked firmware was edited. The emulator substitutes
controller execution at runtime.

```ini
[env:esp32-s3-devkitc-1]
platform = https://github.com/pioarduino/platform-espressif32.git#55.03.38-1
board = esp32-s3-devkitc-1
framework = arduino
monitor_speed = 115200
```

Use `esp32-c3-devkitm-1` for C3 and `esp32-c6-devkitc-1` for C6. Build each with
`pio run -d /tmp/up-ble-firmware/CHIP/EXAMPLE`. The examples are Server, Notify,
Write and Scan. C6 Server was built only for transport inspection and the rejection
check. PlatformIO build directories were removed after recording their hashes.

```sh
python3 docs/evidence/ble-s3-c3-2026-10-02/run.py \
  --emulator target/release/esp32sim \
  --firmware /tmp/up-ble-firmware \
  --roms "$HOME/.platformio/packages/tool-esp-rom-elfs" \
  --output target/ble
```

The runner records full commands, scripts, ROM/ELF/factory/sketch hashes, exact
checks, stop summaries and timings. Logs stay under ignored `target/ble`.
It uses the discovered handle `0x0010` for these pinned examples and `0x0011`
for Notify's CCCD. Other firmware must use handles from its own discovery.

## Arduino results

Both chips advertise `BLE Server Example` and the custom service
`4fafc201-1fb5-459e-8fcc-c5c9c331914b`, connect, discover that service and read
`Hello World says Neil`. The unchanged Write example advertises `MyESP32` without
a service UUID in the advertising payload; discovery finds its service.

Relevant guest serial output on both chips:

```text
Starting BLE work!
Characteristic defined! Now you can read it in your phone!
Waiting a client connection to notify...
New value: Hello from host
Advertised Device: Name: esp32sim, Address: 02:53:49:00:00:02, serviceUUID: 0000180f-0000-1000-8000-00805f9b34fb, serviceUUID: 0000180f-0000-1000-8000-00805f9b34fb, rssi: -35
Devices found: 1
Scan done!
```

The address is a synthetic fixture, not a real device identifier. Host checks also require written-value readback and four
incrementing notification payloads `01000000` through `04000000`.

| Example | S3 modeled boot to advertising | S3 wall to advertising | C3 modeled boot to advertising | C3 wall to advertising |
| --- | ---: | ---: | ---: | ---: |
| Server | 0.258738 s | 0.389741 s | 0.267993 s | 0.086016 s |
| Notify | 0.256810 s | 0.087219 s | 0.266153 s | 0.086498 s |
| Write | 0.258744 s | 0.093490 s | 0.268008 s | 0.086857 s |

These are single sequential native runs on macOS 26.6.2 arm64, Rust 1.96.0,
with default JIT. Other host load was uncontrolled, including validation work.
Wall time starts at process launch and ends when Python reads the advertising
line; it includes image/ELF loading and scheduling. Cycle time uses the emulator's
CPU clock, not calibrated Bluetooth timing or the optional cost model. No speedup
or cross-chip throughput claim follows from these samples.

Software reset at 0.6 seconds, using `0.6 poke 60008000 80000000`, restores the
runtime substitution and boots through ROM again. S3 advertises at 0.258738 and
0.854738 seconds; C3 at 0.267993 and 0.863994 seconds. `reboot.json` records the
commands and checks. The second boot's startup timing differs from power-on.

## C6 remaining work

The C6 configuration selects NimBLE without legacy VHCI. The unchanged Server ELF
contains `ble_transport_to_ll_cmd_impl` and `ble_transport_to_ll_acl_impl`, which
tail-call `hci_transport_host_cmd_tx` and `hci_transport_host_acl_tx`.
`ble_transport_ll_init` registers `ble_transport_host_recv_cb`. That callback
routes type 2 to `ble_transport_to_hs_acl_impl` and other types to
`ble_transport_to_hs_evt_impl`.

Command allocation calls `r_ble_hci_trans_buf_alloc(3)`; transport free calls
`r_ble_hci_trans_buf_free`. Thus lifecycle substitution alone would leave the
controller-owned buffer environment uninitialized. A separate adapter must keep
or replace that setup, allocate/free event buffers, transfer ownership of chained
`os_mbuf` ACL packets, and call the host from a guest task. Its controller BSS is
only 324 bytes, so the VHCI packet reservation cannot be copied unchanged.

This adapter was not implemented. C6 Server exits with status 2 and
`--ble: BLE is unsupported on this chip`; no C6 boot-to-advertising time or BLE
success is claimed. Notify, Write and Scan were not built or run on C6.
`inputs.json` retains ELF addresses and hashes for the inspected transport.
The installed NimBLE `transport.h` and the ELF disassembly are the primary evidence;
[ESP-IDF's VHCI source](https://github.com/espressif/esp-idf/blob/v5.5.4/components/bt/host/nimble/esp-hci/src/esp_nimble_hci.c)
explains the different S3/C3 H4 callback path.

## Checks and evidence handling

Workspace tests: 477 passed, 0 failed, 22 ignored.

`validation.json` records the release build, workspace tests, WASM build, privacy
check and whitespace check. Focused tests execute both instruction sets' actual
trampolines, callback arguments, host yielding, function-boundary dispatch,
invalid-buffer handling and physical flash restoration after MMU changes. C3 also
checks a halfword-aligned controller entry. Shared protocol tests cover malformed
packets, advertising/scanning, HCI capabilities, ATT operations and ACL reassembly.

`vhci-rv.S` preserves the small RV32IMC trampoline's source. Assemble with
`riscv32-esp-elf-as -march=rv32imc -mabi=ilp32`, extract `.text` with objcopy, and
compare bytes beginning at offset 48 with `esp32c3/src/ble.rs`. Same-section branch
encodings are also checked by the instruction-execution tests.

Home paths are normalized to `/Users/alice`. No machine identity, unrelated
process inventory, private capture or firmware binary is committed. Log hashes
identify original in-memory output and normalized retained output; redaction
changes labels only, not measured values or protocol bytes. Prior diagnostic
hashes remain in `negatives.json`. Full normalized logs and initial diagnostic
summaries stay under ignored `target/ble`. Nothing was pushed or posted.
