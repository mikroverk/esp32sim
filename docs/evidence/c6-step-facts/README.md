# RV32 result-only execution (EX263)

Base: upstream main `04f07602dba76eb6704b8901c989c9c9c1f8b42a`.
Rust 1.99.0, native AArch64 release/LTO and wasm32 release.

## Change and scope

Inline the existing `step_outcome` and `exec_insn` into their callers. The
result-only execution loop can discard observing-only facts without creating a
second interpreter. Fetches, retirement, cycles, traps, IRQs, device accesses,
and instruction budgets are unchanged. The RV32 core is shared by C3, C6 and
P4; there is no new state or public API. Code size grows, and static codegen
alone does not establish a speedup or rule out a layout regression.

## Exact work

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

## Emitted code

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

For WASM, `wasm-functions.json` compares every named function in `wasm-dis`
output with type indices resolved to their exact signatures: 2,784 identical,
19 changed, 30 added, 30 removed. Baseline has five outlined `step_outcome`
functions and ten calls to them; the candidate has none. `attribute_wasm.py`
normalizes numeric suffixes in type-resolved function bodies and hashes them:
25 removed and 25 added bodies pair by hash, eight changed bodies differ only
in suffixes, and the remaining 11 changed bodies, five additions and five
removals are RV32 core or C3/C6/P4 machine execution functions. The eight
suffix-only bodies call renamed functions whose normalized bodies match; the
hash comparison can group duplicate bodies and is not a dynamic performance
proof. Names and matches are in `wasm-attribution.json`. The binary-body
comparison in `wasm-compare.json` also retains function-index differences.
WASM size is 4,065,319 → 4,133,209 bytes (+67,890, +1.67%); deterministic
`gzip -n` size is 1,142,241 → 1,165,704 (+23,463, +2.05%). The selected native
hot-code result is not a whole-module identity claim for WASM.

## Tests and mutations

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
`codegen.py BASE_C6 CANDIDATE_C6 OUT` for native selected symbols and
`mutate.py OUT BASE_C6` with a dedicated target directory in a disposable
candidate tree. For WASM, build both with `tools/wasm-build.sh`, disassemble with
`wasm-dis`, then use `compare_wat.py BASE.wat CANDIDATE.wat OUT.json`. The
probe/mutation builders temporarily edit sources and must not run concurrently
with other builds in the same worktree.

## CPU comparison

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
