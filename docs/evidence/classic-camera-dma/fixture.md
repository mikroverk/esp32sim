# Public-driver camera fixture and first captured frame

## Scope and base

C8 adds a reproducible fixture on accepted C7.1
`06b7ae64c6795bd3b7c4c6210b077c05de9c83ad`, descending from accepted C7
`a3315ab85be28cd5318f2ab381a59f9b4b9c6b7a`.
No emulator implementation, PSRAM model, existing golden payload, waveform,
public driver source or DMA path changes are made by C8.

## Inputs and reproduction

[The build/run recipe](../../../examples/classic-camera/README.md) pins
ESP-IDF v5.4.3 (`ea1c174c1cbb7348bd8ba0ff1eb306246938dd80`), public
esp32-camera (`b817e9b81041980608d1a3d3e0b08d86e3666382`), Xtensa GCC
`esp-14.2.0_20250730` and esp_jpeg 1.3.1's registry hash. The 107 unmodified
driver files are fingerprinted in [fixture-driver-sources.json](fixture-driver-sources.json).
The lockfile is committed; fetched components and SDK/tools are not.

The native firmware uses the AI-Thinker wiring, stock camera initialization,
QQVGA RGB565, one internal-DRAM framebuffer, FreeRTOS and software SHA-256.
There are no symbol stubs, manually programmed DMA descriptors or substituted
frame returns. The host sets the board picture through its ordinary public
picture input before normal ROM → bootloader → application startup.

Two clean native builds in distinct source and output directories produced
identical bootloader, partition table and application images. All three also
match the committed CI inputs byte-for-byte:
[fixture-builds.json](fixture-builds.json). Reproduction is checked by
`examples/classic-camera/check-builds.py`, which asserts all six comparisons.
Reproducible-build configuration removes dates and maps source paths; the
project version is fixed to `c8-fixture-1`.

The application and bootloader link-map archive inventories are in
[fixture-linked-archives.json](fixture-linked-archives.json). The
[NOTICE](../../../esp32/tests/fixtures/classic-camera/NOTICE) identifies
linked licences and points to their accompanying full texts. No Wi-Fi/BT
controller or JPEG decoder is linked. The ROM is fetched, not redistributed.

## Captured bytes

Observed: stock driver initialization returns ESP_OK and identifies OV2640.
The CI test returns exactly two indexed frames, each 160 × 120 RGB565 and
38,400 bytes. Both guest-computed SHA-256 values equal:

`c67913e0afba1703e8c9c47e02b1ae2ea62e47596a8440d83f1ebba14aed6cf1`

The independent Python oracle uses no emulator imports: pixel `p` is
`(p % 256, (p // 160) % 256, (p * 7) % 256)`, reduced to RGB565 MSB-first.
[fixture-frame.json](fixture-frame.json) records its result.
`esp32/tests/support/camera_fixture.rs` checks the complete two frame records,
initialization and final completion, not only a success marker. The example
runner uses the same assertion helper and requires no ELF or symbol file.

## Mutation qualification

`examples/classic-camera/mutate.py` runs the real committed firmware for the
baseline and each mutation, restores all sources even on failure, and accepts
only compiled test/assertion failures. All five mutations were assertion-killed;
no compile failures or timeouts count:
[fixture-mutations.json](fixture-mutations.json).

| Mutation | Killing test |
| --- | --- |
| Camera dispatch disabled | public_driver_captures_host_frame |
| Transport drops bytes | public_driver_captures_host_frame |
| Wrong FIFO mode | public_driver_captures_host_frame |
| Sensor sample ordering reversed | public_driver_captures_host_frame |
| Classic EOF count changed from words to bytes | public_driver_captures_host_frame |

The baseline passes again in full ROM qualification after restoring the source.
This C8-specific suite is not a claim to rerun every inherited C4–C7.1 mutation.

## Checks and limits

Executed commands and results are recorded in `fixture-checks.json`.
All seven gates pass: 1,049 ROM-enabled workspace tests; 1,020 plain tests
(51 ignored); native and WASM Clippy; WASM build; eight named WASM demos;
and the evidence privacy check. Commands run at nice level 10 using Rust 1.99.0.
The ROM and plain workspace suites use an empty HOME; only the ROM suite has
ESP32SIM_ROM_DIR. Cargo/rustup locations remain explicit for tool discovery.
Both native/WASM Clippy commands deny warnings, and all eight required WASM
demos are exercised. JIT code is unchanged, so the JIT-specific script is not
required. Existing golden files are unchanged.

This is a bounded static-frame RGB565/internal-DRAM fixture, not validation of
JPEG, PSRAM, multiple changing host frames, streaming, per-PCLK accuracy or real
hardware. No production data was used. No new runtime code or hot-path layout
is introduced; no new codegen or performance claim is made.

## CPU comparison

PENDING. No timing run was performed.
