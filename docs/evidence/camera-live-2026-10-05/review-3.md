# EX212 inline camera progress

The adopted code uses the inline u128 expression and has no non-inlined helper.
The candidate is the commit containing this receipt, on parent
`68068d9838fd6761106619f9ff74666d49b7f4fa`. The base is
`cbb9edf607a09be78cb199c49c2e4cc0bd27d0dc`. Source hashes are in `review-3.json`.
This continues EX212's layout comparison with the inline form adopted on the
maintainer's evidence. Both defensive guards remain, with belt-and-braces comments.

The maintainer reported the following M5 Max results in [PR #190](https://github.com/mikroverk/esp32sim/pull/190). All instruction
counts and console hashes matched. The helper was slower in every pair, with
separated ranges. These are reviewer-reported measurements, not local runs.

| Variant versus main | S3 hello | Pocket Tank |
| --- | --- | --- |
| Helper at `68068d98` | +2.6%, 9 pairs | +6.3%, 7 pairs |
| `68068d98` with `review-2-inline.patch` | −1.0%, 9 pairs | −0.2%, 7 pairs |

The [earlier M5 Pro campaigns](review-2.md) remain evidence of a different layout
result: inline Pocket Tank +2.8%, helper +0.9%. They do not establish a portable
advantage for the helper. The new local comparison below tests the adopted inline
code directly. No additional layout tuning is inferred from a small median delta.

Reproduce using the existing `speed.py`: one warmup pair, seven alternating pairs,
child user CPU, S3 hello_world for 3000 emulated seconds on `--board none`, and
Pocket Tank for 30 seconds on its manifest's AMOLED board. Both executables use
`cargo +1.99.0 build --release --bins`. The current-main executable hashes match
the earlier receipt. No builds or tests run during measurement. Host: Apple M5
Pro, macOS 27.0.1 arm64, Rust 1.99.0. Every pair must match both core instruction
counts and console hashes.

```sh
python3 docs/evidence/camera-live-2026-10-05/speed.py "$MAIN_BIN" target/release \
  --base-revision cbb9edf607a09be78cb199c49c2e4cc0bd27d0dc \
  --candidate-revision 'commit containing review-3.md; source hashes in review-3.json' \
  --workloads hello pocket-tank --output "$OUTPUT"
```

The full validation commands remain in [README.md](README.md). The camera CLI
check additionally pins the specific size diagnostics for `12`, `0x0`, `+2x+2`
and `4096x4096`, and rejects an unpaired stream before the Cooja handshake.

User CPU seconds; seven alternating pairs after warmup.

| Workload | Main median and range | Inline median and range | Median change | Slower pairs |
| --- | ---: | ---: | ---: | ---: |
| S3 hello | 27.663, 27.087 to 28.216 | 27.563, 27.034 to 27.957 | -0.4% | 4/7 |
| Pocket Tank | 39.642, 39.277 to 40.027 | 39.840, 39.415 to 40.189 | +0.5% | 3/7 |

Each cell below is main / inline. I = inline candidate, M = main. Warmup is excluded from medians.

| Pair | Order | S3 hello | Pocket Tank |
| --- | --- | ---: | ---: |
| Warmup | I→M | 27.467 / 27.105 | 43.181 / 40.426 |
| 1 | M→I | 28.216 / 27.160 | 40.027 / 39.624 |
| 2 | I→M | 27.663 / 27.034 | 39.642 / 40.189 |
| 3 | M→I | 27.435 / 27.450 | 39.519 / 39.951 |
| 4 | I→M | 27.453 / 27.609 | 39.496 / 39.467 |
| 5 | M→I | 27.950 / 27.957 | 39.974 / 39.840 |
| 6 | I→M | 27.932 / 27.563 | 39.767 / 39.415 |
| 7 | M→I | 27.087 / 27.903 | 39.277 / 40.037 |

Both core instruction counts and console hashes match in every run. S3 hello totals 1,789,819,657 instructions; Pocket Tank totals 10,073,833,775. These samples do not establish sub-percent precision.

[Raw samples, commands and executable/input hashes](review-3-speed.json).

The new M5 Pro inline result is −0.4% for S3 hello and +0.5% for Pocket Tank. Ranges overlap, and individual pairs differ in both directions, so these medians do not establish sub-percent costs. The earlier inline Pocket Tank +2.8% remains a negative result for that build and campaign; it did not recur here. Layout outcomes differ between machines and builds. The M5 Max helper regression rules out treating the helper as a portable improvement; the inline form is adopted.

Both Clippy gates pass with warnings denied. Release workspace tests pass 619 with ignored tests included and 14 external-input tests filtered; plain release passes 601 with 32 ignored. The production WASM build and all eight requested workloads pass. The CLI diagnostic and pre-handshake checks pass. Goldens are unchanged. No JIT implementation changed.
