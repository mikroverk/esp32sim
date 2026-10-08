# EX218: opt-in GPIO waveform decoding

Base: upstream main `954f2a68` (includes #198). Related EX205 supplies the existing GPIO pin
transport. EX218 adds a WS2812 decoder and instruction-sized scheduling for boards
that opt in. It uses `gpio_output_at` and the existing GRB decoder.

## Contract and quantum-1 evidence

`BoardModel::uses_gpio_waveform` defaults to false. For an attached waveform board,
`Machine::run` uses unmodeled quantum 1; timing models are incompatible.
`run_until_cycle` limits each budget to one instruction. The configured quantum
is preserved. Browser external blocks are refused for waveform boards.

The shared `tests/gpio_waveform.rs` program sends three GRB pixels and checks RGB
`[7,11,13]`, `[17,19,23]`, `[29,31,37]`, 145 ordered edges, and each of the 72 high
pulse widths against its transmitted bit, plus 800 ns inter-bit low periods. S3 widths are 96/192 cycles at 240 MHz;
C3/C6 widths are 64/128 cycles at 160 MHz. Setting quantum 1 alone passes these checks with main's CPU/JIT/bus execution
paths and `gpio_output_at`. Native AArch64 execution with JIT enabled retired compiled
instructions. WASM with JIT enabled passed through its existing interpreter
fallback at this budget (zero compiled modules in the focused quantum-1 run).
No JIT helper, store, instruction-position or bus-clock changes are needed.

The final shared regression runs requested quanta 1, 64, 256 and 1024, S3 cores
0 and 1 with JIT enabled/disabled, C3 and C6. Requested quantum 1024 on core 0
uses `run_until_cycle`; the other cases use `run`. It also checks restoration,
the browser guard, and ordinary quantum 64 with `NoBoard`. The CLI test and the
WASM JIT suite execute the same source. Native compiled-instruction assertions
apply only on AArch64 Linux/macOS, where that backend is available.

## Idle path and integration

The CPU, native/WASM JIT, bus structs, GPIO write paths and device ticks are
byte-identical to main. The scheduler checks board opt-in at public execution
entry points. No per-store, per-instruction or per-helper work is added. Structural check:

```sh
git diff 954f2a68 -- emu-core/src riscv-rv32/src xtensa-lx7/src esp32s3/src/bus.rs esp32c3/src/bus.rs esp32c6/src/bus.rs
```

Result: empty output.

External board models opt in, construct `Ws2812Chain::new(n).with_gpio_clock(hz)`,
feed `gpio_output_at` into `gpio_drive`, publish `gpio_deadline` through
`next_deadline`, and call `advance_gpio` from `advance_to`. The shared CLI/WASM
regression is a concrete board adapter; no built-in board enables this decoder.
The frequency is stored once on the chain. Existing GPIO drive masks supply
output/enable transitions; there is no additional pin-routing mechanism.

## Inputs and verification

`inputs.json` pins the synthetic program source and fetched ROMs. Assets are
obtained with `tools/fetch-demo-assets.sh --no-linux`. No external firmware build
is required. `checks.json` records final command exit statuses with Rust 1.99.0.
Both workspace test invocations start with empty HOME; the ignored-inclusive
run sets only ESP32SIM_ROM_DIR among emulator variables, and the plain run sets
none. CARGO_HOME and RUSTUP_HOME retain access to the pinned toolchain.
Existing goldens are unchanged. `tools/wasm-jit-test.sh` runs the full differential
suite and the shared waveform suite in separate Node processes, bounding retained
generated-module metadata. Node v22.23.1 uses its default heap; no heap override
is part of the required check. The final script passes 111,125 differential cases (99,677 compiled modules
released) and 24 waveform cases (738 compiled modules released). The baseline
differential suite at `017af524` has the same 111,125/99,677 counts.

`mutations.json` records each mutation, exact edit, command and killing result.
The 18 retained rules cover pulse windows, reset deadline, output enable, bounded
storage, invalid frames, repeated levels, nonzero clock, both scheduling entry
points, quantum restoration, browser bypass, opt-in, both timing configuration
APIs and both execution guards for a board installed after timing configuration. Each mutation is applied
alone and restored. Deleted position-plumbing and duplicate route rules have no
remaining implementation to mutate. Every recorded mutation is killed.

## Limits

High pulses 150–1100 ns, a 550 ns bit split, minimum 150 ns low and 50 us reset
are decoder policy for WS2812-class signals, not hardware calibration. Maximum
non-reset low width is unconstrained. Invalid pulses discard a frame; incomplete
pixels do not update it. Storage is bounded by strip size. Simultaneous waveform
writers on both S3 cores and physical matrix inversion are not modeled by this
adapter. Quantum 1 trades execution throughput for exact instruction-cycle GPIO
timing only while opted in. `set_cost_model` and `set_approximate_jit_timing`
reject an attached waveform board with an explicit error before changing timing
state. A board installed later through the public bus field causes `run` and
`run_until_cycle` to return an attachment-lifecycle error before any instruction
executes. The configured model and quantum remain intact. The shared native/WASM
test checks both configuration orders for both timing APIs, zero executed
instructions/cycles, and unchanged model/quantum state. Supporting priced GPIO
waveforms requires a separate timing contract. No CPU benchmark was run.

## CPU comparison

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 954f2a68 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured on `5818acf3a22170370b60f6dd3b6c1933f62d4391` against main `954f2a68`. User CPU seconds.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| S3 hello | 25.5241 (25.1574–26.3508) | 24.6417 (24.4775–25.1007) | -3.46% | 0/7 | 1789819657 | 4.32 |
| C3 hello | 2.4787 (2.4612–2.5245) | 2.3646 (2.3461–2.4229) | -4.61% | 0/7 | 4800000000 | 4.38 |
| C6 hello | 3.2529 (3.2433–3.3134) | 3.1862 (3.1728–3.2190) | -2.05% | 0/7 | 4800000000 | 4.36 |
| Pocket Tank | 37.2218 (35.5897–38.3215) | 36.1742 (35.5047–38.2153) | -2.81% | 1/7 | 10073833775 | 5.02 |

### S3 hello

Status: PASS. Flags: ranges do not overlap.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 26.044847 | 25.590601 | 4.31/3.51/4.31 | 4.32/4.31/4.32 | accepted |
| 1 | 2 | M→B | 26.350844 | 25.100745 | 3.51/3.46/3.63 | 3.46/3.03/3.46 | accepted |
| 2 | 3 | B→M | 26.246720 | 24.949363 | 3.34/2.69/3.34 | 3.03/3.34/3.46 | accepted |
| 3 | 4 | M→B | 25.524139 | 24.477487 | 2.69/2.87/3.21 | 2.87/2.30/2.87 | accepted |
| 4 | 5 | B→M | 25.524617 | 24.500143 | 2.78/3.32/3.32 | 2.30/2.78/2.78 | accepted |
| 5 | 6 | M→B | 25.348757 | 24.719130 | 3.32/3.06/3.32 | 3.06/3.59/3.59 | accepted |
| 6 | 7 | B→M | 25.466152 | 24.616450 | 3.50/4.07/4.16 | 3.59/3.50/3.59 | accepted |
| 7 | 8 | M→B | 25.157369 | 24.641709 | 4.07/3.71/4.07 | 3.71/3.67/3.74 | accepted |

### C3 hello

Status: PASS. Flags: ranges do not overlap.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.480961 | 2.395583 | 3.67/3.78/3.78 | 3.67/3.67/3.67 | accepted |
| 1 | 2 | M→B | 2.497100 | 2.348899 | 3.78/3.78/3.78 | 3.78/3.88/3.88 | accepted |
| 2 | 3 | B→M | 2.524467 | 2.385395 | 3.88/3.81/3.88 | 3.88/3.88/3.88 | accepted |
| 3 | 4 | M→B | 2.476307 | 2.356060 | 3.81/3.81/3.81 | 3.81/3.98/3.98 | accepted |
| 4 | 5 | B→M | 2.472633 | 2.346131 | 3.98/4.38/4.38 | 3.98/3.98/3.98 | accepted |
| 5 | 6 | M→B | 2.481296 | 2.364556 | 4.38/4.38/4.38 | 4.38/4.27/4.38 | accepted |
| 6 | 7 | B→M | 2.461157 | 2.422904 | 4.27/4.17/4.27 | 4.27/4.27/4.27 | accepted |
| 7 | 8 | M→B | 2.478750 | 2.372580 | 4.17/4.17/4.17 | 4.17/4.08/4.17 | accepted |

### C6 hello

Status: PASS. Flags: ranges do not overlap.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.339379 | 3.117786 | 4.08/4.15/4.15 | 4.08/4.08/4.08 | accepted |
| 1 | 2 | M→B | 3.250247 | 3.172780 | 4.15/4.06/4.15 | 4.06/4.06/4.06 | accepted |
| 2 | 3 | B→M | 3.313431 | 3.218958 | 4.05/4.13/4.13 | 4.06/4.05/4.06 | accepted |
| 3 | 4 | M→B | 3.299768 | 3.186188 | 4.13/4.13/4.13 | 4.13/4.04/4.13 | accepted |
| 4 | 5 | B→M | 3.243281 | 3.195484 | 4.35/4.35/4.35 | 4.04/4.35/4.35 | accepted |
| 5 | 6 | M→B | 3.252882 | 3.214530 | 4.35/4.25/4.35 | 4.25/4.25/4.25 | accepted |
| 6 | 7 | B→M | 3.244073 | 3.174259 | 4.23/4.13/4.23 | 4.25/4.23/4.25 | accepted |
| 7 | 8 | M→B | 3.265642 | 3.178179 | 4.13/4.13/4.13 | 4.13/4.36/4.36 | accepted |

### Pocket Tank

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 35.622958 | 35.808734 | 4.07/3.68/4.07 | 4.36/4.07/4.41 | accepted |
| 1 | 2 | M→B | 35.589673 | 35.504695 | 3.68/3.45/3.68 | 3.45/3.03/3.45 | accepted |
| 2 | 3 | B→M | 37.180785 | 36.174180 | 3.55/3.43/3.58 | 3.03/3.55/3.55 | accepted |
| 3 | 4 | M→B | 35.775990 | 35.724871 | 3.43/3.29/3.72 | 3.29/2.91/3.29 | accepted |
| 4 | 5 | B→M | 37.890182 | 35.681037 | 3.11/3.38/3.57 | 2.91/3.11/3.11 | accepted |
| 5 | 6 | M→B | 38.321456 | 38.215254 | 3.38/3.75/3.85 | 3.75/4.44/4.66 | accepted |
| 6 | 7 | B→M | 37.221784 | 37.731235 | 4.80/4.26/5.02 | 4.44/4.80/4.80 | accepted |
| 7 | 8 | M→B | 37.428915 | 36.750171 | 4.26/4.93/4.93 | 4.93/4.62/5.02 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above. Raw output files are preserved.

The PR was faster on every workload here (S3 hello −3.46%, C3 −4.61%, C6 −2.05%, Pocket Tank −2.81%) with identical instruction counts. No board in the tree opts into waveform mode, so the only change on these runs is one `board_ref()` check per `run()` call; the difference is most likely code layout, and no speedup is claimed.

## Evidence privacy

Only public source paths, input hashes, commands and correctness results are
retained. No raw captures, local filesystem identities or session logs are used.
