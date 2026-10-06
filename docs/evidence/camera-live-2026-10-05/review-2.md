# EX212 receive completion and bounded camera progress

The historical helper candidate is `68068d9838fd6761106619f9ff74666d49b7f4fa`, on parent
`fb97f45059d9aa4e100629124a1c7ffc2a4edd5d`. Main remains
`cbb9edf607a09be78cb199c49c2e4cc0bd27d0dc`.
The source hashes in `review-2.json` identify the measured files. This follows
EX212 with wider active-camera arithmetic and a corrected receive-completion
interrupt; the earlier idle-layout results remain in the main receipt.

DSCR_EMPTY means receive data remains without an inlink, per ESP-IDF v5.5.4
`components/soc/esp32s3/register/soc/gdma_reg.h`. An exactly fitting AES or camera
receive reports `0x3`; a short camera destination reports `0x11`. Progress uses
u128 and ordered slice bounds. The non-inlined helper was rejected after the
maintainer measured regressions on M5 Max; [inline adoption](review-3.md)
records the current decision. The 16 MB, 2^40-cycle reproduction failed before
the fix with slice start 14399999 greater than end 822783. Still-image validation
uses the same 8 MiB RGBA-equivalent limit as streaming.

Reproduce the tests with the commands in [the main receipt](README.md).
`cargo +1.99.0 test --release -p esp32s3 --lib dma_tests` includes the two interrupt
cases and the overflow reproduction. `node tools/camera-cli-test.mjs` checks
oversized still images, per-chip flag errors and a FIFO with no writer.
The public Arduino driver uses the same firmware and inputs recorded in the
[driver receipt](../camera-start-2026-10-02/README.md).

The speed comparison uses the existing `speed.py`, with `--workloads hello
pocket-tank`, one warmup pair and seven alternating pairs. S3 hello runs 3000
emulated seconds on board none; Pocket Tank runs 30. Both builds use
`cargo +1.99.0 build --release --bins`. The retained main executables match the
base hashes in `speed.json`. No builds or tests run during measurement. Host:
Apple M5 Pro, macOS 27.0.1 arm64, Rust 1.99.0. Results retain every round and
compare both core instruction counts and console hashes.

Validation: both Clippy gates pass with warnings denied; release workspace tests
pass 619 tests with ignored tests included and 14 external-input tests filtered;
plain release passes 601 with 32 ignored. Production WASM and all eight smoke
workloads pass. Goldens are unchanged. The public driver captures four RGB565
and three YUYV frames, including re-init and reboot, with unchanged hashes.
`review-2.json` records these checks and the new executable hash.

```sh
python3 docs/evidence/camera-live-2026-10-05/speed.py "$MAIN_BIN" target/release \
  --base-revision cbb9edf607a09be78cb199c49c2e4cc0bd27d0dc \
  --candidate-revision 68068d9838fd6761106619f9ff74666d49b7f4fa \
  --workloads hello pocket-tank --output "$OUTPUT"
```

User CPU seconds, seven pairs after warmup.

| Workload | Main median and range | Candidate median and range | Median change |
| --- | ---: | ---: | ---: |
| S3 hello | 27.034, 25.524 to 27.890 | 26.755, 25.705 to 27.476 | -1.0% |
| Pocket Tank | 38.295, 37.110 to 39.026 | 38.641, 38.031 to 40.279 | +0.9% |

Each cell below is main / candidate. B = candidate, M = main. Warmup is excluded from medians.

| Pair | Order | S3 hello | Pocket Tank |
| --- | --- | ---: | ---: |
| Warmup | B→M | 26.062 / 26.384 | 38.232 / 38.544 |
| 1 | M→B | 25.524 / 25.705 | 38.295 / 38.712 |
| 2 | B→M | 26.438 / 25.750 | 38.669 / 38.031 |
| 3 | M→B | 26.910 / 26.709 | 39.026 / 38.641 |
| 4 | B→M | 27.034 / 26.755 | 38.238 / 40.279 |
| 5 | M→B | 27.890 / 27.476 | 38.214 / 39.255 |
| 6 | B→M | 27.181 / 26.840 | 37.110 / 38.525 |
| 7 | M→B | 27.285 / 27.384 | 38.349 / 38.420 |

Both core instruction counts and console hashes match in every run. S3 hello totals 1,789,819,657 instructions; Pocket Tank totals 10,073,833,775. Ranges overlap. These samples do not resolve sub-percent costs or establish a precise speedup.

Raw samples, commands and executable/input hashes: [review-2-speed.json](review-2-speed.json).

## Inlined wide-division variant

The same u128 calculation inlined into the shared tick body measured S3 hello
−1.5% and Pocket Tank +2.8% by median on M5 Pro. This is a retained negative
result for that machine, not grounds to prefer the helper on other machines. Apply [review-2-inline.patch](review-2-inline.patch) with
`git apply --unidiff-zero` to `68068d9838fd6761106619f9ff74666d49b7f4fa` to reproduce the inlined source; its hashes and all samples are in
[review-2-speed-inline.json](review-2-speed-inline.json). Both campaigns used the
same base executable. Their paired comparisons have overlapping ranges; they
do not establish the helper's isolated effect independently of measurement noise.

User CPU seconds, seven pairs after warmup.

| Workload | Main median and range | Candidate median and range | Median change |
| --- | ---: | ---: | ---: |
| S3 hello | 26.025, 25.373 to 26.633 | 25.634, 25.311 to 26.182 | -1.5% |
| Pocket Tank | 36.777, 36.192 to 37.247 | 37.814, 36.069 to 38.322 | +2.8% |

Each cell below is main / candidate. B = candidate, M = main. Warmup is excluded from medians.

| Pair | Order | S3 hello | Pocket Tank |
| --- | --- | ---: | ---: |
| Warmup | B→M | 25.099 / 25.298 | 36.181 / 36.104 |
| 1 | M→B | 25.373 / 25.589 | 36.192 / 36.069 |
| 2 | B→M | 25.712 / 25.711 | 37.136 / 37.827 |
| 3 | M→B | 26.116 / 25.761 | 36.588 / 37.134 |
| 4 | B→M | 26.119 / 26.182 | 36.777 / 38.322 |
| 5 | M→B | 25.373 / 25.408 | 37.017 / 36.871 |
| 6 | B→M | 26.633 / 25.311 | 37.247 / 38.056 |
| 7 | M→B | 26.025 / 25.634 | 36.366 / 37.814 |

Both core instruction counts and console hashes match in every run. S3 hello totals 1,789,819,657 instructions; Pocket Tank totals 10,073,833,775. Ranges overlap. These samples do not resolve sub-percent costs or establish a precise speedup.
