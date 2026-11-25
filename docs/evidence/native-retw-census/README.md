# Native guarded RETW census

This receipt compares the AArch64 native JIT at base `04f07602dba76eb6704b8901c989c9c9c1f8b42a` and the guarded-return change. The census is counts-only; two separate CPU comparisons follow it, the author's on the census fixture and the maintainers' on in-repo workloads. None of them is a production workload estimate. The existing WASM implementation and priced-mode interpreter were not changed. Earlier guarded-return attempts and their limitations are recorded in [EX109](../../experiments.md#ex109).

## Census (author's run)

The S3 serial starter is an idle-heavy 3-second emulated run, with 8 MiB flash and no PSRAM. The original factory image had SHA-256 `51a42d3456138d775a0a8cf2f127b9e62349e0e9aec6e5ed65e6707a7a6eecb1`; the S3 ROM ELF had SHA-256 `c0ce0f338d1de1bdc6efbef1591779a2a42c1ab7d759d3c6ae8ae63a7dd34cfd`. The starter uses Arduino on `esp32-s3-devkitc-1` with pioarduino platform `55.03.38-1`; its build embeds timestamps, so a rebuilt factory image need not have the same hash.

A diagnostic build counted interpreter-helper calls by decoded opcode; this instrumentation changes host work and must not be used for CPU timing. The CLI invocation was:

```sh
esp32sim --chip s3 --board none --boot rom --rom "$ROM" --flash-image "$FACTORY" --flash-mb 8 --psram-mb 0 --console both --no-reboot --max-seconds 3 --no-dump
```

| Measurement | Base | Guarded return |
|---|---:|---:|
| RETW.N helper calls | 510,100 | 28,464 |
| core 0 instructions | 8,270,125 | 8,270,125 |
| core 1 instructions | 10,634,791 | 10,634,791 |
| emulated cycles | 720,000,000 | 720,000,000 |
| console SHA-256 | `0b06c16529e781f46bbdf92be4880ebe51f9d9210691f26ce871019404e959f7` | same |

All remaining RETW.N helper calls in this sample trapped. Equal totals and console output do not prove full machine-state equivalence. The native-JIT differential tests (`xtensa-lx7/tests/jit_retw.rs`) cover both encodings, all three eligibility failures and underflow for increments 1–3, from a PS with a stale CALLINC, OWB and INTLEVEL, and a return to the next instruction that the block must not run past; the opt-in priced path already uses the interpreter on native. Removing any one guard makes those tests fail.

Limits: the factory image is not in the repository and the counter patch was not kept, so the census cannot be reproduced from this tree. The host model of the run was not recorded.

## CPU comparison (author's run)

Base `04f07602dba76eb6704b8901c989c9c9c1f8b42a`; candidate `63a7fe9fe074a6b368d8d3ac80b15161887b4360`. The run used rustc 1.99.0 on macOS AArch64 (host model not recorded) and measured `wait4` user CPU with a timer that is not in the repository. The start load was 3.22; the run used one warmup pair followed by seven alternating pairs on the 30 emulated seconds S3 serial starter: the census command above with `--max-seconds 30`, in a normal release build. All accepted runs matched core 0 / core 1 instruction counts of 54,872,125 / 98,195,791 and console SHA-256 `0b06c16529e781f46bbdf92be4880ebe51f9d9210691f26ce871019404e959f7`.

| Pair | Base user CPU s | Candidate user CPU s | Delta |
|---:|---:|---:|---:|
| 1 | 1.018971 | 0.973587 | -4.45% |
| 2 | 0.991808 | 0.963707 | -2.83% |
| 3 | 0.989567 | 0.970015 | -1.98% |
| 4 | 0.992497 | 0.967608 | -2.51% |
| 5 | 0.995071 | 0.944880 | -5.04% |
| 6 | 0.994653 | 0.952342 | -4.25% |
| 7 | 0.989987 | 0.961682 | -2.86% |

Median paired delta -2.86%; candidate slower 0/7; not flagged (threshold ≥6/7).

This is one idle-heavy fixture, approximately one CPU second per run; it is not a production-wide speedup.

## CPU comparison (maintainers' run)

New `0acd7190d67798edf84c5c2d02bdb7b48a31640e` (the candidate above plus docs), base `04f07602dba76eb6704b8901c989c9c9c1f8b42a`, measured 2026-11-01. Both trees were extracted with `git archive` into sibling directories with paths of the same length and built with `cargo build --release --bins`, rustc 1.99.1 (58cf8f9c3 2026-10-13), aarch64-apple-darwin. Machine: Apple M5 Max (18 cores), macOS 27.0.1. ROMs: esp-rom-elfs 20260528 (`tools/fetch-rom-elfs.sh`). Each run of the harness started with the 1-minute load below 5 and nothing building; later workloads in the same run started wherever the load was (last column). One warmup pair, then 7 alternating pairs per workload. User CPU comes from rusage at 0.01 s resolution, so on the 0.25 s and 0.39 s workloads one tick is 2.5–4%.

| Workload | Base median (range) | New median (range) | Change | New slower | Instructions, console | Load before → after |
|---|---|---|---:|---:|---|---|
| s3-hello (30 s) | 0.250 s (0.240–0.250) | 0.250 s (0.240–0.250) | +0.0% | 1/7 | identical in all 16 runs | 4.69 → 4.80 |
| pocket-tank (8 s) | 7.470 s (7.290–7.520) | 7.440 s (7.310–7.650) | -0.4% | 2/7 | identical in all 16 runs | 4.80 → 5.63 |
| classic-hello (30 s) | 0.390 s (0.380–0.400) | 0.390 s (0.380–0.410) | +0.0% | 4/7 | identical in all 16 runs | 5.63 → 5.45 |
| atech script1 (5 s) | 1.610 s (1.590–1.630) | 1.570 s (1.560–1.580) | -2.5% | 0/7 | identical in all 16 runs | 4.55 → 4.36 |
| atech SID jukebox (6 s) | 1.530 s (1.510–1.550) | 1.510 s (1.490–1.520) | -1.3% | 0/7 | identical in all 16 runs | 4.36 → 4.97 |

Nothing is flagged (new slower in at least 6 of 7 pairs, or ranges that don't overlap). Both atech workloads, the in-repo RETW-heavy ones, are faster in all 7 pairs: script1 by 2.5% with the ranges apart, the SID jukebox by 1.3%. s3-hello, pocket-tank and classic hello don't move. The C3, C6 and P4 are RISC-V and not affected.

Harness: [`docs/evidence/p4-cpu/cpu_pairs.py`](../p4-cpu/cpu_pairs.py) (unchanged since `04f07602`; SHA-256 `efb6406cb9a27ec82db3ee3c00075992f20d8f42fb90d99417b41876d9011e41`) with three workloads added to its `workloads()` table. Their commands are in the JSON below: `classic-hello` (`--chip esp32 --board esp32dev` with the repository's classic hello images), and `atech-script1` and `atech-sid`, the first two commands of `tools/bench-goldens.sh`. The modified copies (SHA-256 prefixes `c9e8ca4b` for the first file, `f0b6e70a` for the second) were not kept. pocket-tank needs `web/wasm/fw/local/model_q4.bin` from `tools/fetch-pocket-tank.sh`. From the base tree's root, with `BASE` and `NEW` the two `target/release` directories:

```sh
ESP32SIM_ROM_DIR=... python3 cpu_pairs.py --base "$BASE" --new "$NEW" --workloads s3-hello,pocket-tank,classic-hello --out 334-0acd7190-04f07602.json
ESP32SIM_ROM_DIR=... python3 cpu_pairs.py --base "$BASE" --new "$NEW" --workloads atech-script1,atech-sid --out 334-0acd7190-04f07602-extra.json
```

Results, as published on the pull request: [`334-0acd7190-04f07602.json`](334-0acd7190-04f07602.json) (SHA-256 `721ccbba66e521db8899858b5dabbebc4ef9f23a75a4cda4a6d7c81dc1e83eec`) and [`334-0acd7190-04f07602-extra.json`](334-0acd7190-04f07602-extra.json) (SHA-256 `54c764d5cc741ae8273439bc60dba9d737139b4a57a09be6efbff76a863492b7`). The only edit to the raw files is the ROM directory, written as `$ESP32SIM_ROM_DIR`; the raw files had SHA-256 prefixes `0141aaa8` and `02012c8f` and were not kept. The edit does not change any measured value.

The code measured is the code merged: later commits change tests, comments and a debug-only assertion, and the emitted AArch64 code is the same.
