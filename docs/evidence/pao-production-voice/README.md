# Production PAO voice flow

The PAO ESP32-S3 firmware at Xiaozhi base
`b3ff05c030923ebf44f494ea1cba54752d638803` uses ESP-IDF 6.0.1,
Wi-Fi, I2S0 TX, I2S1 RX, PSRAM, Opus and the `wn9_nihaoxiaozhi_tts` model.
Its production configuration enables WakeNet; it does not use OpenETH, a fake
codec, guest function stubs or injected application state.

The production binary SHA-256 is
`bc89da3690a7ccb97f0c6afa5b2d3755253bf6602378338f50d827904578bbfa`.
Starting from an NVS containing only a board UUID, the firmware:

1. Started its own SoftAP; an emulated client joined and obtained a DHCP lease.
2. Served its actual HTTP provisioning page, scanned the virtual WPA2 AP,
   accepted credentials and saved the SSID.
3. Connected and activated against the configured Lingxi service.
4. Cold-booted from saved flash and reconnected without provisioning.
5. Detected the actual microphone phrase “你好小智” through WakeNet.
6. Finished the greeting, recognized “你好，请用一句话介绍你自己。”,
   received a cloud response and returned to listening after playback.

The second full-flow run captured 1,032,019 mono samples at 24 kHz,
including 413,694 nonzero samples. Of these, 199,726 occurred after the
conservative sample offset corresponding to greeting completion, demonstrating
output beyond startup/greeting audio. Device identities, PCM input, flash,
RAM dumps, cloud configuration and raw logs are private local inputs/artifacts.
The reproducible entry point is `firmware/simulation/esp32sim-flow.py` in
[pao-1024/pao](https://github.com/pao-1024/pao).

## CPU diagnosis

`LSIP`, `SSIP`, `LSXP` and `SSXP` accessed the incremented address instead of the
original address. WakeNet consequently read the next normalization parameter.
An isolated model call from a QEMU memory checkpoint diverged at instruction
237,357 on `LSIP`. After correcting the four postincrement forms, the call
returned after 397,250 instructions: observed general, floating and Q registers
matched the QEMU trace, and all changed PSRAM bytes matched.
This diagnostic checkpoint enabled CP3 because QEMU's PIE translator omits its
guard; the production firmware runs retain all coprocessor checks.

The standalone reciprocal, reciprocal square root and nearest multiply-add
instructions also now write their results. Reciprocal seeds use rounded host
arithmetic, not bit-exact silicon lookup values. Hermetic tests cover the four
postincrement forms and standalone float operations in step, block and native
JIT execution. The existing window-operand test now supplies a mapped original
base for postincrement accesses.

## Disabled-feature comparison

On macOS ARM64, Rust 1.99.0 release binaries, three alternating runs of upstream
`b97fdc7c` and this branch, each with 30 emulated seconds and no networking or
PCM input, produced identical instruction counts and console output hashes.

| Firmware | Upstream median user seconds | Branch median user seconds | Instructions, core 0 / core 1 |
| --- | ---: | ---: | --- |
| S3 hello | 0.267166 | 0.258967 | 11,990,488 / 6,798,360 |
| C3 hello | 2.464098 | 2.479192 | 4,800,000,000 / — |
| C6 hello | 1.722193 | 1.693255 | 4,800,000,000 / — |
| Pocket Tank | 41.005127 | 41.555439 | 4,548,165,341 / 6,107,970,320 |

Pocket Tank used the committed public firmware and the model verified by
`tools/fetch-pocket-tank.sh`. These are host CPU comparisons, not silicon timing
or real microphone/acoustic validation.
