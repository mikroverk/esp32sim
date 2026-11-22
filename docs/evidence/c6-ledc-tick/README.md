# EX252: the LEDC tick skips unconfigured timers and moves wraps out of line

[EX252](../../experiments.md#ex252), PR #301. It started as a follow-up to [EX204](../../experiments.md#ex204) (the LEDC register model and its idle dispatch) and has its own row: this is the hot path while a timer is configured.

Base: main `104d7bd1`. Toolchain 1.99.0, release profile unchanged. ARM64 macOS host.

Rebased onto main `2e36224d` with code range-diff `=`; the numbers below were measured on `104d7bd1` unless marked. The disassembly, the mutations, the checks, a second exact-work run (with the S3 fixtures) and a host-instruction re-check were redone on main `d043ff1d` against the PR head merged into it (`686305aa`), rustc 1.100.0 (0ef1bc386 2026-11-09); binary hashes in [inputs.json](inputs.json) (`rebased_d043ff1d`).

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

Neither S3 fixture configures an LEDC timer. Run with `--debug mmio` for 30 emulated seconds (hello as `web/wasm/fw/hello.json`: `--board none --flash-mb 8 --psram-mb 2`; pocket-tank as `web/wasm/fw/pocket-tank.json`, model from `tools/fetch-pocket-tank.sh`, pinned by SHA-256), neither reads or writes any `LEDC` register, and the probe below finds no call, so the changed S3 code does not run: these two rows show that the rest of the S3 build is unaffected, not that the S3 tick is right. The same check on c6-contiki (3 s) shows the start-up write to timer 0's configuration (`LEDC+0x0a0`). The classic ESP32's tick changes too; no classic fixture that configures the LEDC was run.

[tick-probe.patch](tick-probe.patch) answers which fixtures execute the changed code at all: a throwaway counter at the top of `tick_with_source`, the function every LEDC tick goes through, that prints at each power of two (it applies to `5cfee516` and later trees with the same `ledc.rs`; never committed as code). Ten guest seconds on this `ledc.rs`: no call on the S3 hello, pocket-tank, c3-hello, c6-hello, p4-hello and the classic hello; at least 2^24 (16,777,216) calls on c6-contiki and on c6-energy-scan, none with `active` 0.

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

`Ledc::tick` grows because the four-timer loop is now unrolled; the path through it shrinks: the frame is 48 bytes with two saved pairs (was 160 and six), each unconfigured timer costs a bit test and a branch, and a configured timer that does not wrap costs one register read, the field decode, a multiply-add and a store. The remaining differing symbols (on `d043ff1d`: 35 in `esp32sim` and `esp32sim-p4`, 38 in `esp32sim-c3` and `esp32sim-c6`; on `104d7bd1`: 25 in `esp32sim-c6`, 26 in each other binary) only call local symbols whose numeric suffix moved (the two new functions shift the numbering), are those renamed local symbols themselves, or differ in a data page offset that is 0 on one side. Of the symbols `codegen_nop.py` attributes to the Xtensa chip crates (on `d043ff1d` 4 differ in `esp32sim` and `esp32sim-p4`, 12 in `esp32sim-c3` and `esp32sim-c6`), only the classic LEDC tick is in class `code`; the S3's tick is the shared `Ledc` one. The others are `renumbered` or `pageoff` (S3 and classic `run`, `run_unmodeled`, `run_modeled` and `block::build` among them in some binaries): data they address moved.

## Mutations

`mutate.py <scratch-worktree> <out>`: each mutant edits one changed (or now load-bearing) line; the esp-periph library tests run first, then the classic ESP32 LEDC tests and the S3/C3/C6 PWM integration tests, stopping at the first crate with a failing test ([mutations.json](mutations.json)). On `d043ff1d`, rustc 1.100.0 (and before on `104d7bd1`, 1.99.0, same result): 13 of 13 non-equivalent mutants killed by failing assertions (none by compile errors); the unmutated control passes, and `active-check-removed` survives as expected: without the skip, `timer(n)` rejects the same timers, so it only costs time (equivalent).

New tests in `esp-periph/src/ledc.rs`: `each_timer_alone_counts_wraps_and_latches_its_channel` (for S3, C3, C6 and a classic group: each timer configured alone with a pending duty on channel `n + 1`; counter before and at the wrap, raw bits, duty latch, several wraps in one tick, the other timers' phases stay 0, pause and resume, a second timer with twice the period beside it) and `a_tick_that_ends_exactly_on_the_period_wraps`. Both pass on main as well (`tests_on_base.py`: base production code + new tests, 6/6 LEDC tests pass).

| Mutant | Change | Result | Failing tests (first three) |
| --- | --- | --- | --- |
| control-unmutated | none | control: passes |  |
| active-inverted | skip test `== 0` → `!= 0` | killed | a_tick_that_ends_exactly_on_the_period_wraps, c6_twenty_bit_timer_and_duty_do_not_truncate_or_overflow, each_timer_alone_counts_wraps_and_latches_its_channel |
| active-timer0-bit | test bit 0 for every timer | killed | each_timer_alone_counts_wraps_and_latches_its_channel, a_tick_that_ends_exactly_on_the_period_wraps |
| active-next-bit | test bit `n + 1` | killed | a_tick_that_ends_exactly_on_the_period_wraps, c6_twenty_bit_timer_and_duty_do_not_truncate_or_overflow, idle_clock_and_large_batches_match_exact_phase |
| active-break | `continue` → `break` (stop at the first unconfigured timer) | killed | a_tick_that_ends_exactly_on_the_period_wraps, each_timer_alone_counts_wraps_and_latches_its_channel |
| active-check-removed | skip test deleted | equivalent: passes |  |
| fast-le | stay-below test `<` → `<=` | killed | a_tick_that_ends_exactly_on_the_period_wraps, each_timer_alone_counts_wraps_and_latches_its_channel, static_duty_latches_at_wrap_and_preserves_fractional_divider |
| fast-no-store | below the period, phase not stored | killed | each_timer_alone_counts_wraps_and_latches_its_channel, a_tick_that_ends_exactly_on_the_period_wraps, idle_clock_and_large_batches_match_exact_phase |
| fast-never | every tick takes the wrap path | killed | each_timer_alone_counts_wraps_and_latches_its_channel, static_duty_latches_at_wrap_and_preserves_fractional_divider |
| wrap-subtract | `total % modulus` → `total - modulus` | killed | c6_twenty_bit_timer_and_duty_do_not_truncate_or_overflow, each_timer_alone_counts_wraps_and_latches_its_channel |
| wrap-overflow-zero | u64 overflow path returns 0 | killed | idle_clock_and_large_batches_match_exact_phase |
| wrap-no-raw | wrap does not set the timer raw bit | killed | each_timer_alone_counts_wraps_and_latches_its_channel, a_tick_that_ends_exactly_on_the_period_wraps |
| wrap-channel-timer-ne | latch channels on the other timers | killed | c6_twenty_bit_timer_and_duty_do_not_truncate_or_overflow, p4_channel_seven_and_its_gamma_ram, each_timer_alone_counts_wraps_and_latches_its_channel |
| active-never-cleared | configuration write only sets `active` bits | killed | idle_clock_and_large_batches_match_exact_phase, each_timer_alone_counts_wraps_and_latches_its_channel |
| active-never-set | configuration write only clears `active` bits | killed | static_duty_latches_at_wrap_and_preserves_fractional_divider, idle_clock_and_large_batches_match_exact_phase, p4_channel_seven_and_its_gamma_ram |

## Host instructions

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

Re-run on a rebuild of the same sources (byte-identical binaries), with the S3 hello added (`rerun` in that file): c6-contiki −4.56%, c6-energy-scan −0.80%, c6-hello −0.00%, c3-hello +0.00%, S3 hello −0.01% (2,499,123,158 → 2,498,785,451).

Screening ablation (host instructions, c6-contiki 10 s, one run each, same build command): the timer skip alone −2.43%, the cold wrap alone −1.26%, both −4.72%; each part carries part of the gain.

Host counters on c6-contiki (`smoke_hw.py`, `/usr/bin/time -l`, main `104d7bd1`, 1 warmup + 3 alternating pairs, load 20–23, [smoke-hw.json](smoke-hw.json)): user CPU 2.71 → 2.59 s (-4.4%, 0/3 higher), retired host instructions -4.74% (0/3 higher), elapsed host cycles -3.89% (0/3 higher). Two daytime user-CPU smokes, with an A/A control that moved by up to 5.4% at load 19–31, are superseded by the table below; their files are in the PR head `5cfee516`.

## CPU (maintainers' run)

Run by the maintainers, not by the author of the change, on November 22, 2026: `smoke_hw.py` from this folder, base main `d043ff1d`, new the PR head `5cfee516` merged into it (`686305aa`; the run's tree was `550fb5cf`, which adds only files under `docs/`), rustc 1.100.0, aarch64-apple-darwin, Apple M5 Max (18 cores), macOS 27.2.1. Each tree was built with `cargo build --release --bins` in its own worktree, the two paths of equal length. One warmup pair, then alternating pairs, 30 guest seconds. The instruction counts (stop lines) and console are identical in every run of every file. From the new tree's root:

```sh
ESP32SIM_ROM_DIR=<roms> FIXTURES='hello c3-hello c6-hello c6-contiki' \
  python3 -I docs/evidence/c6-ledc-tick/smoke_hw.py <out.json> 30 <pairs> <base-tree>/target/release target/release
```

Medians of user CPU; ranges are over the measured pairs.

| New arm | Fixture | Pairs | Base median s (range) | New median s (range) | User CPU | New slower | Host insns (higher) | Host cycles (higher) | 1-min load |
| --- | --- | ---: | --- | --- | ---: | ---: | ---: | ---: | --- |
| this change ([json](cpu-pairs-first.json)) | hello | 7 | 0.246 (0.245–0.250) | 0.250 (0.248–0.252) | +1.55% | 6/7 | +0.05% (7/7) | +2.14% (6/7) | 6.1–6.4 |
| this change ([json](cpu-pairs-first.json)) | c3-hello | 7 | 2.521 (2.491–2.708) | 2.536 (2.479–2.637) | +0.59% | 3/7 | +0.00% (5/7) | +0.49% (3/7) | 4.4–6.1 |
| this change ([json](cpu-pairs-first.json)) | c6-hello | 7 | 1.559 (1.543–1.585) | 1.579 (1.566–1.609) | +1.30% | 6/7 | -0.00% (2/7) | +1.33% (6/7) | 4.0–4.4 |
| this change ([json](cpu-pairs-first.json)) | c6-contiki | 7 | 3.426 (3.416–3.483) | 3.302 (3.242–3.349) | -3.64% | 0/7 | -4.96% (0/7) | -3.71% (0/7) | 3.1–3.9 |
| this change, repeat ([json](cpu-pairs-first-repeat.json)) | hello | 11 | 0.236 (0.231–0.244) | 0.240 (0.238–0.245) | +1.81% | 9/11 | +0.05% (11/11) | +2.07% (10/11) | 1.3–1.3 |
| this change, repeat ([json](cpu-pairs-first-repeat.json)) | c6-hello | 11 | 1.509 (1.492–1.642) | 1.505 (1.469–1.520) | -0.29% | 4/11 | -0.00% (4/11) | -1.09% (4/11) | 1.2–1.4 |
| A/A control: main `d043ff1d` again ([json](cpu-pairs-aa.json)) | hello | 11 | 0.237 (0.233–0.247) | 0.238 (0.234–0.245) | +0.30% | 5/11 | -0.00% (4/11) | +0.38% (6/11) | 1.0–1.0 |
| A/A control: main `d043ff1d` again ([json](cpu-pairs-aa.json)) | c6-hello | 11 | 1.476 (1.460–1.504) | 1.484 (1.449–1.562) | +0.55% | 6/11 | -0.00% (3/11) | +0.47% (6/11) | 1.0–1.4 |
| this change ([json](cpu-pairs-first-pocket-tank.json)) | pocket-tank | 7 | 38.255 (38.147–38.665) | 38.200 (38.007–38.971) | -0.14% | 3/7 | -0.00% (2/7) | +0.10% (3/7) | 1.3–2.1 |

c6-contiki is the fixture here that configures the LEDC, and the only one that runs the changed code (the probe above): −3.64%, slower in 0 of 7 pairs, host instructions −4.96%.

The S3 hello is slower, and that reproduced: +1.55% (6/7), then +1.81% (9/11), host cycles +2.07% (10/11), against an A/A control of +0.30% (5/11). That is about 4 ms of 0.24 s. The LEDC tick never runs on that fixture, so this is not executed LEDC code; the cause is not established. Code layout is possible and not demonstrated, and the 0.05% more host instructions in every 30 s pair (7/7 and 11/11) are unexplained. c6-hello's +1.30% (6/7) did not repeat (−0.29%, 4/11; A/A +0.55%, 6/11). c3-hello is +0.59% (3/7), with overlapping ranges and no A/A control of its own. pocket-tank is flat, −0.14% (3/7).

A variant with an early return on `active == 0` was also built and measured (1 warmup + 11 pairs: S3 hello +0.65%, 6/11; c6-contiki −3.55%, 1/11); it was not adopted, because the tick is never called on the S3 hello, so the early return cannot explain the S3 hello difference.

## Limits

The claim is c6-contiki: user CPU −3.64% (0/7 slower, main `d043ff1d` against the PR head merged into it, rustc 1.100.0), retired host instructions −4.55% in one 10 s run (−4.74% on `104d7bd1`) and c6-energy-scan −0.81% (−0.89%). The S3 hello is slower by +1.81% (9/11) for a reason not established, see above; it and the other fixtures in the table do not run the changed code. c6-energy-scan, the P4 and the classic are not in the CPU table. One machine. A firmware whose LEDC timer wraps on nearly every round (a PWM period of a few dozen APB cycles) now pays a call per wrap; no fixture here does. Native only, not a browser or hardware claim.

## Checks

On `686305aa` (the PR head merged into main `d043ff1d`), rustc 1.100.0: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings` pass; `ESP32SIM_ROM_DIR=… cargo test --release --workspace --no-fail-fast -- --include-ignored --skip external_` 1809 passed, 0 failed, including the `scintilla_p4_lcd35` golden; `node tools/check-evidence-privacy.mjs` pass; `git diff --check` clean. No goldens changed.

At the code commit, on `104d7bd1`: `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings` and the wasm32 `esp32sim-wasm --features jit-tests` clippy pass; `ESP32SIM_ROM_DIR=… cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_` 1371 passed, 0 failed (empty `HOME`); plain `cargo +1.99.0 test --release --workspace` 1315 passed, 0 failed, 86 ignored; `tools/wasm-build.sh` + `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel` ok; `node tools/check-evidence-privacy.mjs` pass; `git diff --check` clean. No goldens changed.
