# Persistent flash, NVS and eFuse state

esp32sim can retain an ESP32-S3's mutable flash and eFuse contents across
separate emulator processes. Persistence is optional: without the state flags,
every process starts from the supplied images and default eFuses as before.

## NVS is persistent flash

There is no separate NVS emulator. ESP-IDF's real NVS library reads, writes,
erases and commits records inside an ordinary SPI-flash partition. Consequently,
`--flash-state` persists NVS as well as OTA metadata, PHY calibration and every
other flash-backed partition without esp32sim interpreting their contents.

## Native command-line flags

| Flag | Purpose |
| --- | --- |
| `--flash-state PATH` | Load or create the mutable S3/C3/C6 logical flash state file |
| `--efuse-state PATH` | Load or create the mutable S3/C3/C6 physical eFuse state file |

Flash and physical eFuse state support S3, C3 and C6.

Example:

```sh
esp32sim --chip s3 --boot rom \
  --bootloader build/bootloader/bootloader.bin \
  --ptable build/partition_table/partition-table.bin \
  --app build/app.bin \
  --flash-mb 16 \
  --flash-state .state/device-1-flash.bin \
  --efuse-state .state/device-1-efuse.bin
```

Run the same command again to continue with the NVS, OTA and eFuse state left
by the first process.

## Seeds and mutable state

Image flags are immutable seeds; state files are the emulated device:

| Storage | State file missing | State file exists |
| --- | --- | --- |
| Flash | Start erased, apply `--flash-image`, `--bootloader`, `--ptable`, `--app` and every `--flash-at`, then create the state file | Load the state file and ignore all flash seed images |
| eFuse | Start from chip defaults, apply `--efuse-regs` if supplied, then create the state file | Load physical eFuses from the state file and ignore `--efuse-regs` |

This precedence lets a runner reuse one stable command. It also means changing
an app, partition table or seed does not update an existing emulated device.
Delete or select a different state file when a genuinely fresh device is
required.

esp32sim never overwrites a file supplied by `--flash-image`, `--bootloader`,
`--ptable`, `--app`, `--flash-at` or `--efuse-regs`.

## Persistence timing and reset behavior

- Completed guest flash program and erase commands are written through to the
  flash-state file.
- A successful eFuse burn is written and synchronized immediately.
- Guest software reset preserves flash and physical eFuses, but clears eFuse
  staging/controller state.
- State is synchronized before a guest reboot and at a normal emulator stop.
- Initial files are written to a temporary sibling and atomically renamed.
- New native state files use user-only permissions (`0600`) on Unix hosts.

An OS or filesystem failure is reported as an emulator storage error. A host
power failure can still lose data the host filesystem has not durably committed;
this is not a full-process or crash-consistent CPU snapshot.

## File formats

### Flash

The flash-state file is exactly the configured flash capacity. A 16 MiB run
therefore requires a 16 MiB state file. A different size is rejected.

It contains esp32sim's logical, CPU-visible flash bytes. When firmware exercises
flash-encryption paths, esp32sim still stores logical decrypted bytes. The file
is not a physical encrypted-SPI dump and must not be treated as interchangeable
with production flash ciphertext.

### eFuse

The native ESP32-S3 eFuse-state file is 336 bytes: 84 little-endian words in
physical block order. Program staging and derived read-shadow registers are not
serialized. One-way programming, whole-block write protection and protected-key
read hiding are enforced by the device model.

This 336-byte payload matches Espressif QEMU's `ESPEfuseBlocks` byte for byte:
blocks 0 and 1 contain six words each and blocks 2 through 10 contain eight.
esp32sim also accepts QEMU's commonly used 1 KiB backing file and reads/writes
the compatible 336-byte payload at offset zero while preserving its padding.
New esp32sim state files remain the compact 336-byte form.

eFuse state may contain security-sensitive material if firmware burns it. Keep
state files private, never commit production keys, and avoid printing or sharing
their contents.

## Browser persistence

The WebAssembly UI stores ESP32-S3 state in IndexedDB; binary flash is not put
in `localStorage` and nothing is uploaded.

Open **Configure** and use:

- **persist state** to enable or disable browser persistence;
- **profile** to name an independent virtual device;
- **Export state** to download flash and eFuse files;
- **Import** to replace flash or eFuse state after validating its size;
- **Clear state** to remove only the selected profile after confirmation.

The profile identity includes ESP32-S3, board name, flash size and the visible
profile name. Browser storage is scoped to the page's origin, so `localhost`,
`127.0.0.1`, another port and a hosted site have separate IndexedDB data.

Dirty state is saved after a short debounce and when the page is hidden or
unloaded where the browser permits it. Reload after an import or clear operation
to boot from the replacement/fresh state. Browser persistence currently applies
to one ESP32-S3 machine, not C3/C6 or multi-node network manifests.

## Common mistakes

- **My new firmware did not load:** an existing flash state took precedence.
  Select a fresh state path or delete the old state intentionally.
- **State size error:** `--flash-mb` and the flash-state file must agree; eFuse
  state must be either the native 336-byte payload or a 1 KiB QEMU backing file.
- **NVS disappeared:** both runs must use the same `--flash-state` path or the
  same browser profile and origin.
- **An eFuse bit will not clear:** physical eFuses are one-time programmable;
  create a fresh eFuse-state file instead.
- **An `espefuse` burn disappeared:** use the same `--efuse-state` path on the
  download-mode and subsequent processes.
