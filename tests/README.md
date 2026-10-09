# Tests

Five layers, base to top. `cargo test --workspace` runs the hermetic ones (no toolchain, no
download, no hardware); the goldens and the ABI tests need the mask ROM ELFs and run with
`--include-ignored` (CI fetches the ELFs). Nothing skips silently: a test either has a
checked-in oracle or is `#[ignore]`d with the reason in its name.

| layer | where | what it pins |
| --- | --- | --- |
| decoders and encoder | `xtensa-lx7/tests/objdump_diff.rs`, `riscv-rv32/tests/objdump_diff.rs`, `emu-core/src/jit_a64.rs` | checked-in sampled objdump corpora (app + mask ROM, `tests/corpus/`) decode to objdump's text, with PIE comparisons excluded by the Xtensa test; every AArch64 encoding in the fixture matches what clang assembled (`emu-core/tests/fixtures`) |
| core semantics | `xtensa-lx7/tests/semantics.rs`, `riscv-rv32/tests/semantics.rs` | on a flat RAM: register-window overflow at the instruction that touches the wrapped window, zero-overhead loops, a CCOMPARE interrupt landing on the same instruction through `step`, the block interpreter and the JIT, INTLEVEL masking, `waiti`; RV32 vectored interrupts, `mret`, `ecall`/`ebreak`; and the oracle property — the three execution paths leave identical state |
| peripherals | `esp-periph/tests/devices.rs` | systimer one-shot and periodic alarms and their deadlines, TIMG prescaler/alarm/autoreload, GPIO edge and level interrupts, USB SOF cadence per chip clock, the I2S rate from the clock registers; the `device_set!` table from outside: ranges, `delta`, `alias`, the generic fallback, source numbering, tick delivery per clock domain, the deadline minimum, `--debug` fan-out |
| machine | `esp32s3/tests/machine.rs`, `esp-soc/tests/parsers.rs` | idle cores skip time, core 1 runs when SYSTEM releases it, what a chip reset keeps, scripts against the board's pin names and encoder, console backlogs and masks, observers on both paths; the ELF/app-image/picture loaders never panic on random, truncated or hostile input |
| whole runs | `cli/tests/goldens.rs`, `wasm/tests/abi.rs` | the goldens below; the wasm C ABI driven natively for both chips, checking the web protocol's `board`, `serial`, `stat`, frame, audio and ring messages |
| the wasm build itself | `tools/wasm-test.mjs` | the real `esp32sim.wasm` under Node, driven through the page's firmware manifests (`web/wasm/fw/*.json`): boot, run, drain the outbox, expect the board message and the console line, no panic — the only layer that sees a wasm-only abort (a std that panics where it used to return nothing took every demo down once) |

GPIO feedback and release checks live in `tests/gpio_feedback.rs`, included by all three
chip crates, and `esp-periph/tests/gpio_release.rs`. C6 `spi_routes.rs` covers physical
SPI selection. They use register writes and board models without firmware fixtures;
[EX217](../docs/evidence/gpio-input-feedback/README.md) records their mutation checks.

The **golden-output tests** (`cli/tests/goldens.rs`) are the
regression bar for everything else: they run the committed demo firmware from the mask ROM and
compare the guest console, the captured audio (SHA-256) and the instruction count against the
files in `tests/golden/`. Bit-identical is the requirement — a timing change that shifts one
audio sample is a failure, not noise (see `docs/decisions.md`, "Performance").

They need the mask ROM ELFs, which ship with ESP-IDF (`~/.espressif/tools/esp-rom-elfs/`) or
can be pointed at with `ESP32SIM_ROM_DIR`, so they are `#[ignore]`d by default and never skip
silently: without a ROM they fail with the path they looked for. Without ESP-IDF,
`tools/fetch-demo-assets.sh --no-linux` fetches them into `web/wasm/fw/`, where `tools/wasm-test.mjs`
finds them by itself; for the cargo tests set `ESP32SIM_ROM_DIR=web/wasm/fw`.

```sh
cargo test --release --workspace -- --include-ignored --skip external_      # ~15 s for the whole set
UPDATE_GOLDENS=1 cargo test --release --workspace -- --include-ignored --skip external_   # after an intentional change
tools/wasm-build.sh && node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel   # the wasm module, as the page runs it
```

Tests named `external_*` need inputs only a developer machine has (full objdump listings via
`XTENSA_DIS_FILES`/`RISCV_DIS_FILES`, Apple's clang for the encoder fixture) and fail loudly
without them; run them by name. Their hermetic counterparts (`decoder_matches_corpus`,
`encodings_match_fixture`) run in the default suite against checked-in oracles.

Use `--release`: the debug build runs the same scenarios ~30x slower. On a mismatch the actual
output is left next to the golden as `*.actual` for diffing.

| golden | what it covers |
| --- | --- |
| `atech-script1.*` | Pocket Synth: buttons, encoder, serial command, ST7735 over bit-banged SPI, WS2812 via RMT, SID voice on I2S/GDMA; re-run with `--no-jit` (the JIT's oracle) |
| `atech-sid.*` | the cRSID C64 jukebox: a 6502 + SID inside the emulated S3 |
| `panel-sid.*` | Touch-LCD-4B energy panel: PSRAM, LCD_CAM RGB frames, GT911 touch and TCA9554 over I2C, ES8311 on I2S, a demo partition via `--flash-at` |
| `hello-s3.*` | stock ESP-IDF hello_world on UART0, ROM → bootloader → app_main |
| `hello-c3.*` | the same on the ESP32-C3 (RISC-V), with the MAC/reset cause/straps of the real module in `hw/c3-hello-world-real.txt`, through `esp32sim-c3` and `esp32sim --chip c3` |
| `hello-c6.*` | the same on the ESP32-C6 (RISC-V, RV32IMAC), with the identity of the Waveshare ESP32-C6-LCD-1.47 in `hw/c6-hello-world-real.txt`; the reboot variant also pins the ROM's `Saved PC` |
| `energy-scan-c6.*` (`external_`) | the 802.15.4 energy scanner on the Waveshare board — its owner's firmware, `ENERGY_SCAN_DIR` points at the build: PHY up, no panic, ≥50 scans, the board's report, console and count |
| `ble-advertiser-c3.*` | C3 original-controller advertising: console, BLE configuration, every observer PDU, exception and interrupt totals, per-source interrupt counts, and accounted instructions. The latter equal modeled cycles; timing regressions are checked by the interrupt report and directed tests. |
| `wifi-station-{s3,c3,c6}.*` | WiFi on all three chips: `examples/c6-wifi-station`, committed as `web/wasm/fw/public/*-wifi*` (rebuild in the example's README): the unmodified library scans, finds the virtual access point, joins through the WPA2 four-way handshake, takes a lease and gets five of five gateway pings. Pinned: the station's own lines (the same on every chip), the console, the end-of-run WiFi/network counts with the exception and interrupt counts, and the instruction count. On the C3 and C6 the instruction count is the cycle count, so a timing change in the WiFi model shows in the interrupt count |
| `cooja-echo.ndjson` (`cli/tests/cooja.rs`) | the Cooja-NG lock-step peer against a 32-instruction RV32 echo program: a hand-written NDJSON session with frames injected mid-slice; the echoes stamped at the `TX_START` cycle one air time after the frame went in, byte-identical twice over — no ROM, no toolchain |
| `cooja-ack.ndjson` (`cli/tests/cooja.rs`) | the stage-2 radio under a 55-instruction RV32 program with an address and auto-ACK: an AR frame to it is acknowledged by hardware exactly 192 µs after its end and echoed with AR on `ACK_TX_DONE`; the injected ACK is taken; without one the hardware timeout aborts; a frame to another address is filtered; a broadcast is echoed |
| `cooja-nullnet-c6.ndjson` (`external_`) | the same peer around Contiki-NG on ESP-IDF (`CONTIKI_C6_DIR` = esp32-contiki's `build-nullnet`): the periodic broadcasts as `tx` events at their `TX_START`, an injected broadcast reaching the driver after its air time and Contiki's nullnet callback, two sessions byte-identical |
| `atech-script1` with observers | the same run with `--profile-blocks --coverage --irq-latency --vcd` attached must be byte-identical and produce every report |

CI ([workflow](../.github/workflows/ci.yml)) gets the ROM ELFs with `tools/fetch-rom-elfs.sh`, which pins
the espressif/esp-rom-elfs release and each ELF's SHA-256 (the same script fills `web/wasm/fw/`), and runs the hermetic and golden tests on every push and pull request. It excludes tests named
`external_*`; it does not rerun hardware comparisons or full external objdump listings.
