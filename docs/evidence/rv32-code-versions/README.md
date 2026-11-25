# Code versions on the C3, C6 and P4 buses: decoded blocks and the RV32 JIT there (EX270)

Revisions: main `8c1c227d`; the change is `jit/rv32-all` `588a00c0` (main + `jit/c3` `a4757049`,
`jit/c6` `d4f2ea5e`, `jit/p4` `5997e66c`), rustc 1.100.0.

## Workload

Espressif's own CoreMark test app, `test_apps/coremark` of github.com/espressif/esp-emulator at
`6bc8abda` (2026-11-20), unmodified (`sdkconfig.defaults`: task watchdog off, 160 MHz where the chip
has it). Built with ESP-IDF v5.5.4 for the C3, C6, S3 and P4 (the P4 build is the default rev ≥ 3
one, run with `esp32p4_rev300_rom.elf`) and with ESP-IDF v6.1 for the S31. One image per chip runs on
both emulators: esp32sim takes the bootloader, partition table and app, esp-emu the `esptool
merge-bin` of the same three. Image hashes: `coremark-mac.json` `firmware_sha256`.

A run ends at the app's `CPU frequency` line (`--exit-on` on both emulators); every run checked
`[0]crcfinal : 0xa14c`. The guest work is the same on both emulators for the C3, C6 and S31 (the same
Iterations/Sec: 533.43 on the C3 and C6, 1093.7 vs 1093.9 on the S31); on the S3 (857.3 vs 571.4) and
the P4 (1200.2 vs 1333.9) the two emulators' cycle models differ, so those rows compare different
emulated cycle counts. Neither model was checked against silicon here.

## Measurement

`coremark_bench.py` (paths are caller-supplied): one warm-up run of each side, then 5 rounds in
alternating order (esp-emu, main, new; reversed every other round). Per run: user CPU seconds and
host instructions retired from `/usr/bin/time -l`. Apple M5 Max, macOS 27.2.1, all three native
AArch64 builds (esp-emu 1.2.0 release asset, SHA-256 in `coremark-mac.json`, matching the release's
SHA256SUMS). The machine also ran other work: the 1-minute load average was 4.8–40 during the runs
(per-run values in the JSON); host instructions are the load-independent measure.

| chip | esp-emu 1.2.0 | main | this change | change vs main | vs esp-emu (user time) |
|---|---|---|---|---|---|
| C3 | 7.64 s, 143.2 G | 15.17 s, 411.6 G | **4.36 s, 92.9 G** | 3.48x | 1.75x faster |
| C6 | 7.12 s, 147.7 G | 12.37 s, 363.1 G | **3.39 s, 79.3 G** | 3.65x | 2.10x faster |
| P4 | 6.99 s, 148.1 G | 15.36 s, 455.6 G | **5.38 s, 133.7 G** | 2.86x | 1.30x faster |
| S31 | 7.65 s, 172.2 G | 6.33 s, 157.3 G | 6.35 s, 157.3 G | (already had it) | 1.20x faster |
| S3 | 7.30 s, 215.1 G | 5.01 s, 155.6 G | 5.03 s, 155.6 G | (Xtensa, unchanged) | 1.45x faster |

Medians of 5; host instructions retired in G. The S31 and S3 rows are controls for the change (same
host instructions as main).

Before the change, on x86-64 Linux (i9-12900F, one P-core, user CPU only; esp32sim main, so the C3,
C6 and P4 interpreted; the JIT has no x86 backend): esp-emu was 2.26x (C3), 2.01x (C6), 1.54x (S3) and
2.70x (P4) faster than esp32sim (`coremark-x86-main.json`). This change was not measured on x86, where
the C3/C6/P4 now run the decoded blocks without the JIT (the blocks alone, `RV32_JIT=0` on the Mac:
C3 202.7 G, C6 189.5 G, P4 243.7 G host instructions, about half of main's).

## Correctness

- Goldens bit-identical, none regenerated, with blocks+JIT and with `RV32_JIT=0` (48 goldens; the 14
  P4 goldens incl. rev ≥ 3 and both-core runs, the C3/C6 WiFi, BLE and TLS goldens).
- `cargo test --release --workspace -- --include-ignored --skip external_` on `588a00c0`: 1912 passed,
  0 failed; both Clippy commands with `-D warnings` pass.
- Per-chip unit tests: `fetch_ahead` equals `fetch` without its fault record; every write path moves
  the version of the page it changed (CPU stores through each alias, every DMA engine that writes
  memory, SPI1 program/erase, MMU changes, loaders) and not unrelated ones; a self-modifying-code test
  per chip runs code rewritten after it was compiled (own store, the other core on the P4, DMA, WiFi
  RX, MMU remap, flash erase+program). Mutation checks: C3 18/18, C6 17/17 (5 rechecked
  independently), P4 17/17 killed. P4 hardware loops: blocks and interpreter match at 400 budget cuts.
- c3-hello, c6-hello and the P4 hello goldens: the same guest instruction counts and console as main.

## Limits

- The interpreter path (`--no-jit`) on the P4 costs about 2.3% more host instructions than main on
  CoreMark (the version bumps on stores and the shared fetch reader); not tuned.
- The memory and MMU fields of the C3/C6/P4 `SocBus` are public; code that changes them directly must
  call `invalidate_code()` (documented on the fields; only tests do so today).
- The C3 and C6 keep one version for the whole flash window (any mapped program/erase or MMU change
  invalidates all flash blocks); the P4 keeps one per 64 KB MMU page.
- WebAssembly (`tools/wasm-build.sh`, `tools/wasm-test.mjs` under Node, single runs): c3-hello,
  c6-hello, p4-hello, p4-lcd35-synth, p4-c6-hosted and s31-mosaico give main's instruction, console-line
  and frame counts; the C6 workloads (c6-xiao, c6-wifi, c6-energy-scan, c6-contiki(-net), c6-rpl-net)
  were checked the same way. Wall times are single runs on a loaded machine: p4-lcd35-synth 7.7 -> 5.9 s,
  p4-c6-hosted 4.4 -> 4.1 s, the idle hello demos unchanged.
