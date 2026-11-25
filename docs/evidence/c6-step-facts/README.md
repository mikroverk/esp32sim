# RV32 result-only execution (EX263)

Rebased base: upstream main `816246c86fe8395a16bbf95538e9bd9d6d8356a3` (includes #383). The maintainer's x86 run used Rust 1.100.0; historical measurements below retain their recorded toolchain.

Earlier results below use `c693fe897f2b6ce39aa93bf14eba7810e17cba40`, `3f49dad86878afca429508a7a8b2e8f0bf38ea6e` or `04f07602dba76eb6704b8901c989c9c9c1f8b42a` and their respective candidates; they are historical and do not establish current codegen or performance. EX265 added RV32 decoded blocks; this experiment instead requested inlining for the result-only interpreter path. Main retains `#[inline]` on `exec_insn` after the EX265 follow-up found forcing it inline slowed C3. The later candidate added only `#[inline(always)]` on `step_outcome`; that request is now dropped. Adoption: not adopted, superseded by EX270.

## Change and scope

Request inlining of the existing `step_outcome` into its callers. The
result-only execution loop can discard observing-only facts without creating a
second interpreter. Fetches, retirement, cycles, traps, IRQs, device accesses,
and instruction budgets are unchanged. The RV32 core is shared by C3, C6 and
P4; there is no new state or public API. Static codegen
alone does not establish a speedup or rule out a layout regression.

## Interaction with code versions (#383)

The C3, C6 and P4 buses now supply page versions used by the decoded-block cache. When blocks are enabled and versions are available, `Core::run` dispatches to `block::run`; writes that change fetched code advance the corresponding versions so cached blocks can be invalidated. With blocks disabled or no page versions, `Core::run` uses the result-only `exec::step` loop. The block path also calls `step_outcome` for slow-path events. The inline request changes neither the dispatch condition nor version tracking or instruction semantics. The historical x86 table below measured the interpreter loop that `816246c8` runs only with `--no-jit`; `RV32_JIT=0` still uses blocks and is the default on hosts without a JIT backend. The following current-base measurements cover all three modes; they do not quantify path frequency.

### Maintainer's current-base measurement

Maintainer review of PR #335, 2026-11-25: main `816246c86fe8395a16bbf95538e9bd9d6d8356a3` versus candidate `72639f00a0cd5eabfe944b9073d823849a341900`, native AArch64, Rust 1.100.0, host instructions retired via `/usr/bin/time -l`. Each cell is the median of three alternating runs; within-arm spread was at most 0.2%. Console and complete end-of-run reports (guest instructions, interrupts, blocks built, JIT regions) matched in every cell. Hello uses the CONTRIBUTING commands (C3/C6 30 s, P4 10 s); Espressif `test_apps/coremark` ran as in EX270 until `CPU frequency`, checking `crcfinal 0xa14c` on every run. User CPU was not reported because the machine was loaded; these are instruction counts, not a CPU timing table.

| workload | default (blocks + JIT) | `RV32_JIT=0` (blocks only) | `--no-jit` (interpreter) |
|---|---|---|---|
| c3-hello | −0.01% | +0.03% | −0.31% |
| c6-hello | +0.02% | +0.04% | −0.54% |
| p4-hello | +0.10% | +0.00% | −0.09% |
| C3 CoreMark | +0.81% (92.9 → 93.6 G) | +1.75% (202.6 → 206.1 G) | −10.90% (421.7 → 375.7 G) |
| C6 CoreMark | +1.44% (79.3 → 80.5 G) | +2.07% (189.5 → 193.4 G) | −13.22% (358.1 → 310.8 G) |
| P4 CoreMark | +0.59% (133.8 → 134.5 G) | +1.42% (243.8 → 247.3 G) | −9.83% (466.2 → 420.4 G) |

The maintainer also tested two variants isolating the interpreter: both retained its gain but cost +0.2% to +0.6% in default CoreMark and +2.5% to +4.1% with blocks alone. The default-path costs outweigh adopting the attribute for the debugging oracle; no new Mac timing table was run.

## Current-base native code generation

The AArch64 C6 executable built with Rust 1.99.0 compares upstream main `816246c86fe8395a16bbf95538e9bd9d6d8356a3` with `b00589c0e351f13c3be753c9d4991b282fb30384` (the source is unchanged by this receipt). The EX230 normalized comparison in `codegen-816246c8.json` selects 242 functions: 216 identical and 26 changed, added or removed; the report retains only differing rows. These counts include symbol renumbering and are not whole-binary identity.

On the candidate, `block::run` is newly outlined for C3, C6, P4 and S31, with respectively 2,436, 5,040, 2,481 and 2,759 static operations. `step_core` grows from 464 to 1,508 operations for C3, 466 to 1,600 for C6 and 509 to 1,601 for S31; C6 `run_until_cycle` grows from 4,590 to 5,308. These compiler changes follow the sole runtime source change, the `step_outcome` inline request, not a source-level change to the block path. Static size is not runtime cost; see the host-instruction measurements above. The maintainer reports the same outlining with Rust 1.100.0: candidate `block::run` outlined and `step_outcome` absent, vice versa on main.

To reproduce, use separate clean base and candidate worktrees at the revisions above, separate `CARGO_TARGET_DIR` directories, and in each run `nice -n 10 cargo +1.99.0 build --release --bins`. From the candidate worktree run `python3 docs/evidence/c6-step-facts/codegen.py BASE_TARGET/release/esp32sim-c6 CANDIDATE_TARGET/release/esp32sim-c6 OUT_DIR`; the resulting `OUT_DIR/codegen.json` uses `xcrun llvm-objdump` and the EX230 normalizer. The recorded toolchain is rustc 1.99.0 (`b940084d7`, LLVM 23.1.1), host `aarch64-apple-darwin`. Binary hashes can vary with build paths; compare normalized rows and totals.

## Historical rebase screening (base 3f49dad8, 2026-11-23)

A daytime screen compared main `3f49dad8` with the rebased implementation `5854362e`, using `cargo +1.99.0 build --release --bins` on each side, one warmup and three rotating A/B/A runs per workload, user CPU, 30 guest seconds. The A/A arm runs the same base binary twice. This is a noisy smoke, not a seven-pair quiet-machine CPU table; the A/A spread limits conclusions. Each workload matched at 4,800,000,000 scheduler instructions and its console hash, with no exceptions and matching interrupt totals (3,028 / 3,029 / 31,160). Samples and hashes: `rebase-smoke.json`.

| Workload | Candidate/base paired deltas | A/A paired deltas |
| --- | --- | --- |
| C3 hello | +19.05%, +0.01%, -1.94% | +50.65%, +2.04%, +1.85% |
| C6 hello | -9.58%, -0.54%, -11.78% | -11.14%, -24.57%, -17.58% |
| C6 display starter | -7.85%, -8.19%, -9.57% | -0.14%, +1.10%, +0.56% |

On that earlier base the display screen suggested a benefit but did not establish a formal CPU result; C3 and C6 hello were inconclusive under this noise. This is not evidence for the current base.

## Historical exact work (old base)

Both arms ran each input for 10 guest seconds with unprobed release `--bins`
builds. The unprobed outputs give scheduler work, exceptions, interrupt lines
and sources, and console hashes. Architectural retirement comes instead from a
separate post-run reporting probe; it is not a measurement from the unprobed
binary. All four runs per fixture (base/candidate, unprobed/probed) agree on
console, work and interrupts; both probes agree on retirement. The ROM and
firmware hashes, including the externally supplied display starter, are pinned in
`inputs.json`; full rows and binary hashes are in `exact-work.json`.

| Fixture | Retired, both probes | Scheduler insns, both | IRQs, both | Console SHA-256, both |
| --- | ---: | ---: | ---: | --- |
| C3 hello | 3,181,874 | 1,600,000,000 | 1,009 | `6dd87f0ce7e0bbefdf2ddbf2347caf26066d82fa03f942985413a1156320458a` |
| C6 hello/delay | 3,657,500 | 1,600,000,000 | 1,010 | `d5d1f41f315b6ba0f33ff9dc55ca01abb7256791bd21d6caac9442f665b689c0` |
| C6 display starter | 373,050,392 | 1,600,000,000 | 10,346 | `1e6395a895a9d9663bd4aaf22cbd3049c60d546e87b2c86e3ab248ba4547215b` |

Zero exceptions in all runs. For both C3 and C6 binaries the unprobed versus
probed comparison passes on 128/128 selected normalized hot symbols on base
and 130/130 on candidate (`probe-c3.json`, `probe-c6.json`). The normalizer
handles an ADRP followed by a zero-offset load as well as an explicit page
offset: without that correction three base machine functions differ only in
that address syntax. This is a selected-function check, not a whole-binary
identity claim.

## Historical code generation against c693fe89 (not this rebase)

These native and WASM measurements predate #383 and do not establish code generation on `816246c8`. AArch64 release `--bins`, base `c693fe897f2b6ce39aa93bf14eba7810e17cba40` versus implementation `7a0d64e7517312b2957de8a79fb65a80ddc43d5c`: `codegen-c693.json` compares 218 selected normalized symbols (186 identical, 32 changed, added or removed). C6 outlined `step_outcome` is 3,157 static operations on base and absent on the candidate. S31 `Core::run` changes from 1,452 to 1,160 operations, and `block::run` becomes a separate 2,759-operation symbol (absent as a separate symbol on base). This outlining is a compiler consequence of the sole runtime source change, the `step_outcome` inline request; there is no source-level block change. Static body sizes are not executed instruction counts; the runtime effect is unproven.

WASM release comparison (`wasm-c693.json`): 3,508 named function texts identical, 31 changed, 41 added and 41 removed; outlined `step_outcome` functions six to zero. `wasm-dis` type indices are resolved to signatures, but other symbol and layout changes remain, so this is literal text, not semantic equivalence. Module size is 4,660,205 → 4,726,598 bytes; `gzip -n` size is 1,360,880 → 1,387,586 bytes. Module SHA-256: base `96d386556edba33a291395ca1253891a66fe22325277d0720789e934687b87b2`, candidate `1feb8f96d4cc41bf6c65fd3014a0f5587d0bd263f7e96cba8d49ce11773434bc`.

To repeat this comparison with Rust 1.100.0, use disposable base and candidate worktrees at the two revisions above and separate target directories. The recorded codegen and hashes above were produced with Rust 1.99.0; a 1.100.0 rerun is not expected to reproduce them byte-for-byte. In each worktree, set `CARGO_TARGET_DIR` to its own absolute target directory and symlink `target` to it, then run `nice -n 10 cargo +1.100.0 build --release --bins` and save `target/release/esp32sim-c6`. Run `python3 docs/evidence/c6-step-facts/codegen.py BASE_C6 CANDIDATE_C6 OUT` from the candidate tree (uses `xcrun llvm-objdump`); it writes `OUT/codegen.json`. In each worktree run `RUSTUP_TOOLCHAIN=1.100.0 nice -n 10 tools/wasm-build.sh` and save `web/wasm/esp32sim.wasm`; run `wasm-dis BASE.wasm -o BASE.wat`, `wasm-dis CANDIDATE.wasm -o CANDIDATE.wat`, then `python3 docs/evidence/c6-step-facts/compare_wat.py BASE.wat CANDIDATE.wat OUT.json`. Use `shasum -a 256`, `wc -c` and `gzip -n -c FILE | wc -c` on the saved modules for hashes and sizes. These are selected-symbol and text comparisons, not whole-binary identity or runtime measurements.

## Historical code generation (base 3f49dad8)

The following native and WASM results are for `3f49dad8` and its candidate, not `816246c8`.

The native AArch64 release `--bins` comparison (`codegen-rebase.json`) selects 196 functions, 168 identical and 28 changed, added or removed. The outlined C6 `step_outcome` has 3,149 static operations on base and is absent in the candidate. This is a selected-symbol comparison, not a whole-binary identity or a dynamic performance proof.

For WASM, `wasm-functions.json` compares `wasm-dis` function text, resolving type indices to signatures; 3,462 are identical, 31 changed, 41 added and 41 removed. Only changed/added/removed rows are retained; symbol and layout changes can affect this literal comparison. Outlined `step_outcome` functions: six on base, zero on candidate. WASM bytes: 4,568,056 → 4,634,960 (+66,904, +1.46%); `gzip -n` bytes: 1,343,790 → 1,368,013 (+24,223, +1.80%). SHA-256: base `d23f1ee0a452cad5696a1f68569aadcb2eabff7462b9c2187db82bed776c547a`, candidate `128ac60732b9b807c17ebc4f4d3b4db40f91962bd179682f79376af905fbc813`. The old-base WASM numbers are historical and do not describe this rebase.

## Historical emitted code (old base)

Native AArch64 normalized comparison (`codegen.json`) selects 134 functions:
113 identical and 21 changed, added or removed. The changes are RV32 C3/C6/P4
run, modeled and stepping functions; selected bus, IRQ, decode and Xtensa
functions are unchanged, as are the selected classic and S3 machine run loops.
The C3/C6 `run_unmodeled` bodies change their register allocation and
control-flow layout as the outlined step moves into the RV32 run path: C3
1,060 → 1,040 and C6 1,065 → 1,058 static operations. All 21 selected
changed/added/removed symbols and their per-symbol sizes are listed in
`codegen.json`; they include RV32 machine run, modeled and step functions, not
only the unmodeled loops. Static comparison cannot rule out a layout or idle
performance effect. Baseline C6 `step_outcome` is 1,853 static operations
and is called from the run path; the candidate has no such outlined function
and has a 2,049-operation C6 `Core::run`. These body sizes are not dynamic
instruction counts. P4 codegen changes because it uses the same interpreter.

The selected native hot-code result is not a whole-module identity claim for WASM.

## Checks on 816246c8 rebase

With empty HOME and explicit Cargo/Rustup paths, native workspace and release WASM jit-tests Clippy with Rust 1.100.0 and `-D warnings` passed. Rust 1.99.0 ROM-enabled release workspace tests with `--no-fail-fast -- --include-ignored --skip external_` passed 1,914 tests; the plain release workspace passed 1,831 with 117 ignored. The maintainer independently reports 1,914 passing ROM-enabled tests with Rust 1.100.0 on `72639f00`. The WASM build and eight demos listed below passed, as did the evidence privacy checker. In a separate worktree, `mutate-rebase.py` killed all three runtime mutants by test failures; `outline-mut-rebase.py` killed the inline-removal mutant via the emitted-code gate. These checks do not establish current-base timing; the selected native codegen comparison is described above.

## Historical rebase checks (c693fe89 and earlier)

On `c693fe89` versus `7a0d64e7`, the ROM-enabled release workspace test with `--include-ignored --skip external_` passed 1,855 tests. With Rust 1.99.0, native and WASM `-D warnings` clippy both exit 101 on base and candidate on the same pre-existing `needless_range_loop` at `esp32s31/src/soc.rs:410`. The maintainer reports both Clippy commands pass on main `c693fe89` with Rust 1.100.0.

The following earlier checks were run against base `3f49dad8`, not `816246c8`. With an empty HOME and explicit Rustup/Cargo paths, Rust 1.99.0 (the following workspace counts are from the original rebase checks before adding the CLIC test):

- Native workspace/all-targets and release WASM JIT-feature clippy with `-D warnings`: both exit 101 on base and candidate. In each paired gate, error lines and locations are identical: `clippy::needless_range_loop` at `esp32s31/src/soc.rs:410`, pre-existing on main, unchanged; candidate adds zero diagnostics.
- ROM-enabled `cargo +1.99.0 test --release --workspace --no-fail-fast -- --include-ignored --skip external_`: 1,851 passed, zero failed.
- Plain `cargo +1.99.0 test --release --workspace --no-fail-fast`: 1,778 passed, 104 ignored, zero failed.
- `RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh` and `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel`: passed all eight demos.
- `node tools/check-evidence-privacy.mjs`: passed before staging.
- CLIC follow-up: focused three-crate release tests passed; ROM-enabled workspace passed 1,852 tests and plain workspace passed 1,779 (104 ignored), with no failures. Native and WASM clippy again matched base's existing diagnostic; all eight WASM demos passed.
- CLIC differential: mtvec modes 0 and 3 with a pending level-5 interrupt both pass on this candidate and main `3f49dad8`, comparing observing, result-only and one-instruction run state. Setting the CLINT-mode delivery's `mil` to zero instead of the delivered level causes `clic_interrupt_delivery_matches_all_paths` to fail its independent level assertion (runtime assertion, not compile failure).
- `mutate-rebase.py` killed three runtime mutants without compile errors: omitted retirement, doubled instruction cycles, and omitted observer fetch bytes (`mutations-rebase.json`). `outline-mut-rebase.py` also killed removal of the `step_outcome` inline request via the native emitted-code gate (`outline-result-rebase.json`). The old `exec_insn` inline mutation no longer applies: this rebase intentionally retains main's plain inline hint.

## Historical tests and mutations (old base)

The differential test covers 49,152 compressed encodings, 16,384 deterministic
full-width words and 77 directed instruction/state combinations, comparing CPU
fields, RAM, the observing step, result-only step, and one-instruction run.
It includes independent retirement and cycle assertions. This is a semantic
preservation test and also passes on main; it is not a speed oracle.

Five mutations were killed, none by a compile error (`mutations.json`): removing
either inline request fails the native codegen gate; dropping retirement or
doubling cycles fails the differential test; removing the observing fetch bytes
fails the existing fetch-window test.

With empty HOME and Rustup/Cargo paths retained, the required checks passed:

- `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings`.
- `cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings`.
- `ESP32SIM_ROM_DIR=<fw> cargo +1.99.0 test --release --workspace --no-fail-fast -- --include-ignored --skip external_`: 1,460 passed.
- Plain `cargo +1.99.0 test --release --workspace --no-fail-fast`: 1,396 passed, 95 ignored.
- `RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh` and `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel`: eight demos passed.
- `node tools/check-evidence-privacy.mjs`: passed after staging evidence.

## Reproduction

Use disposable main/candidate worktrees and separate target directories. Build
both using `cargo +1.99.0 build --release --bins`; save the unprobed C3/C6
binaries. Run `build_probe.py TREE PREFIX` with the matching target directory,
then compare each chip's two pairs with `check_probe.py OUT BASE BASE_PROBE
CANDIDATE CANDIDATE_PROBE`. `exact_work.py OUT BASE_PREFIX CAND_PREFIX FW
STARTER` uses the pinned inputs and saved normal/probed binaries. Use
`codegen.py BASE_C6 CANDIDATE_C6 OUT` for native selected symbols. Reports retain only changed, added and removed rows; totals include identical symbols. To reproduce this report-only curation without rebuilding the historical binaries, for each `f` in `codegen.json codegen-rebase.json`, run `git show 1f0cdc135be34bcb5684f3593f3eb62646d6b5a0:docs/evidence/c6-step-facts/$f > "$f.full"` then `python3 docs/evidence/c6-step-facts/codegen.py --from-report "$f.full" "$f"`. This reuses the original full comparisons rather than claiming a new measurement. SHA-256: `codegen.json` `3e58762185ecf783a5575e3752f3dcd8d06cd6dc34b47336c237ddb8537ce70f`; `codegen-rebase.json` `1bea92dc2a75364ce7ca4fd53515b073d8c7b3449b0c59f85bacd6987dab3390`.
Use `mutate.py OUT BASE_C6` with a dedicated target directory in a disposable
candidate tree. For WASM, build both with `tools/wasm-build.sh`, disassemble with
`wasm-dis`, then use `compare_wat.py BASE.wat CANDIDATE.wat OUT.json`. The
probe/mutation builders temporarily edit sources and must not run concurrently
with other builds in the same worktree.

## CPU comparison

No new Mac CPU table was run against `816246c8`. The table immediately below is the maintainer's historical x86 run against `c693fe89`; it measured the interpreter loop that `816246c8` runs only with `--no-jit`. The later Mac table is also historical, not the current-base comparison.

### Historical maintainer x86 comparison against c693fe89

The maintainer compared the result-only `step_outcome` inline change merged with main `c693fe89` against that main, on Linux x86_64, i9-12900F, Rust 1.100.0, pinned with `taskset -c 4` to one P-core. Load was approximately 1 on 24 threads. User CPU came from child rusage; perf counters were unavailable. After one warmup, 11 rounds alternated order. Instruction counts and console SHA-256 matched across runs; a main-versus-main A/A arm stayed within ±0.43%.

| Workload (guest duration) | Main user CPU | Result-only delta | Slower pairs |
| --- | ---: | ---: | ---: |
| c6-energy-scan (10 s) | 11.95 s | −9.9% | 0/11 |
| c6-contiki (12 s) | 2.33 s | −6.0% | 0/11 |
| c6-hello (30 s) | 2.40 s | −4.2% | 0/11 |
| c6-wifi-station (14 s) | 1.58 s | −2.6% | 0/11 |
| c3-wifi-station (14 s) | 2.34 s | −0.9% | 0/11 |
| s31-hello (30 s) | 6.72 s | −0.5% | 0/11 |
| c3-hello (30 s) | 4.01 s | +1.8% | 11/11 |
| p4-hello (10 s) | 6.77 s | +2.1% | 11/11 |

A second run reproduced C3 hello at +2.0% and P4 hello at +2.5% (both 11/11 slower), and the C6 gains within 0.5 percentage points. An alignment check using `-C llvm-args=-align-all-functions=6` moved main alone by up to ±5%; aligned candidate versus aligned main retained the signs (C3 hello about +4%, P4 hello about +2%, C6 −4% to −9%). Restricting forced inlining to `exec::step` gave identical numbers. The measured trade-off on this machine is a C6 gain of 2.6–9.9%, a small consistent cost on C3 and P4 hello, and a neutral S31 hello result (−0.5%); the alignment check does not suggest the C3/P4 cost is merely an alignment accident.

### Historical Mac comparison (old base)

Base `04f07602dba76eb6704b8901c989c9c9c1f8b42a`; candidate `dd72abaa7b30bc92ec5c0d2ce16c306b42503616`. On 2026-11-01 03:02–03:03 CET, unprobed rustc 1.99.0 release binaries ran on macOS AArch64, measuring user CPU with `/usr/bin/time -l` at 0.01 s resolution: one warmup and seven alternating pairs per workload, with sampled load 1.65–2.33. Each run stopped at scheduler count 1,600,000,000 and had identical console SHA-256 within its workload: hello `d5d1f41f315b6ba0f33ff9dc55ca01abb7256791bd21d6caac9442f665b689c0`, display starter `1e6395a895a9d9663bd4aaf22cbd3049c60d546e87b2c86e3ab248ba4547215b`. Retirement equality comes from the separate post-run reporting build described above, not from the timed binaries.

C6 hello:

| Pair | Base user CPU s | Candidate user CPU s | Delta |
|---:|---:|---:|---:|
| 1 | 0.48 | 0.47 | -2.08% |
| 2 | 0.46 | 0.47 | +2.17% |
| 3 | 0.47 | 0.46 | -2.13% |
| 4 | 0.48 | 0.47 | -2.08% |
| 5 | 0.46 | 0.49 | +6.52% |
| 6 | 0.47 | 0.48 | +2.13% |
| 7 | 0.47 | 0.46 | -2.13% |

Median paired delta -2.08%; candidate slower 3/7; not flagged (threshold ≥6/7).

C6 display starter (Waveshare C6 LCD 1.47, 10 emulated seconds):

| Pair | Base user CPU s | Candidate user CPU s | Delta |
|---:|---:|---:|---:|
| 1 | 3.12 | 2.83 | -9.29% |
| 2 | 3.12 | 2.91 | -6.73% |
| 3 | 3.12 | 2.82 | -9.62% |
| 4 | 3.31 | 2.98 | -9.97% |
| 5 | 3.29 | 3.03 | -7.90% |
| 6 | 3.24 | 2.81 | -13.27% |
| 7 | 3.17 | 2.88 | -9.15% |

Median paired delta -9.29%; candidate slower 0/7; not flagged (threshold ≥6/7).

The 0.01 s resolution puts hello (~0.47 s/run) near the noise floor; C3, P4 and WASM were not timed.
