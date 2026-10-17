# C6 source-change interrupt refresh (EX090 follow-up)

Base: `142805d5399e0c3cf25995fe21ba524e75a2b3cb`.

## Change and contract

C6 previously discarded `Dispatch::tick`'s existing source-change result and
returned 1 from every bus tick. That made `Machine::after_round` rebuild the
interrupt matrix and present interrupts even when no source or routing changed.
The bus now ORs that result into its existing `irq_dirty` flag and reports the
flag from `Bus::tick`. The shared dispatcher and scheduler are unchanged.

Every device still ticks on exactly the same rounds, with the same cycle counts,
clock remainders, deadlines and 64-cycle idle limit. No time is elided, no guest
instruction is skipped, no refresh is deferred across a source transition. This
is not EX134/EX157 tick-deferral widening, EX028 source caching or an instruction
cache. It is a C6-specific use of the existing EX090 invalidation contract.
No new fields, allocation, feature flag or shared per-chip work is introduced.

Invalidation audit:

- Clocked devices: the generated dispatcher already compares interrupt sources
  before/after tick, including optional devices and their cached source words.
- MMIO reads: the USB FIFO word (including byte/halfword lane aliases) now
  invalidates before dispatch, because draining it can present the next packet
  and raise RECV_PKT before tick's before/after comparison begins. The dirty bit
  is ORed, so unrelated subsequent reads cannot clear it. Other reads do not
  acquire blanket invalidation. See the read-side audit and regression below.
- MMIO writes: peripheral and CPU-subsystem writes retain their existing dirty flag,
  including source enables/clears, matrix routes, PLIC type/priority/threshold
  and software interrupt latches. Edge clears and remaps still refresh even
  if peripheral status itself is unchanged.
- DMA, radio receive, Wi-Fi and completed RMT: existing dirty flags remain.
  I2S RX's GDMA receive path already ORs its interrupt-change result into dirty.
- Board GPIO and UART callbacks: existing dirty flags remain.
- Host input: GPIO, UART, USB and radio input keep their existing invalidation;
  direct host mutation of public device fields still requires `sync_irq()`.
  Reset and initialization keep the existing machine synchronization.

An unchanged source and unchanged routing/controller configuration cannot change
arbitrated CPU input. Source detection occurs before interrupt refresh at the
same original scheduling boundary, preserving level and edge-latch semantics.

## Tests and rule removal

`esp32c6/tests/irq_tick.rs` contains three synthetic regressions. On the base,
the quiet-tick and unchanged-level tests fail (unconditional tick result 1),
and the timer-wakeup control passes. On the candidate all three pass. The
wakeup test starts a sleeping real RISC-V core and checks the interrupt cause,
wakeup, architectural retirement and execution of the handler.

Run:

    CARGO_TARGET_DIR="$OUT/target" cargo +1.99.0 test --release -p esp32c6 --test irq_tick
    CARGO_TARGET_DIR="$OUT/target" python3 docs/evidence/c6-irq-tick/mutate.py "$OUT/mutations"

Four mutations are killed by actual failing assertions, not compilation errors:

| Mutation | Killing test |
| --- | --- |
| Discard clocked-source invalidation | sleeping_core_wakes_when_the_timer_source_changes |
| Remove interrupt line refresh | sleeping_core_wakes_when_the_timer_source_changes |
| Restore unconditional refresh request | quiet_tick_does_not_request_an_interrupt_refresh |
| Remove peripheral-write invalidation | timer_tick_requests_refresh_at_assertion_not_on_unchanged_level |

`mutations.json` contains results; the script restores each source in a finally
block. The three tests pass again after mutation collection.

## Exact-work measurement method

Use the upstream `c6-hello` ROM/bootloader fixture (countdown/delay/restart) and
a permitted prebuilt public C6 display starter, board `waveshare-c6-lcd147`.
Input hashes and byte sizes are in `inputs.json`. The starter is not a CI fixture
or a new firmware build: callers supply the four matching prebuilt files. No
hardware, RF, shipped browser runtime or production-session reproduction is
claimed. The hello fixture is fetched with `tools/fetch-demo-assets.sh --no-linux`.

The CLI's normal reported instruction count includes idle work. To measure
actual architectural retirement, apply `retired-probe.patch` identically to
both builds. It prints `Cpu::retired_count` once after execution, outside the
measured emulator loop; the counter already exists and survives guest resets.
This instrumentation is NOT part of the product diff. Both original and
instrumented release binaries have all 106 selected hot functions identical
per the EX230 disassembly normalizer, on each arm separately. No counting code
was added to the hot path. C6 has one core.

Build each arm with the same toolchain and release configuration:

    git apply docs/evidence/c6-irq-tick/retired-probe.patch
    nice -n 10 env CARGO_TARGET_DIR="$OUT/target" cargo +1.99.0 build --release -p esp32sim --bin esp32sim-c6

Copy the two binaries separately, undo the probe patch, and run:

    python3 docs/evidence/c6-irq-tick/smoke.py "$OUT/smoke" "$BASE_PROBE" "$CANDIDATE_PROBE" "$ROM_DIR" "$PUBLIC_STARTER_DIR"

The harness runs three alternating pairs per workload, 30 guest seconds each,
records aggregate load and wall/user/system CPU time per sample, saves the full
stdout/stderr, and verifies the full console SHA256 and architectural retirement
are identical across all six runs of each workload. It refuses active cargo or
rustc builds before/after each sample. This is engineering smoke, not the
central quiet-machine 1-warmup/7-pair CPU comparison.

## Historical engineering smoke result (before read-side correction)

These samples measure `a41db5cd14ac94d6d6918145952d524c51e633c1`, which
fails the USB two-packet guest regression described below. They are retained
as historical measurements, not evidence of the corrected tree's correctness
or speed. Required checks and the original four mutations did not cover that
read-driven source transition. Fresh exact-work and codegen results for the
correction are in [read-side.md](read-side.md); no new speed claim is made.

Twelve successful native runs: three alternating pairs per workload, each 30
guest seconds, on ARM64 macOS 27.0.1, Rust 1.99.0 release (unchanged fat LTO,
opt-level 3, codegen-units 1). No cargo/rustc process was detected before or
after any retained sample. One-minute load ranged 5.65–10.10 and was falling;
this is a loaded-machine smoke, not a quiet or thermal-controlled CPU table.
`smoke.json` retains every load and timing; `python3 analyze.py` recomputes the
checks and medians without prior directories, builds or firmware. Commands are
specified by the harness rather than embedding collector-specific paths.

| Workload | Base median wall | Candidate wall | Less wall | Base median user | Candidate user | Less user |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| delay fixture | 4.160906 s | 2.187391 s | 47.43% | 4.128451 s | 2.175679 s | 47.30% |
| display starter | 17.072546 s | 14.106733 s | 17.37% | 17.015850 s | 14.068924 s | 17.32% |

Every pair is faster in both metrics. Per-pair user reductions: delay 47.13%,
47.30%, 50.39%; display 22.55%, 17.32%, 8.44%. Load drift makes the precise
percentage uncertain; no slowdown occurred in this display smoke. This clears
the bounded >=10% delay engineering threshold, not a population/browser claim.
No further scheduling/deadline optimization was attempted.

All six delay runs retire exactly 10,313,837 core-0 instructions, with full
9,735-byte console SHA256
`bd69fbb9ac5c677337a293c1ec0ff7941306eec82c499290917a7c42360f86ad`.
All six display runs retire exactly 1,126,728,507 core-0 instructions, with full
336-byte console SHA256
`c2935031fcffb102bb9f3c720adfc300db2e16b57ddd47f55537338f947e7929`.
Both workloads also retain identical exception counts (zero), interrupt totals
(delay 3,029; display 31,160), and per-line/source interrupt counts. No displayed
instruction number including idle cycles is substituted for retirement.

## Historical static cost and other chips (before read-side correction)

`codegen.py` reuses the committed EX230 normalizer (instruction addresses are
normalized; field offsets and opcodes are retained). Run it on baseline and
candidate `cargo +1.99.0 build --release --bins` outputs:

    python3 docs/evidence/c6-irq-tick/codegen.py "$BASE_BIN" "$CANDIDATE_BIN" "$OUT/codegen"

For each of `esp32sim`, `esp32sim-c3` and `esp32sim-c6`: 106 functions selected,
103 identical; all 90 non-C6 selected functions identical. All binaries contain
multiple chip monomorphizations. The three differences are C6 bus tick (1983
versus 2135 normalized instructions), optional tick (277 versus 322), and
post-round handling (92 versus 100). Previously dead source-change comparisons
and optional return propagation are now live, and the post-round call tests the
new result. There is no non-C6 codegen change. Fetch/decode/execute and timer
deadline code are unchanged. This selector is scoped, not whole-binary identity.

## Historical required checks (before read-side correction)

`checks.json` retains command results; `test-totals.json` has parsed totals.
All Rust commands use `cargo +1.99.0`, nice 10 and an external target directory.

- Native workspace/all-target Clippy with warnings denied: PASS.
- Release WASM Clippy with jit-tests and warnings denied: PASS.
- Release workspace, ignored included except external_, ROM directory and an
  empty HOME: 988 passed, zero failed/ignored.
- Plain release workspace, empty HOME and no ROM environment: 960 passed,
  zero failed, 50 ignored.
- `RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh`: PASS after making the checkout's
  ignored `target` a symlink to the external target directory. The script copies
  from a literal target path; its initial copy failed despite a successful
  compilation. No script or toolchain change was required.
- `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki
  c6-contiki-net c6-rpl-net panel`: all eight PASS after that build-path repair.
- `node tools/check-evidence-privacy.mjs`: PASS (rerun on final tracked evidence).
- No golden output changed. JIT code unchanged; wasm-jit-test is not applicable.

## CPU comparison

Measured October 17, 2026: corrected candidate
`2f0cbb092fcd335822463ead84d63e49368a8a7a` against upstream main
`142805d5399e0c3cf25995fe21ba524e75a2b3cb`, using retained release binaries
without a rebuild. The two C6 30-second workloads use the identical retirement
probe described above on both arms; the other workloads use the native CLI.

User CPU seconds (`wait4` user resource usage), not wall time: one warmup pair
and seven measured pairs per fixture, with `nice -n 10`. Odd measured pairs run
candidate/base; even pairs run base/candidate.

| Workload | Main median user s | Candidate median user s | Change (ratio of medians) | Candidate slower | Flag |
| --- | ---: | ---: | ---: | ---: | --- |
| hello-delay | 3.200214 | 1.630844 | -49.040% | 0/7 | no |
| public-display-starter | 12.530602 | 10.924238 | -12.820% | 0/7 | no |
| s3-hello | 0.048686 | 0.048321 | -0.750% | 3/7 | no |
| c3-hello | 0.263981 | 0.264203 | +0.084% | 4/7 | no |
| c6-hello | 0.342944 | 0.182179 | -46.878% | 0/7 | no |
| classic-hello | 0.061382 | 0.061376 | -0.010% | 4/7 | no |
| pocket-tank-startup | 1.129013 | 1.094202 | -3.083% | 2/7 | no |

All 112 executions (14 warmup runs and 98 measured runs / 49 measured pairs)
match instruction counts, console SHA-256s and stop summaries across arms and
the accepted smoke manifest. Both C6 30-second workloads also match actual
retired-core0 counts. Cycle-derived C3/C6 CLI instruction fields alone are not
retirement proof.

The run window was 05:52:56–05:58:01 UTC+02:00. Initial 1-minute load was
2.369, final load 2.782, maximum 2.861. Monitoring every 0.5 seconds enforced
an initial load below 5, abort at load >=7, no cargo/rustc builds and the
01:00–07:00 local window. No build was detected.

No fixture reaches the flag threshold of slower in at least 6/7 pairs. This
threshold is not a statistical significance test; no layout-only waiver is
claimed. Short hello CPU times are noisy. These bounded native results do not
establish the cause of slow production sessions, a browser/WASM speedup or a
hardware timing correction.
