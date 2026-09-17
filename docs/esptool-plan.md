# esptool and UART download-mode integration plan

Date: 2026-09-17

Status: milestones 1 through 5 implemented for ESP32-S3 with the ROM loader and modern v2 stub.
Milestone 6 is in progress: C3 and C6 pass ROM-loader `flash-id`, and esptool 4.8.1's v2 stub now
passes upload, `flash-id`, compressed write and digest verification on both chips. The legacy v1
S3 stub has the documented upstream address defect below. C3/C6 persistence, the remaining command
matrix and automated external-tool regression coverage remain.

Update, 2026-09-17: the apparent ULP mismatch was a stale default Cargo target artifact; a fresh
target builds the complete CLI. S3 strap `0x7` reaches `UART0_BOOT`. With UART autobaud counters
modelled, esptool 4.7.0 `--no-stub` passes `flash-id`, writes the repository's 161,712-byte
`hello_world.bin`, and verifies its digest through `socket://`. Default stub mode synchronizes and
starts uploading, but the transition to the uploaded stub still fails and remains under
investigation.

Further result: pacing TCP input at the emulated 115200-baud line rate allows both stub generations
to upload. The legacy v1.3.0 S3 stub then panics in its flash-status path because its official
source hard-codes the classic ESP32 `g_rom_flashchip` address `0x3ffae270`, while the same source's
S3 ROM linker file places `rom_spiflash_legacy_data` at `0x3fceffe4`. No non-silicon address alias
is added to hide that upstream defect. The newer v2 stub works after modelling the S3/C3 UART
configuration-update self-clear and correcting Xtensa's reset `CPENABLE` value to `0xff`: esptool
4.8.1 v2 passes `flash-id`, compressed write and flash digest verification. An independent host
comparison confirms the persisted flash range is byte-identical to the 161,712-byte input image.

Lifecycle acceptance also passes: the v2 stub flashed bootloader, partition table and application
into a fresh 8 MiB `--flash-state`; a separate process booted that state through the real mask ROM
and printed `Hello world!`.

C3/C6 parity update: C6 ROM autobaud uses bit 19 and its pulse/count registers are at
`0x7c/0x80/0x84`. Both RISC-V chips also expose the raw peripheral source bitmap through their
interrupt-matrix `INTR_STATUS_REG_n` registers. Before those registers were modelled at their real
offsets, the CPU entered the UART priority handler but esp-hal observed no UART source, returned
without draining the FIFO, and immediately took the same level interrupt again. With the status
registers and FIFO-read interrupt refresh in place, the unmodified v2 stubs service UART0 and pass
161,712-byte compressed writes with flash hash verification on C3 and C6.

## Objective

Allow unmodified `esptool`, `espefuse` and `idf.py flash` processes to communicate with a
running esp32sim instance over a raw TCP UART connection. Both the mask-ROM loader
(`--no-stub`) and esptool's uploaded RAM flasher stub are required outcomes.

The implementation must preserve esp32sim's existing execution model. The real mask ROM remains
the source of truth for the download protocol; esp32sim will not add a host-side implementation
of SLIP or esptool commands.

## User-facing target

The intended native workflow is:

```sh
esp32sim --chip s3 --boot download \
  --uart-tcp 127.0.0.1:5555 \
  --flash-mb 16 \
  --flash-state .state/device-flash.bin
```

In another shell:

```sh
esptool.py --chip esp32s3 --port socket://127.0.0.1:5555 \
  --before no-reset --after no-reset flash-id

ESPPORT=socket://127.0.0.1:5555 idf.py flash
```

Raw TCP does not carry RTS/DTR modem-control state. Automatic reset through esptool is therefore
out of scope for the initial implementation. Commands must use no-reset behavior, and the
emulator must provide an explicit, deterministic way to restart into SPI-flash boot afterwards.

## Design constraints

- UART transport is raw bytes. It must never pass through UTF-8 conversion, JSON or the web
  console protocol.
- Socket reads must not be copied directly into the 128-byte emulated UART FIFO. A bounded host
  queue supplies bytes only as the guest drains FIFO space, preventing packet-sized TCP reads
  from becoming artificial UART overruns.
- Socket writers must not run on the emulator thread. A stalled client cannot stall CPU/device
  scheduling.
- TCP connection and reconnection do not reset the chip or discard flash.
- Download mode boots the real mask ROM. Protocol emulation, command shortcuts and synthetic
  flasher responses are not acceptable.
- Existing console, script and WebSocket input paths remain unchanged when the TCP bridge is off.
- Firmware seed images remain immutable. Mutable flash and eFuse state use the existing state
  backends rather than overwriting build artifacts.
- Native-only transport code must not add socket dependencies to the WebAssembly build.

## Milestone 0 — characterization and retained fixtures

Purpose: establish observable download-mode behavior before changing the machine.

Work:

- Record the correct download and SPI-boot strap values for S3, C3 and C6 from their ROM behavior.
- Add a small retained, non-sensitive esptool command corpus: sync, chip identification and flash
  identification request/response boundaries. Do not reimplement the protocol from those bytes.
- Add a ROM-download smoke harness that can report the first unimplemented instruction,
  peripheral register or exception without hiding it behind transport errors.

Acceptance:

- Each supported chip either reaches its ROM download receive loop or has a documented, directly
  observed blocker.
- No production behavior is changed by the characterization tests.

## Milestone 1 — native raw UART/TCP transport

Purpose: make socket behavior independently testable before attaching it to a chip.

Work:

- Add a native-only TCP listener with one active client, reconnect support and `TCP_NODELAY`.
- Provide bounded binary input/output queues.
- Apply TCP backpressure instead of dropping bytes when a queue is full.
- Expose connection state and the listener's resolved address for diagnostics and tests.

Acceptance:

- Round trips preserve `0x00`, `0xc0`, `0xdb`, `0xff` and arbitrary non-UTF-8 bytes exactly.
- Input can be drained in smaller pieces without loss or reordering.
- A second connection replaces a disconnected client without restarting the listener.
- Queue bounds and disconnected-output behavior have tests.
- Native `esp-soc` tests pass; the WebAssembly build continues to compile without the module.

## Milestone 2 — machine and CLI integration

Purpose: expose UART0 as an interactive host transport without changing UART device semantics.

Work:

- Add `--uart-tcp HOST:PORT` and attach the transport to UART0.
- Route UART0 TX to the socket while preserving internal console capture needed by tests.
- Feed queued input only up to currently available UART RX capacity.
- Poll host input at scheduler boundaries and wake interrupt delivery immediately after injection.
- Prevent emulated receive timeouts from outrunning a waiting host. Initially, TCP mode may force
  real-time pacing; remove that restriction only after deterministic host-wait behavior exists.
- Keep USB-Serial/JTAG and UART1/2 behavior unchanged.

Acceptance:

- A binary loopback firmware exchanges multi-kilobyte data without corruption or RX overflow.
- Slow and reconnecting clients cannot block the emulator thread.
- Runs without `--uart-tcp` retain their golden console output and instruction counts.

## Milestone 3 — real ROM loader (`--no-stub`)

Purpose: prove the real mask-ROM download path before involving uploaded code.

Work:

- Add `--boot download`, mapping to the correct per-chip strap while retaining `--strap` as the
  low-level override.
- Validate esptool sync, chip-id, flash-id, read-flash, write-flash, verify-flash, erase-region and
  erase-flash using `--no-stub`.
- Fix only directly observed UART, ROM, SPI flash, reset or eFuse model deficiencies.
- Preserve and test NOR semantics: programming only clears bits; erase restores `0xff`.

Acceptance for S3:

- Every command above succeeds through `socket://` against an unmodified esptool release.
- Written bytes are visible to both the ROM loader and a subsequent normal ROM boot.
- Failures and disconnects leave a valid flash state.

## Milestone 4 — uploaded esptool flasher stub

Purpose: support normal esptool and `idf.py flash` behavior rather than requiring a diagnostic
fallback.

Work:

- Validate RAM upload, execution transfer and the stub greeting.
- Validate compressed and incompressible writes at the default block sizes.
- Exercise stub read, write, verify and erase paths.
- Investigate failures through instruction/peripheral traces; do not bypass the stub with
  host-side command handling.

Acceptance for S3:

- The same command matrix passes with esptool's default stub mode and `--no-stub`.
- `idf.py flash` succeeds when configured with `ESPPORT=socket://...` and no-reset options.
- Stub and ROM-loader writes produce identical flash bytes.

## Milestone 5 — lifecycle, persistence and operator workflow

Purpose: make flashing useful beyond a single emulator process.

Work:

- Integrate S3 `--flash-state` and `--efuse-state` with download-mode mutations.
- Define explicit post-flash behavior: restart the process, or request a controlled strap change
  followed by chip reset. Do not infer reset from a TCP disconnect.
- Flush persistent storage before controlled reset and normal shutdown.
- Add graceful socket shutdown and concise diagnostics without dumping protocol or fuse contents.
- Document fresh-device, retained-device and recovery workflows.

Acceptance:

- Flash with esptool, exit, restart in SPI-boot mode and run the newly flashed application.
- NVS/OTA data and successful supported eFuse burns survive restart.
- Seed images remain byte-identical.

## Milestone 6 — C3 and C6 parity

Purpose: extend the proven S3 path without weakening the more mature target.

Work per chip:

- Run the complete ROM-loader and stub command matrices.
- Fix directly observed chip-specific ROM, UART, SPI and reset gaps.
- Add flash persistence before claiming cross-process flashing support.
- Add eFuse persistence before claiming `espefuse` support.

Acceptance:

- Capability is reported per chip and per command; unsupported combinations fail explicitly.
- S3 regressions remain green while C3/C6 support is added.

## Milestone 7 — regression suite and release documentation

Work:

- Pin the tested esptool and ESP-IDF versions in test metadata.
- Add native integration tests for no-stub, stub, `idf.py flash`, disconnect/reconnect and restart.
- Add negative tests for malformed addresses, queue pressure, missing ROM, unsupported chips and
  state-size mismatch.
- Document differences from Espressif QEMU and esp-emulator, especially reset handling and
  esp32sim's separate immutable seed versus mutable state files.

Completion criteria:

- Default esptool stub mode and `--no-stub` both work on S3.
- `idf.py flash` works over `socket://` with documented no-reset settings.
- Flashing is byte-accurate, persistent and followed by a successful normal ROM boot.
- C3/C6 support is either complete or precisely reported as partial with retained failing tests.
- No host-side protocol shortcut, mock firmware or simplified replacement component is introduced.
