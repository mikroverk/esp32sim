# Native guarded RETW census

This counts-only experiment compares the AArch64 native JIT at base `04f07602dba76eb6704b8901c989c9c9c1f8b42a` and the guarded-return change. It is not a CPU benchmark or a production workload estimate. The existing WASM implementation and priced-mode interpreter were not changed. Earlier guarded-return attempts and their limitations are recorded in [EX109](../../experiments.md#ex109).

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

All remaining RETW.N helper calls in this sample trapped. Equal totals and console output do not prove full machine-state equivalence. The native-JIT differential tests cover both encodings, all three eligibility failures and underflow for increments 1–3; the opt-in priced path already uses the interpreter on native. Removing any one guard makes those tests fail. A clean release build comparison found the 68 selected normalized hot symbols identical; emitted return code is deliberately different.

## CPU comparison

Base `04f07602dba76eb6704b8901c989c9c9c1f8b42a`; candidate `63a7fe9fe074a6b368d8d3ac80b15161887b4360`. The run used rustc 1.99.0 on macOS AArch64 and measured `wait4` user CPU. The start load was 3.22; the run used one warmup pair followed by seven alternating pairs on the 30 emulated seconds S3 serial starter. All accepted runs matched core 0 / core 1 instruction counts of 54,872,125 / 98,195,791 and console SHA-256 `0b06c16529e781f46bbdf92be4880ebe51f9d9210691f26ce871019404e959f7`.

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
