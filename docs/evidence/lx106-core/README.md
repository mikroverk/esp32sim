# LX106 interpreter profile

`Lx106` is a separate interpreter-only core type. It shares the integer decoder and
executor but does not enter the LX6/LX7 block caches or JIT. The existing `Cpu` type
and its execution paths remain the default for the existing chips. This is not an
ESP8266 machine, ROM boot, SDK-compatibility or hardware-conformance claim.
External-register operations return an unimplemented trap; protection/TLB operations
retain the existing executor's identity/no-effect behavior. Full debug watchpoints
and hardware validation are outside this slice.

Nothing selects `Lx106` yet. The follow-up that wires it into a machine is the
ESP8266 machine of [issue #214](https://github.com/mikroverk/esp32sim/issues/214):
bus, ROM boot and a CLI chip. That machine also needs a way for bus errors to raise
LoadStoreError (EXCCAUSE 3), which the Arduino core's `core_esp8266_non32xfer.cpp`
relies on for byte and halfword IRAM accesses (the shared load/store path raises
LoadProhibited/StoreProhibited), and a register dump without the LX6/LX7 `wb`, `ws`
and `lcount` fields that `regs`, `trace_regs`, `regtrace_line` and `dump` now
inherit from `Cpu`.

## Sources and rules

The configuration comes from Arduino ESP8266 3.1.2, revision
`210897ef83305496947c4e73c937bab52a33cb48`,
`libraries/GDBStub/src/xtensa/config/core-isa.h`, checked against PlatformIO
`toolchain-xtensa` 2.100300.220621. Header option lines 49–94, alignment lines
110–113, CONFIGID lines 126–127, interrupt/timer lines 260–343 and vector lines
379–419 specify the profile. Reset address `0x40000080` comes from `_ResetVector`
in `tools/sdk/ld/eagle.rom.addr.v6.ld`. Source hashes and specific options are in
[sources.json](sources.json); CONFIGID is a header value, not a hardware observation.

The 16-register call0 profile has density, absolute literals, MUL16 and MULL but
no windowed registers, optional divide, loops, floating point or coprocessors.
There is one compare timer, fifteen interrupt lines and profile-specific exception
vectors. Unaligned halfword and word data accesses trap before the access.

[The special-register direction audit](sr-direction-audit.md) cites the Xtensa
manual and QEMU sources. Direction checks precede register effects. RSR INTCLEAR
has an undefined result in the manual; this profile deliberately rejects it, not
a claim that hardware must trap. Source and assembler comparisons do not replace
hardware measurements.

## Corpus and tests

The repository-owned assembly corpus and generated Rust fixtures contain no ROM,
SDK, firmware, or third-party runtime. The positive corpus uses GNU as 2.32 from
PlatformIO toolchain-xtensa 2.100300.220621; foreign instructions are assembled
with GNU as 2.35.1.20201223 from toolchain-xtensa-esp32s3 8.4.0+2021r2-patch5,
then their raw bytes reassembled with the LX106 toolchain. The manifest pins file
hashes. Eleven extra raw special-register direction negatives do not use
assembler acceptance as a legality oracle. To regenerate with the two executable
prefixes supplied by the caller:

```sh
python3 xtensa-lx7/tests/corpus/build-lx106.py --lx106-prefix "$LX106_PREFIX" --s3-prefix "$S3_PREFIX" --out "$SCRATCH/asm"
nice -n 10 cargo test --release -p xtensa-lx7 --test lx106
```

The 19 tests cover arithmetic, call0, literals, interrupt and exception entry,
timer and WAITI behavior, BREAK/BREAK.N debug causes and masking, alignment,
unsupported instructions, RER/WER traps, and a 256-register by three-direction
matrix. The latter checks illegal raw instructions preserve register state before
exception effects. The tests also check that JIT requests cannot bypass the
interpreter profile.

## ISA membership

The positive list in `supports()` is checked against the LX106 opcode table that
QEMU v10.1.0 ships (`target/xtensa/core-lx106/xtensa-modules.c.inc`, `opcodes[]`
lines 5823–6435). `LX106_OPS` in `tests/lx106.rs` holds one encoded instance of
every listed op with its table line; one test decodes and steps each without
IllegalInstruction, and another decodes all 2^24 three-byte words and fails if
`supports()` accepts an op outside the table. EXTW, the LX3-era suspect, is in the
table (line 6091). The bytes come from GNU as 2.43.1 with the `xtensa_esp8266.so`
configuration of Espressif's esp-14.2.0 multi-config toolchain, assembled from
[isa-membership.S](isa-membership.S); the same assembler rejects the extra
negatives (MAC16 `mula.dd.ll`, `dhi`, `ihi`, `dpfr`, `salt`, `saltu`, PIE
`ee.vadds.s8`), which are hand-encoded raw bytes in the test because the pinned
corpus toolchains are not available to regenerate `lx106-forbidden.S`. SIMCALL is
in the table but illegal on pre-RE.3 hardware. Sources, hashes, commands and the
table opcodes the profile leaves out are in [isa-membership.json](isa-membership.json).

## Mutations and static code generation

`mutations.json` lists 42 compile-valid mutants, each with the first failing test
in name order (one test thread), the source line of its failing assertion, the panic
message and every failing test; each was killed. Besides the forbidden-option, SR,
vector, interrupt and alignment mutants, five remove ops from the positive list (EXTW,
BBCI, L8UI, SYSCALL, NSA), one adds SALT, two undo the BREAK.N cause and debug-level
masking, and one lets RER/WER fall back to the generic executor. The file is the
script's unedited output for this tree's `lx106.rs` and tests, with rustc 1.100.0. Reproduce in a disposable
worktree and target directory:

```sh
CARGO_TARGET_DIR="$TARGET" python3 docs/evidence/lx106-core/mutate.py "$SCRATCH/mutations"
python3 docs/evidence/lx106-core/codegen.py "$BASE_TARGET/release" "$HEAD_TARGET/release" "$SCRATCH/codegen"
```

The codegen comparator uses the established EX230 normalizer and requires linked
native release binaries built identically. It is not a timing test. No raw captures
or identifying environment data are retained in this evidence directory.

## CPU comparison

The primary evidence that the existing chips are unaffected is static: outside the
new module the only change is `exec::control_event` becoming `pub(crate)`, and
[codegen.json](codegen.json) finds all 126 normalized hot symbols identical in
esp32sim, esp32sim-c3 and esp32sim-c6 (base `04f07602`, head `3c976953`). No binary
instantiates `Lx106`; later commits on this profile touch only `lx106.rs`, its tests
and this directory.

The maintainers also ran the usual CPU pairs and posted them in
[the PR #337 review](https://github.com/mikroverk/esp32sim/pull/337#issuecomment-6599264805);
these are their measurements, not a rerun. `cpu_pairs.py` from `docs/evidence/p4-cpu`
as on `04f07602`, plus a `classic-hello` workload (`--chip esp32 --board esp32dev`
with the repository's classic hello images; script SHA-256 prefix `c9e8ca4b…`).
New = `3c976953` (this PR's head), base = `04f07602` (its base). Both trees were
extracted with `git archive` into sibling directories with paths of the same length
and built with `cargo build --release --bins`, rustc 1.99.1 (58cf8f9c3 2026-10-13),
aarch64-apple-darwin. Machine: Apple M5 Max (18 cores), macOS 27.0.1. Each workload
started with the 1-minute load below 5 and nothing building. ROMs: esp-rom-elfs
20260528 (`tools/fetch-rom-elfs.sh`). One warmup pair, then 7 alternating pairs per
workload, from the base tree's root. User CPU comes from rusage at 0.01 s
resolution, so one tick is 2.5–4% of these workloads.

| workload | base median (range) | new median (range) | change | new slower | instructions, console | load before → after |
| --- | --- | --- | --- | --- | --- | --- |
| s3-hello (30 s) | 0.250 s (0.240–0.250) | 0.250 s (0.240–0.260) | +0.0% | 3/7 | identical in all 16 runs | 4.30 → 4.35 |
| classic-hello (30 s) | 0.390 s (0.380–0.390) | 0.380 s (0.380–0.390) | -2.6% | 3/7 | identical in all 16 runs | 4.35 → 4.41 |

Nothing is flagged (the flag is new slower in at least 6 of 7 pairs, or ranges that
do not overlap). Instruction counts and console hashes are identical in every run.

The [posted JSON](337-3c976953-04f07602.json) is preserved byte for byte as posted,
without a final newline: SHA-256
`d17c5e14adce7cee40d8eb5fed1fec0604ecde4051c882e8fc985704148b7638`, matching the
sanitized prefix `d17c5e14…` in the review. The only redaction is the ROM directory,
written as `$ESP32SIM_ROM_DIR`; the review gives the raw file's SHA-256 prefix as
`209e04a7…`. The redaction does not change measured values.
