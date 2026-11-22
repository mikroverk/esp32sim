# EX252: the LEDC tick skips unconfigured timers and moves wraps out of line

[EX252](../../experiments.md#ex252), PR #301. It started as a follow-up to [EX204](../../experiments.md#ex204) (the LEDC register model and its idle dispatch) and has its own row: this is the hot path while a timer is configured.

Base: main `104d7bd1`. Toolchain 1.99.0, release profile unchanged. ARM64 macOS host.

Rebased onto main `2e36224d` with code range-diff `=`; the numbers below were measured on `104d7bd1` unless marked. The disassembly, the checks, a second exact-work run (with the S3 fixtures) and a host-instruction re-check were redone on main `d043ff1d` against the PR head merged into it (`686305aa`), rustc 1.100.0 (0ef1bc386 2026-11-09); binary hashes in [inputs.json](inputs.json) (`rebased_d043ff1d`).

## What

While any LEDC timer is configured the C6 lists the LEDC as an active optional device and ticks it every scheduling round (at most 64 CPU cycles). `Ledc::tick` (`tick_with_source`) then decoded all four timer configurations, and kept the wrap case (a division, the duty latch over every channel) in the same function, so every call set up a 160-byte frame with six saved register pairs. Profile of main (macOS `sample`, 5 s, top of stack): c6-contiki `Ledc::tick` 8.7% (292 of 3360 samples), the largest self-time symbol outside fetch/decode, the step, the deadline query, the round tick and the GP-SPI2 refresh. On c6-hello and the public display starter the LEDC is not configured and never ticks.

Now:

- timer `n` is skipped when bit `n` of `active` is clear. `active` bit `n` is `timer(n).is_some()`, recomputed on every write to that timer's configuration register (the only writer of both inputs of `timer()`: the register word and `timer_params`), and reset with the device, so the skip drops exactly the timers `timer()` would reject;
- a tick that leaves the phase below the period only stores it; reaching the period, or overflowing u64, goes to the `#[cold]` out-of-line `wrap_timer`, which computes the phase as before (`total % modulus`, the u128 path on overflow), sets the timer's raw bit and latches pending duties.

The classic speed groups use the same `tick_with_source` and get the same code. Behaviour is unchanged: the timer order, phase arithmetic and latch loop are the same.

## Exact work

`exact.py <out> 10 <base> <candidate>`, 10 guest seconds, ROM boot: stop line (instructions, cycles, exceptions, interrupts), stderr with wall time removed and console SHA-256 identical on all six fixtures ([exact-work.json](exact-work.json)).

| Fixture | Stop | Console SHA-256 | Identical |
| --- | --- | --- | --- |
| c6-hello | 1600000000 insns, 1010 interrupts | d5d1f41f315b… | True |
| c6-contiki | 1600000000 insns, 12994 interrupts | 101f1060911f… | True |
| c6-energy-scan | 1600000000 insns, 28722 interrupts | 720c0e65fa18… | True |
| public-display-starter | 1600000000 insns, 10346 interrupts | 1e6395a895a9… | True |
| c3-hello | 1600000000 insns, 1009 interrupts | 6dd87f0ce7e0… | True |
| p4-hello | core0 3600000000 + core1 0 insns | db6c8bcfa9ae… | True |

Again on the rebased pair (main `d043ff1d` against the PR head merged into it, rustc 1.100.0, binaries in `inputs.json`), `FIXTURES='c6-hello c6-contiki c6-energy-scan c3-hello p4-hello hello pocket-tank' exact.py <out> 10 bins/base bins/cand` with `ESP32SIM_ROM_DIR` set ([exact-work-d043ff1d.json](exact-work-d043ff1d.json)): every fixture above except the starter (its images were not at hand) gives the same stop line and console hash as before, and the S3 `hello` and pocket-tank are identical as well:

| Fixture | Stop | Console SHA-256 | Identical |
| --- | --- | --- | --- |
| hello (S3) | core0 3969318 + core1 2257786 insns, 2010 interrupts | 2599f69b42a1… | True |
| pocket-tank (S3) | core0 1403266693 + core1 1423609803 insns, 33067 interrupts | ca5f687faeba… | True |

Neither S3 fixture configures an LEDC timer. Run with `--debug mmio` for 30 emulated seconds (hello as `web/wasm/fw/hello.json`: `--board none --flash-mb 8 --psram-mb 2`; pocket-tank as `web/wasm/fw/pocket-tank.json`, model from `tools/fetch-pocket-tank.sh`, pinned by SHA-256), neither reads or writes any `LEDC` register, so the S3 LEDC never ticks there and the changed S3 code does not run: these two rows show that the rest of the S3 build is unaffected, not that the S3 tick is right. The same check on c6-contiki (3 s) shows the start-up write to timer 0's configuration (`LEDC+0x0a0`). The classic ESP32's tick changes too; no classic fixture was run here.

What the identical rows show: c6-contiki and c6-energy-scan keep timer 0 configured for the backlight (about 33k wraps in 16.7M ticks, counted with an instrumented build in the 10-28 review of PR #301), and nothing in their console depends on the LEDC phase. So identical hashes there show the change has no side effects, not that the LEDC counts right; the unit tests below carry that. The end-to-end check is the `scintilla_p4_lcd35` golden (`cli/tests/goldens.rs`): the P4 synth keeps timer 1 configured (about 15k wraps in 33.5M ticks, same review) and the golden compares the WAV it writes; it passes on this tree (Checks).

## Hot-path disassembly

`codegen_nop.py <base-bins> <candidate-bins> <out>` then `classify.py` ([codegen-classes.json](codegen-classes.json), the class counts and the code rows per binary; `classify.py` lists every differing symbol on stdout): every text symbol of the four release binaries, normalized for addresses, data pages, import stubs and alignment nops, v0 impl-block ordinals dropped from names. In each binary exactly four symbols change code. On `d043ff1d`, rustc 1.100.0 (3515 symbols per binary):

| Symbol | Base insns | Candidate insns |
| --- | ---: | ---: |
| `<Ledc as Device>::tick` (S3/C3/C6/P4) | 187 | 457 |
| `<ClassicLedc as Device>::tick` (classic ESP32, inlines `tick_with_source`) | 434 | 567 |
| `Ledc::wrap_timer` (new, cold) | — | 92 |
| `Ledc<true>::wrap_timer` (new, cold) | — | 91 |

On `104d7bd1`, rustc 1.99.0 (2854 symbols per binary), the same four symbols: 186 → 450, 433 → 567, and 90 for each `wrap_timer`.

`Ledc::tick` grows because the four-timer loop is now unrolled; the path through it shrinks: the frame is 48 bytes with two saved pairs (was 160 and six), each unconfigured timer costs a bit test and a branch, and a configured timer that does not wrap costs one register read, the field decode, a multiply-add and a store. The remaining differing symbols (on `d043ff1d`: 35 in `esp32sim` and `esp32sim-p4`, 38 in `esp32sim-c3` and `esp32sim-c6`; on `104d7bd1`: 25 in `esp32sim-c6`, 26 in each other binary) only call local symbols whose numeric suffix moved (the two new functions shift the numbering), are those renamed local symbols themselves, or differ in a data page offset that is 0 on one side. The classic LEDC tick is the only Xtensa-chip code that changes.

## Mutations

`mutate.py <scratch-worktree> <out>`: each mutant edits one changed (or now load-bearing) line; the esp-periph library tests run first, then the classic ESP32 LEDC tests and the S3/C3/C6 PWM integration tests, stopping at the first crate with a failing test ([mutations.json](mutations.json)). 13 of 13 non-equivalent mutants killed by failing assertions (none by compile errors); the unmutated control passes, and `active-check-removed` survives as expected: without the skip, `timer(n)` rejects the same timers, so it only costs time (equivalent).

New tests in `esp-periph/src/ledc.rs`: `each_timer_alone_counts_wraps_and_latches_its_channel` (for S3, C3, C6 and a classic group: each timer configured alone with a pending duty on channel `n + 1`; counter before and at the wrap, raw bits, duty latch, several wraps in one tick, the other timers' phases stay 0, pause and resume, a second timer with twice the period beside it) and `a_tick_that_ends_exactly_on_the_period_wraps`. Both pass on main as well (`tests_on_base.py`: base production code + new tests, 6/6 LEDC tests pass).

| Mutant | Change | Result | Failing tests (first three) |
| --- | --- | --- | --- |
| control-unmutated | none | control: passes |  |
| active-inverted | skip test `== 0` → `!= 0` | killed | a_tick_that_ends_exactly_on_the_period_wraps, c6_twenty_bit_timer_and_duty_do_not_truncate_or_overflow, each_timer_alone_counts_wraps_and_latches_its_channel |
| active-timer0-bit | test bit 0 for every timer | killed | a_tick_that_ends_exactly_on_the_period_wraps, each_timer_alone_counts_wraps_and_latches_its_channel |
| active-next-bit | test bit `n + 1` | killed | a_tick_that_ends_exactly_on_the_period_wraps, c6_twenty_bit_timer_and_duty_do_not_truncate_or_overflow, each_timer_alone_counts_wraps_and_latches_its_channel |
| active-break | `continue` → `break` (stop at the first unconfigured timer) | killed | a_tick_that_ends_exactly_on_the_period_wraps, each_timer_alone_counts_wraps_and_latches_its_channel |
| active-check-removed | skip test deleted | equivalent: passes |  |
| fast-le | stay-below test `<` → `<=` | killed | a_tick_that_ends_exactly_on_the_period_wraps, static_duty_latches_at_wrap_and_preserves_fractional_divider, each_timer_alone_counts_wraps_and_latches_its_channel |
| fast-no-store | below the period, phase not stored | killed | a_tick_that_ends_exactly_on_the_period_wraps, each_timer_alone_counts_wraps_and_latches_its_channel, static_duty_latches_at_wrap_and_preserves_fractional_divider |
| fast-never | every tick takes the wrap path | killed | each_timer_alone_counts_wraps_and_latches_its_channel, static_duty_latches_at_wrap_and_preserves_fractional_divider |
| wrap-subtract | `total % modulus` → `total - modulus` | killed | each_timer_alone_counts_wraps_and_latches_its_channel, c6_twenty_bit_timer_and_duty_do_not_truncate_or_overflow |
| wrap-overflow-zero | u64 overflow path returns 0 | killed | idle_clock_and_large_batches_match_exact_phase |
| wrap-no-raw | wrap does not set the timer raw bit | killed | a_tick_that_ends_exactly_on_the_period_wraps, each_timer_alone_counts_wraps_and_latches_its_channel |
| wrap-channel-timer-ne | latch channels on the other timers | killed | each_timer_alone_counts_wraps_and_latches_its_channel, c6_twenty_bit_timer_and_duty_do_not_truncate_or_overflow, static_duty_latches_at_wrap_and_preserves_fractional_divider |
| active-never-cleared | configuration write only sets `active` bits | killed | each_timer_alone_counts_wraps_and_latches_its_channel, idle_clock_and_large_batches_match_exact_phase |
| active-never-set | configuration write only clears `active` bits | killed | c6_twenty_bit_timer_and_duty_do_not_truncate_or_overflow, static_duty_latches_at_wrap_and_preserves_fractional_divider, idle_clock_and_large_batches_match_exact_phase |

## Host instructions and daytime smoke (the claim; the smoke is not a CPU table)

Retired host instructions (`ir.py 10 <base> <candidate>`, `/usr/bin/time -l`, one run each; load-independent to about ±0.05%) ([host-insns.json](host-insns.json)):

| Fixture | Base | Candidate | Change |
| --- | ---: | ---: | ---: |
| c6-hello | 17,962,663,445 | 17,967,339,495 | +0.03% |
| c6-contiki | 40,626,424,966 | 38,700,218,178 | -4.74% |
| c6-energy-scan | 232,687,481,951 | 230,625,533,933 | -0.89% |
| public-display-starter | 114,011,878,042 | 114,016,114,097 | +0.00% |
| c3-hello | 24,753,723,202 | 24,754,290,262 | +0.00% |

Re-check on `d043ff1d`, rustc 1.100.0 (`ir.py 10 bins/base bins/cand`, one run each, same stop line and console as the base on every row; user CPU and cycles left out of the file) ([host-insns-d043ff1d.json](host-insns-d043ff1d.json)):

| Fixture | Base | Candidate | Change |
| --- | ---: | ---: | ---: |
| c6-hello | 17,669,311,936 | 17,667,409,721 | -0.01% |
| c6-contiki | 39,977,309,021 | 38,156,427,967 | -4.55% |
| c6-energy-scan | 227,345,065,743 | 225,497,997,805 | -0.81% |
| c3-hello | 27,470,992,855 | 27,470,580,102 | -0.00% |

Screening ablation (host instructions, c6-contiki 10 s, one run each, same build command): the timer skip alone −2.43%, the cold wrap alone −1.26%, both −4.72%; each part carries part of the gain.

User CPU from `wait4`, 10 guest seconds, 1 warmup pair + 3 alternating pairs; the 1-minute load (other work on the machine) is in the last column ([smoke.json](smoke.json)):

| Fixture | Base median s | Candidate median s | Change | Candidate slower | Load |
| --- | ---: | ---: | ---: | ---: | --- |
| c6-contiki | 2.607 | 2.325 | -10.8% | 0/3 | 14–16 |
| c6-energy-scan | 18.102 | 18.116 | +0.1% | 2/3 | 15–21 |
| c6-hello | 1.148 | 1.138 | -0.8% | 0/3 | 19–20 |

A/A control, main against itself, same procedure, right after ([smoke-aa.json](smoke-aa.json)):

| Fixture | Base median s | Base (2nd arm) median s | Change | 2nd arm slower | Load |
| --- | ---: | ---: | ---: | ---: | --- |
| c6-contiki | 2.635 | 2.493 | -5.4% | 0/3 | 19–23 |
| c6-energy-scan | 18.220 | 17.848 | -2.0% | 0/3 | 23–31 |
| c6-hello | 1.078 | 1.032 | -4.3% | 0/3 | 23–26 |

The A/A arms differ by up to 5%, so user CPU alone does not resolve this change at this load. Host counters on c6-contiki (`smoke_hw.py`, `/usr/bin/time -l`, 1 warmup + 3 alternating pairs, load 20–23, [smoke-hw.json](smoke-hw.json)): user CPU 2.71 → 2.59 s (-4.4%, 0/3 higher), retired host instructions -4.74% (0/3 higher), elapsed host cycles -3.89% (0/3 higher).

## CPU (maintainers' run)

Run by the maintainers, not by the author of the change: `smoke_hw.py` from this folder, base main `d043ff1d`, new the PR head merged into it (`686305aa`), each built with `cargo build --release --bins` in its own worktree, the two paths of equal length. One warmup pair and seven alternating pairs per fixture, 30 guest seconds: the S3 `hello`, `c3-hello`, `c6-hello` and c6-contiki (the one that configures the LEDC). From the new tree's root:

```sh
ESP32SIM_ROM_DIR=<roms> FIXTURES='hello c3-hello c6-hello c6-contiki' \
  python3 -I docs/evidence/c6-ledc-tick/smoke_hw.py docs/evidence/c6-ledc-tick/cpu-pairs.json 30 7 <base-tree>/target/release target/release
```

<!-- CPU TABLE -->

## Limits

The speed claim is the retired host instruction count: c6-contiki −4.74% (−4.55% on `d043ff1d` with rustc 1.100.0), c6-energy-scan −0.89% (−0.81%), and no change on the fixtures that never configure the LEDC (c6-hello, the display starter, c3-hello; the S3 `hello` and pocket-tank never touch the LEDC either), which do not run the changed code. The daytime user-CPU smokes ran on a shared, heavily loaded machine whose A/A control moved up to 5.4%, so they are not a CPU claim; user CPU is the maintainers' run above. A firmware whose LEDC timer wraps on nearly every round (a PWM period of a few dozen APB cycles) now pays a call per wrap; no fixture here does. Native only, not a browser or hardware claim.

## Checks

On `686305aa` (the PR head merged into main `d043ff1d`), rustc 1.100.0: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings` pass; `ESP32SIM_ROM_DIR=… cargo test --release --workspace --no-fail-fast -- --include-ignored --skip external_` 1809 passed, 0 failed, including the `scintilla_p4_lcd35` golden; `node tools/check-evidence-privacy.mjs` pass; `git diff --check` clean. No goldens changed.

At the code commit, on `104d7bd1`: `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings` and the wasm32 `esp32sim-wasm --features jit-tests` clippy pass; `ESP32SIM_ROM_DIR=… cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_` 1371 passed, 0 failed (empty `HOME`); plain `cargo +1.99.0 test --release --workspace` 1315 passed, 0 failed, 86 ignored; `tools/wasm-build.sh` + `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel` ok; `node tools/check-evidence-privacy.mjs` pass; `git diff --check` clean. No goldens changed.
