# EX221: short cycle slices and quad PSRAM

Base: upstream/main `017af524`. The final source is the commit containing this receipt.
Related scheduling mechanisms: EX047, EX133 and EX177. This change bounds a final
partial round; it does not change the default quantum or price instructions.
The command path reuses SpiMem and its existing dirty-memory reporting.

The ordinary scheduler clips the final round to max_cycles. Virtual and batched
rounds admit only whole quanta that fit. The synthetic spin program `06 ff ff`
runs with one or two busy cores, batching enabled/disabled, and successive
1, 255, 256, 257, 1025 and 1-cycle budgets at quantum 256.

Quad PSRAM returns three manufacturer/KGD/density bytes for the configured
2/4/8 MiB capacity on CS1 command 0x9f. The density match returns the final byte
directly; no extra ID bytes are synthesized.
Absent and unsupported capacities return an invalid ID. Quad read/write aliases
reuse the octal transfer path; octal mode-register and CS0 flash IDs remain unchanged.
No firmware fixture or golden was added or regenerated.

## Public definitions

- ESP-IDF v5.5.5 `components/esp_psram/device/esp_quad_psram_defs_ap.h`,
  lines 21-60: commands, manufacturer 0x0d, KGD 0x5d, density bits.
- ESP-IDF v4.4.8 `components/esp_hw_support/port/esp32s3/spiram_psram.c`,
  lines 46-83: the same commands, KGD and density encoding. Arduino-ESP32 2.x
  uses IDF 4.4; 3.x uses IDF 5.x. No relevant encoding difference.
- ESP-IDF v5.5.5 `components/soc/esp32s3/register/soc/spi_mem_reg.h`,
  lines 110-113, 383-391 and 560-570: USR, MISO/MOSI and CS disable bits.

Revision bits and an absent chip returning 0xff are inferred
model choices, not hardware measurements. Octal density reporting is unchanged.
No speed, RF or hardware-timing claim. No new fields or per-tick callbacks;
the scheduler adds an inline bound at the existing round boundary.

## CPU comparison

The maintainer measured S3 hello at +0.99% user CPU, slower in 8/9 alternating
pairs against main `017af524`, with identical instruction counts and console hashes.
Main median 25.562 s, range 25.419–26.177; candidate median 25.814 s,
range 25.663–26.339. C3 was +0.38%, slower in 4/9 pairs. This motivated
guarding the ordinary-round ceiling with `max_cycles != u64::MAX`, so unlimited
runs skip the cycle read, subtraction and minimum. No fields or tick work added.

Re-measured after the fix, against main `954f2a68`:

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 954f2a68 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured on `28691051acb3cfde702a06bb03d532ede2767786`. User CPU seconds.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| S3 hello | 26.8405 (26.4767–27.0191) | 26.5880 (25.9938–26.9224) | -0.94% | 2/7 | 1789819657 | 4.97 |
| C3 hello | 2.5681 (2.5540–2.5906) | 2.5680 (2.5487–2.6237) | -0.00% | 3/7 | 4800000000 | 4.74 |

#### S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 26.599303 | 26.651071 | 4.30/4.23/4.52 | 4.41/4.30/4.50 | accepted |
| 1 | 2 | M→B | 27.019115 | 26.598011 | 4.23/3.59/4.23 | 3.59/4.16/4.16 | accepted |
| 2 | 3 | B→M | 26.562476 | 25.993819 | 4.20/3.91/4.26 | 4.16/4.20/4.44 | accepted |
| 3 | 4 | M→B | 26.888596 | 26.828689 | 3.91/3.79/3.91 | 3.79/3.84/4.00 | accepted |
| 4 | 5 | B→M | 26.886297 | 26.922369 | 3.81/4.30/4.42 | 3.84/3.81/4.01 | accepted |
| 5 | 6 | M→B | 26.576830 | 26.587973 | 4.30/4.43/4.71 | 4.43/4.33/4.50 | accepted |
| 6 | 7 | B→M | 26.840546 | 26.419816 | 3.58/3.92/4.01 | 4.33/3.58/4.33 | accepted |
| 7 | 8 | M→B | 26.476750 | 26.357085 | 3.92/3.88/4.25 | 3.88/4.74/4.97 | accepted |

#### C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.623850 | 2.602977 | 4.74/4.44/4.74 | 4.74/4.74/4.74 | accepted |
| 1 | 2 | M→B | 2.574995 | 2.583375 | 4.44/4.44/4.44 | 4.44/4.56/4.56 | accepted |
| 2 | 3 | B→M | 2.557550 | 2.548675 | 4.56/4.52/4.56 | 4.56/4.56/4.56 | accepted |
| 3 | 4 | M→B | 2.559403 | 2.603012 | 4.52/4.52/4.52 | 4.52/4.64/4.64 | accepted |
| 4 | 5 | B→M | 2.554022 | 2.623748 | 4.64/4.43/4.64 | 4.64/4.64/4.64 | accepted |
| 5 | 6 | M→B | 2.586571 | 2.563727 | 4.43/4.43/4.43 | 4.43/4.23/4.43 | accepted |
| 6 | 7 | B→M | 2.590579 | 2.559384 | 4.23/4.05/4.23 | 4.23/4.23/4.23 | accepted |
| 7 | 8 | M→B | 2.568072 | 2.567971 | 4.05/4.05/4.05 | 4.05/4.05/4.05 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above. Raw output files are preserved.

Earlier measurements, before the finite-limit guard, against main `017af524`, with the same Rust 1.99.0 release build settings for both.

Pinned Rust 1.99.0 release binaries and inputs reused from the original run; main 017af524 built once. Child user CPU seconds from getrusage, startup included. One warmup pair B→M, then seven measured pairs M→B, B→M, alternating. Before EVERY pair attempt, require 1-minute load <3, polling every 15 seconds. Sample load every second during both runs and before/after each run; discard the whole pair if any observed load >4 and retry the same pair/order. Warmup is subject to the same rules. Medians/ranges exclude discarded attempts and warmup. Exact console hashes and per-core counts required, except #197 Pocket Tank permits only main 10073833775 vs PR 10073833665 instructions and the verified +55 main bus-cycle overshoot. Positive change means more CPU time.

Times are user CPU seconds. 

| Workload | Main median (range) | PR median (range) | Median change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| S3 hello | 25.1385 (24.8589–25.7082) | 25.0177 (24.6883–25.9181) | -0.48% | 3/7 | 1789819657 | 3.535 |
| C3 hello | 2.4892 (2.4702–2.4988) | 2.4792 (2.4548–2.5007) | -0.40% | 3/7 | 4800000000 | 2.299 |
| C6 hello | 3.2992 (3.2793–3.3219) | 3.2920 (3.2703–3.3685) | -0.22% | 2/7 | 4800000000 | 2.261 |
| Pocket Tank | 36.0558 (35.7450–36.5261) | 35.9263 (35.8176–36.0751) | -0.36% | 2/7 | 10073833775 → 10073833665 (−110) | 2.442 |

### S3 hello

Status: PASS. All attempts, including discarded attempts, follow.

| Pair | Attempt | Order | Disposition | Main user s | PR user s | Main load before/after | PR load before/after | Gate load | max load |
| --- | ---: | --- | --- | ---: | ---: | --- | --- | ---: | ---: |
| warmup | 1 | B → M | accepted warmup | 25.048567 | 24.980180 | 2.237/2.148 | 2.515/2.237 | 2.515 | 2.515 |
| 1 | 2 | M → B | accepted | 25.247724 | 24.742308 | 2.148/2.151 | 2.151/2.252 | 2.148 | 2.274 |
| 2 | 3 | B → M | accepted | 24.858884 | 24.821634 | 1.972/1.959 | 2.252/1.972 | 2.252 | 2.252 |
| 3 | 4 | M → B | accepted | 25.138484 | 24.688331 | 1.959/2.313 | 2.313/2.249 | 1.959 | 2.506 |
| 4 | 5 | B → M | accepted | 25.084567 | 25.171410 | 2.223/1.867 | 2.249/2.223 | 2.249 | 2.446 |
| 5 | 6 | M → B | accepted | 24.985323 | 25.387411 | 1.867/1.752 | 1.752/2.531 | 1.867 | 2.531 |
| 6 | 7 | B → M | accepted | 25.708211 | 25.918086 | 3.407/3.024 | 2.531/3.407 | 2.531 | 3.535 |
| 7 | 8 | M → B | accepted | 25.289520 | 25.017690 | 2.501/2.534 | 2.534/2.282 | 2.501 | 2.541 |

### C3 hello

Status: PASS. All attempts, including discarded attempts, follow.

| Pair | Attempt | Order | Disposition | Main user s | PR user s | Main load before/after | PR load before/after | Gate load | max load |
| --- | ---: | --- | --- | ---: | ---: | --- | --- | ---: | ---: |
| warmup | 1 | B → M | accepted warmup | 2.467299 | 2.474335 | 2.259/2.259 | 2.282/2.259 | 2.282 | 2.282 |
| 1 | 2 | M → B | accepted | 2.494967 | 2.498681 | 2.259/2.238 | 2.238/2.238 | 2.259 | 2.259 |
| 2 | 3 | B → M | accepted | 2.470184 | 2.479166 | 2.299/2.299 | 2.238/2.299 | 2.238 | 2.299 |
| 3 | 4 | M → B | accepted | 2.489224 | 2.460559 | 2.299/2.194 | 2.194/2.194 | 2.299 | 2.299 |
| 4 | 5 | B → M | accepted | 2.479471 | 2.454755 | 2.179/2.179 | 2.194/2.179 | 2.194 | 2.194 |
| 5 | 6 | M → B | accepted | 2.476631 | 2.469558 | 2.179/2.084 | 2.084/2.084 | 2.179 | 2.179 |
| 6 | 7 | B → M | accepted | 2.498762 | 2.494937 | 1.997/1.997 | 2.084/1.997 | 2.084 | 2.084 |
| 7 | 8 | M → B | accepted | 2.498193 | 2.500739 | 1.997/1.917 | 1.917/1.917 | 1.997 | 1.997 |

### C6 hello

Status: PASS. All attempts, including discarded attempts, follow.

| Pair | Attempt | Order | Disposition | Main user s | PR user s | Main load before/after | PR load before/after | Gate load | max load |
| --- | ---: | --- | --- | ---: | ---: | --- | --- | ---: | ---: |
| warmup | 1 | B → M | accepted warmup | 3.229875 | 3.265443 | 1.923/1.923 | 1.917/1.923 | 1.917 | 1.923 |
| 1 | 2 | M → B | accepted | 3.321858 | 3.289935 | 1.923/2.089 | 2.089/2.162 | 1.923 | 2.162 |
| 2 | 3 | B → M | accepted | 3.279332 | 3.270312 | 2.162/2.069 | 2.162/2.162 | 2.162 | 2.162 |
| 3 | 4 | M → B | accepted | 3.320524 | 3.313870 | 2.069/2.063 | 2.063/2.063 | 2.069 | 2.069 |
| 4 | 5 | B → M | accepted | 3.311121 | 3.368531 | 2.138/2.046 | 2.063/2.138 | 2.063 | 2.138 |
| 5 | 6 | M → B | accepted | 3.288005 | 3.344395 | 2.046/2.046 | 2.046/2.123 | 2.046 | 2.123 |
| 6 | 7 | B → M | accepted | 3.299159 | 3.291991 | 2.112/2.112 | 2.123/2.112 | 2.123 | 2.123 |
| 7 | 8 | M → B | accepted | 3.284244 | 3.283638 | 2.112/2.023 | 2.023/2.261 | 2.112 | 2.261 |

### Pocket Tank

Status: PASS. All attempts, including discarded attempts, follow.

| Pair | Attempt | Order | Disposition | Main user s | PR user s | Main load before/after | PR load before/after | Gate load | max load |
| --- | ---: | --- | --- | ---: | ---: | --- | --- | ---: | ---: |
| warmup | 1 | B → M | accepted warmup | 35.982467 | 35.608799 | 2.180/1.838 | 2.261/2.180 | 2.261 | 2.442 |
| 1 | 2 | M → B | accepted | 36.526081 | 35.926337 | 1.838/2.182 | 2.182/2.041 | 1.838 | 2.289 |
| 2 | 3 | B → M | accepted | 35.744978 | 35.892115 | 1.782/1.843 | 2.041/1.782 | 2.041 | 2.188 |
| 3 | 4 | M → B | accepted | 35.882530 | 35.879235 | 1.843/1.701 | 1.701/1.760 | 1.843 | 1.877 |
| 4 | 5 | B → M | accepted | 36.190938 | 35.973117 | 1.502/1.889 | 1.760/1.502 | 1.760 | 1.889 |
| 5 | 6 | M → B | accepted | 36.055776 | 35.972720 | 1.889/2.262 | 2.262/1.754 | 1.889 | 2.285 |
| 6 | 7 | B → M | accepted | 35.981023 | 36.075125 | 2.151/2.009 | 1.754/2.151 | 1.754 | 2.202 |
| 7 | 8 | M → B | accepted | 36.477184 | 35.817626 | 2.009/1.692 | 1.692/1.712 | 2.009 | 2.009 |

### Pocket Tank cycle-limit exception

S3 `CPU_HZ = 240_000_000` (`esp32s3/src/periph.rs:46`); CLI sets `max_cycles = seconds × CPU_HZ` (`cli/src/lib.rs:487`). `Machine::seconds` divides bus cycles by CPU_HZ (`esp-soc/src/machine.rs:165`). Native quantum is 64 cycles (`esp-soc/src/machine.rs:132`).

30 × 240,000,000 = 7,200,000,000 cycles.

main:
```text
[emu] stop: Halted — core0 4446180089 + core1 5627653686 insns in 35.7s wall = 282.1 Minsn/s; emulated 30.000s (7200000055 cycles); 1006391 exceptions, 95179 interrupts
```

candidate:
```text
[emu] stop: Halted — core0 4446180034 + core1 5627653631 insns in 37.1s wall = 271.2 Minsn/s; emulated 30.000s (7200000000 cycles); 1006391 exceptions, 95179 interrupts
```

The PR stops at 7,200,000,000 cycles, exactly 30 seconds. Main stops at 7,200,000,055 cycles, or 30.0000002291667 seconds. The printed 30.000s rounds away the 55-cycle overshoot. Main executes 55 extra instructions on each core, 110 total, with the same console SHA-256 `c3776029dd98a1cd1ad148aa3cfdbd6639a49e465157214c2f1ca34ac5e2bbd5`.

The change against `017af524` clamps the final quantum to remaining cycles and rounds virtual/busy batching down rather than up. Main executes the untrimmed final batch; the PR trims its tail. This is the EX221 final-round limit fix, not a firmware/output change. The comparison permits exactly these pinned per-arm totals/cycles, with no other count tolerance. Raw stop lines come from the prior actual runs; binary/input and raw-output hashes were rechecked before batch 1, and every new attempt must reproduce these values.

### Why Pocket Tank's instruction count differs

30 × 240,000,000 = 7,200,000,000 cycles.

main:
```text
[emu] stop: Halted — core0 4446180089 + core1 5627653686 insns in 35.7s wall = 282.1 Minsn/s; emulated 30.000s (7200000055 cycles); 1006391 exceptions, 95179 interrupts
```

candidate:
```text
[emu] stop: Halted — core0 4446180034 + core1 5627653631 insns in 37.1s wall = 271.2 Minsn/s; emulated 30.000s (7200000000 cycles); 1006391 exceptions, 95179 interrupts
```

The PR stops at 7,200,000,000 cycles, exactly 30 seconds. Main stops at 7,200,000,055 cycles, or 30.0000002291667 seconds. The printed 30.000s rounds away the 55-cycle overshoot. Main executes 55 extra instructions on each core, 110 total, with the same console SHA-256 `c3776029dd98a1cd1ad148aa3cfdbd6639a49e465157214c2f1ca34ac5e2bbd5`.

The change against `017af524` clamps the final quantum to remaining cycles and rounds virtual/busy batching down rather than up. Main executes the untrimmed final batch; the PR trims its tail. This is the EX221 final-round limit fix, not a firmware/output change. The comparison permits exactly these pinned per-arm totals/cycles, with no other count tolerance.

## Verification

Darwin arm64, cargo 1.99.0, Node v22.23.1. [Commands and exit codes](checks.json)
and [additional CI checks](extra-checks.json) all pass. Fetch inputs with
`tools/fetch-demo-assets.sh --no-linux`; [SHA-256 hashes](inputs.json) identify
ROMs and public demo assets. No private captures or machine identifiers retained.

Workspace checks use an empty HOME, with CARGO_HOME and RUSTUP_HOME pointing to
the installed toolchain/cache. All ESP32SIM_* variables are removed first.
For CI policy only, set `ESP32SIM_ROM_DIR="$PWD/web/wasm/fw"`.
CI-policy tests: 643 passed. Plain tests, no ESP32SIM_* variables: 622 passed,
35 ignored. Both native and WASM Clippy deny warnings. All eight WASM demos pass.
Existing golden files are byte-identical to the base. No JIT code changed.

Focused batching check:
`ESP32SIM_VQ_NATIVE=1 cargo +1.99.0 test --release -p esp32s3 --test machine busy_runs_honor_cycle_ceiling_with_virtual_and_batched_rounds`.
The test asserts that the virtual/batched paths actually ran when enabled.

## Mutation table

All 13 mutations were rerun with the finite-limit guard. Each mutation was applied
alone, compiled successfully and failed the named test. The unlimited fast path
is checked structurally: the cycle read and ceiling arithmetic occur only inside
`max_cycles != u64::MAX`. Runtime cost awaits central remeasurement.
Restore the rule between runs. Scheduler mutations use the focused command above;
PSRAM mutations use `cargo +1.99.0 test --release -p esp-periph --lib cs1_quad_id_tracks_capacity_and_keeps_octal_and_flash_ids`.

| Mutation | Test that kills it |
| --- | --- |
| Remove ordinary cycle bound | `busy_runs_honor_cycle_ceiling_with_virtual_and_batched_rounds` |
| Round virtual cycle bound up with div_ceil | `busy_runs_honor_cycle_ceiling_with_virtual_and_batched_rounds` |
| Round batched cycle bound up with div_ceil | `busy_runs_honor_cycle_ceiling_with_virtual_and_batched_rounds` |
| Disable the absent-PSRAM early return | `cs1_quad_id_tracks_capacity_and_keeps_octal_and_flash_ids` |
| Change quad KGD byte from 0x5d to zero | `cs1_quad_id_tracks_capacity_and_keeps_octal_and_flash_ids` |
| Return zero for 0x200000 density | `cs1_quad_id_tracks_capacity_and_keeps_octal_and_flash_ids` |
| Return zero for 0x400000 density | `cs1_quad_id_tracks_capacity_and_keeps_octal_and_flash_ids` |
| Return zero for 0x800000 density | `cs1_quad_id_tracks_capacity_and_keeps_octal_and_flash_ids` |
| Remove 0x02 transfer alias | `cs1_quad_id_tracks_capacity_and_keeps_octal_and_flash_ids` |
| Remove 0x38 transfer alias | `cs1_quad_id_tracks_capacity_and_keeps_octal_and_flash_ids` |
| Remove 0x03 transfer alias | `cs1_quad_id_tracks_capacity_and_keeps_octal_and_flash_ids` |
| Remove 0x0b transfer alias | `cs1_quad_id_tracks_capacity_and_keeps_octal_and_flash_ids` |
| Remove 0xeb transfer alias | `cs1_quad_id_tracks_capacity_and_keeps_octal_and_flash_ids` |
