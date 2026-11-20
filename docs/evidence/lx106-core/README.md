# LX106 interpreter profile

`Lx106` is a separate interpreter-only core type. It shares the integer decoder and
executor but does not enter the LX6/LX7 block caches or JIT. The existing `Cpu` type
and its execution paths remain the default for the existing chips. This is not an
ESP8266 machine, ROM boot, SDK-compatibility or hardware-conformance claim.
External-register operations return an unimplemented trap; protection/TLB operations
retain the existing executor's identity/no-effect behavior. Full debug watchpoints
and hardware validation are outside this slice.

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
nice -n 10 cargo +1.99.0 test --release -p xtensa-lx7 --test lx106
```

The 15 tests cover arithmetic, call0, literals, interrupt and exception entry,
timer and WAITI behavior, alignment, unsupported instructions, and a 256-register
by three-direction matrix. The latter checks illegal raw instructions preserve
register state before exception effects. The tests also check that JIT requests
cannot bypass the interpreter profile.

## Mutations and static code generation

`mutations.json` lists 33 compile-valid mutants with the first failing test and
assertion; each was killed. Reproduce in a disposable worktree and target directory:

```sh
CARGO_TARGET_DIR="$TARGET" python3 docs/evidence/lx106-core/mutate.py "$SCRATCH/mutations"
python3 docs/evidence/lx106-core/codegen.py "$BASE_TARGET/release" "$HEAD_TARGET/release" "$SCRATCH/codegen"
```

The codegen comparator uses the established EX230 normalizer and requires linked
native release binaries built identically. It is not a timing test. No raw captures
or identifying environment data are retained in this evidence directory.

## CPU comparison

PENDING.
