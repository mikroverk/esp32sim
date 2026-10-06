# Reproduce the C3 hardware comparison

Use Arduino-ESP32 3.3.11, ESP-IDF 5.5.5, and the C3 rev3 mask ROM pinned in
`receipt.json`. Copy `Probe.ino` into a directory named `Probe`, then build with
`arduino-cli compile --fqbn esp32:esp32:esp32c3 --build-path build Probe`.
Use the resulting bootloader, partition table and application binary on both
hardware and emulator. Flash the core's `boot_app0.bin` at `0xe000` on hardware,
bootloader at zero, partitions at `0x8000`, application at `0x10000`, with 4 MB flash.
Pass your own serial port to esptool. Capture one complete boot through
`PROBE kind=done version=1`; do not connect a BLE central during the capture.

Set `PROBE_DIR` to the directory containing the images and `ROM` to the rev3 ROM.
Run the locally built candidate without an application ELF or virtual peer:

```sh
cargo +1.99.0 build --release --bin esp32sim-c3
mkdir -p target/hardware
target/release/esp32sim-c3 --boot rom --rom "$ROM" --flash-mb 4 \
  --bootloader "$PROBE_DIR/images/bootloader.bin" \
  --ptable "$PROBE_DIR/images/partitions.bin" --app "$PROBE_DIR/images/app.bin" \
  --ble full --max-seconds 15 --no-dump > target/hardware/emulator.stdout
rg '^PROBE ' target/hardware/emulator.stdout > target/hardware/emulator.probe.log
python3 docs/evidence/ble-c3-advertising/hardware/compare.py \
  "$PROBE_DIR/hardware.probe.log" target/hardware/emulator.probe.log \
  > target/hardware/comparison.json
python3 docs/evidence/ble-c3-advertising/hardware/check_compare.py \
  target/hardware/emulator.probe.log
```

Exit zero requires matching static registers and mapping starts, structural checks,
clock-rate checks and programmed-interval medians within 10 ms. The comparator
retains asynchronous differences and device-specific bytes; its full output is
private scratch data, not a publishable receipt. LC+2cc is now classified as live
error diagnostics for the ROM-derived reason in the parent receipt. The self-check
ensures its differing value is retained and identity/UUID corruption still fails.

The committed source is byte-identical to the captured probe. Its `INFERRED.md`
reference names the original local inventory; the parent README contains the
applicable inferred-contract inventory. The original private captures and build
products are retained locally, not distributed here. Hashes in `receipt.json`
identify their original bytes; no hardware address or raw device dump is copied.
