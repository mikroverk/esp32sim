# EX269: a JIT for the RV32 decoded blocks, measured on the ESP32-S31

The JIT compiles the blocks of [EX265](../../experiments.md#ex265). `Core::run` runs blocks only on
a bus that versions its code (`Bus::page_versions`), and in this repository that is the ESP32-S31's.
The C3, C6 and P4 buses have no code versions: a probe at the start of `block::run` is reached by
s31-hello and never by c3-hello, c6-hello or p4-hello (3 s runs of `2c6bc4aa`). Those chips are
controls here, they do not run the JIT.

## Builds

| Name | Commit | What |
| --- | --- | --- |
| main, main-again | `3f49dad8` | the base, built in two checkouts (the A/A control: two binaries of one source) |
| on, off | `ef299b26` | the JIT as it ends up, `off` with `RV32_JIT=0`; later commits change tests, CI, docs, this folder and, in `eaaf051f`, the cold compile path (`jit_compile`: a refused module no longer discards other code), which was not timed again |
| off-before | `830696f2` | the JIT as first proposed (`2c6bc4aa`) with the same main merged, `RV32_JIT=0` |
| v2-off, v2-on | `ef299b26` + `separate-loop.patch` (`git apply --unidiff-zero`) | not adopted: the block loop compiled twice, with and without the JIT |

`cargo build --release --bins`, rustc 1.100.0 (0ef1bc386); SHA-256 of every binary in the JSON.
Apple M5 Max (18 cores), macOS 27.2.1.

## Workloads

`pairs.py` holds the commands. s31-mosaico is the page's demo (`web/wasm/fw/public/s31-mosaico-demo*`)
with the script of its golden, 7.5 emulated s; the hellos run 30 s. Inputs are the committed images
and the mask ROM ELFs of `tools/fetch-rom-elfs.sh` (s31 ROM
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`, demo app
`ec2c2f899836671907943a3ee55e2d06ec8c16a601a80596a8e8656bba25d00d`, s31-hello app
`6b965db02941e8db4895d27c2ad8eea1733e22ddf9a82ccd4d6f77ae62c6335c`). In every series all sides have
the same guest instruction counts and console SHA-256 (`insns`, `console_sha256`).

## Method

`pairs.py`: one warm-up run of every side, then rounds of one run per side, the order reversed every
other round; percentages are medians of the per-round ratios to `main`. The 1-minute load average is
below 2.5 before a round and below 3 before and after every run; a round that misses that is
discarded and listed (`discarded_rounds`). User time has a resolution of 10 ms, so the S3 hello
(0.25 s) is read from host cycles. Other work ran on the machine; the limit, not the host, was quiet.

## Results

| Workload | Series | main-again (A/A) | `RV32_JIT=0` | JIT on | load | discarded |
| --- | --- | --- | --- | --- | --- | --- |
| s31-mosaico | 1, 7 rounds | +0.47% (6/7 slower) | +1.57% (7/7) | **-23.43%** (0/7) | 2.25-2.91 | 0 |
| s31-mosaico | 2, 7 rounds | +0.81% (4/7) | +1.29% (5/7) | -23.28% (0/7, v2-on) | 2.29-2.94 | 2 |
| s31-mosaico | 3, 7 rounds | -0.31% (2/7) | +0.31% (4/7) | | 2.33-2.84 | 3 |
| s31-hello | 7 rounds | +0.63% (4/7) | +0.21% (4/7) | -2.05% (1/7) | 1.81-2.42 | 0 |
| c3-hello (control, no JIT) | 9 rounds | +2.04% (6/9) | | +0.79% (5/9) | 1.63-2.87 | 1 |
| c6-hello (control, no JIT) | 9 rounds | +0.00% (4/9) | | +0.00% (4/9) | 2.28-2.66 | 0 |
| s3-hello (control, Xtensa) | 9 rounds | cycles -0.66% | | cycles +0.41% | 2.35-2.38 | 0 |

s31-mosaico: main 6.27 s, JIT on 4.87 s (series 1). Host instructions (they repeat to 0.01% on one
binary): s31-mosaico on -17.27%, off **-0.80%**, off-before +0.68%, v2-off +1.46%, A/A 0.00%;
s31-hello on -1.02%, off -0.07%; c3-hello and c6-hello +0.01% and +0.02%.

**What `RV32_JIT=0` costs the S31.** s31-hello: nothing measurable. s31-mosaico, over the 21 rounds
of the three series: user time +0.96% (median of 21 ratios, 16/21 slower), A/A +0.32% (12/21),
against the second main build +0.80% (17/21), host cycles +1.01%, A/A +0.11%. So about 0.6 to 0.8
points above the A/A control remain, while 0.8% fewer host instructions retire. It is smaller than
`off-before` (+1.85%, 7/7, in the same rounds as series 3), and not shown to be zero. Its cause is
not established: c3-hello, which runs none of this code, differs +2.04% between two builds of the
same source. An earlier reading in the review, that the layout of `Blocks`/`Cpu` is the cause, had no A/A
control and is withdrawn. v2 (a loop of its own without the JIT) retired 1.46% more instructions than
main and was no faster, so it was not adopted.

**Node** (`node_pairs.py`, Node v26.0.0, `tools/wasm-test.mjs s31-mosaico`, 5 pairs, load < 3): wall
-10.45% (0/5 slower; one more pair discarded for its load, listed), same instruction, console and frame counts; main and new modules' SHA-256 in
`node-pairs.json`. Not measured in a browser.

**Memory** (`/usr/bin/time -l`, `ef299b26`, s31-mosaico): maximum RSS 85 MB with `RV32_JIT=0`; 143 MB on
at 7.5 s (13,142 regions, 46 MB of code in the two cores) and 149 MB at 20 s (22,334 regions compiled,
43 MB held, so a 32 MiB cache has started again). Replaced code is freed only by that restart. The
instruction tables helpers read (16 bytes per instruction) are outside the code caps.

**Refused modules** (`wasm_fuzz.py`, seeds 0-39, 320 programs; compile attempts, before and after
`eaaf051f`): `REFUSE=1` 1,161,634 and 48,143; `REFUSE=3` 181,261 and 39,964, all programs equal to the
interpreter. Before it, every refusal discarded all of the core's code and its marks.

**Not usable:** `pairs-loaded.json` is a series whose s31 rounds ran at load up to 5.1 (runs were
checked only before a round); it is kept, and not used above. Two further series were stopped unfinished while the
machine was busy and their numbers not kept. The figures of the first version of EX269 (c3-hello -0.10%, c6-hello
+1.44%, and the two earlier 3-run series it kept) compared programs that never run the JIT or the block loop.

## Correctness

- `blocks_differential` (`riscv-rv32/tests`), JIT on and `RV32_JIT=0`, `RV_BLOCK_SEEDS=4000`:
  32,000 programs, 9,600,000 run calls, 2,503,937 regions compiled (37.5 M + 450 M instructions, the
  latter in the loop programs), all equal to the interpreter.
- A separate fuzz (scratch, not retained): 20,000 programs on three kinds of core (S31, C6, P4),
  4,000,000 run calls, 1,577,400 regions, 2.9 G instructions: no difference.
- The wasm backend (`wasm_fuzz.py`, Node): 2,400 programs, 720,000 run calls, 182,071 regions, no
  difference; every module released at the end. With the byte cap lowered to 20,000 (scratch) the
  cap was never exceeded (peak 19,996 bytes) and results did not change.
- With the AArch64 code cache lowered to 48 KiB (scratch) the differential passes too: full caches.
- `mutate.py`: 42 of 43 mutants of the AArch64 backend and the dispatch are killed (`mutants.txt`).
  The one that survives, region reuse that ignores a block's length, is equivalent on a bus that keeps
  the version contract. 5 of 5 mutants of the wasm backend were killed (scratch).
- The workspace tests, the S31, C3, C6 and P4 goldens and the P4/C6 co-simulation pass, with and
  without the JIT (`RV32_JIT=0`), goldens bit-identical.

## What it does not cover

One machine (Apple M5 Max, macOS). No Linux AArch64 host, no x86 build (no backend there), no
browser. Two binaries of one source differ by up to 2% here, so differences of 1 to 2% are not
evidence. sbc-emu's own figures for the same code generators are in the message of its commit
`509f35de` (S31 Linux boot, 5.5 G steps, native 13.6 s to 10.2 s, WebAssembly to the shell 10.1 s to
7.6 s); they were not measured here and have no receipt.
