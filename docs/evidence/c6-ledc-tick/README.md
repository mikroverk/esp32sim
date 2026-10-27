# EX204 follow-up: the LEDC tick skips unconfigured timers and moves wraps out of line

Base: main `104d7bd1`. Toolchain 1.99.0, release profile unchanged. ARM64 macOS host.

Rebased onto main `2e36224d` with code range-diff `=`; the numbers below were measured on `104d7bd1`.

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

## Hot-path disassembly

`codegen_nop.py <base-bins> <candidate-bins> <out>` then `classify.py` ([codegen-classes.json](codegen-classes.json)): every text symbol of the four release binaries (2854 each), normalized for addresses, data pages, import stubs and alignment nops, v0 impl-block ordinals dropped from names. In each binary exactly four symbols change code:

| Symbol | Base insns | Candidate insns |
| --- | ---: | ---: |
| `<Ledc as Device>::tick` (S3/C3/C6) | 186 | 450 |
| `<ClassicLedc as Device>::tick` (classic ESP32, inlines `tick_with_source`) | 433 | 567 |
| `Ledc::wrap_timer` (new, cold) | — | 90 |
| `Ledc<true>::wrap_timer` (new, cold) | — | 90 |

`Ledc::tick` grows because the four-timer loop is now unrolled; the path through it shrinks: the frame is 48 bytes with two saved pairs (was 160 and six), each unconfigured timer costs a bit test and a branch, and a configured timer that does not wrap costs one register read, the field decode, a multiply-add and a store. The remaining differing symbols (25 in `esp32sim-c6`, 26 in each other binary) only call local symbols whose numeric suffix moved by two (`.591` → `.593`; the two new functions shift the numbering), are those renamed local symbols themselves, or differ in a data page offset that is 0 on one side. The classic LEDC tick is the only Xtensa-chip code that changes.

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

## Host instructions and daytime smoke (not a CPU table)

Retired host instructions (`ir.py 10 <base> <candidate>`, `/usr/bin/time -l`, one run each; load-independent to about ±0.05%) ([host-insns.json](host-insns.json)):

| Fixture | Base | Candidate | Change |
| --- | ---: | ---: | ---: |
| c6-hello | 17,962,663,445 | 17,967,339,495 | +0.03% |
| c6-contiki | 40,626,424,966 | 38,700,218,178 | -4.74% |
| c6-energy-scan | 232,687,481,951 | 230,625,533,933 | -0.89% |
| public-display-starter | 114,011,878,042 | 114,016,114,097 | +0.00% |
| c3-hello | 24,753,723,202 | 24,754,290,262 | +0.00% |

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

## Limits

Daytime smokes on a shared, heavily loaded machine; the CPU table (1 warmup + 7 pairs, quiet machine) is pending. Fixtures that never configure the LEDC (c6-hello, the display starter, c3-hello) do not run the changed code. A firmware whose LEDC timer wraps on nearly every round (a PWM period of a few dozen APB cycles) now pays a call per wrap; no fixture here does. Native timing only, not a browser or hardware claim.

## Checks

At the code commit: `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings` and the wasm32 `esp32sim-wasm --features jit-tests` clippy pass; `ESP32SIM_ROM_DIR=… cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_` 1371 passed, 0 failed (empty `HOME`); plain `cargo +1.99.0 test --release --workspace` 1315 passed, 0 failed, 86 ignored; `tools/wasm-build.sh` + `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel` ok; `node tools/check-evidence-privacy.mjs` pass; `git diff --check` clean. No goldens changed.
