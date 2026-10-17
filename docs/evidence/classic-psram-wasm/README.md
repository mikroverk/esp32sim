# Classic PSRAM native/WASM qualification: bounded success, unresolved JIT validation

Base: accepted P6 `805b886e028dcc91c92bdf7c4bb2f56928cd3921`.
Implementation: `dea5cbc73b9995367b9a5d71f08d1a13b68039be`;
diagnostic/result retention and production-settings probe support:
`2e322af2b1d2916746e82edb18197846cef02826`.

Status: NOT fully qualified. Production JIT settings pass the bounded fixture
matrix, but the stronger `jit-tests` build reproducibly aborts after classic
same-machine reboot. Do not treat the production-settings pass as proof that the
inconsistent compiled-region metadata is harmless. No JIT implementation change
or blanket disable is included here.

## Change and scope

The classic WASM factory now passes its existing PSRAM-size argument through the
existing validated bus configuration method. Only 0 (absent) and 4 MiB are
supported; invalid classic sizes return null with a diagnostic instead of being
silently ignored. There is no new memory-size API and the default remains absent.
No bus, peripheral, translator, cache, scheduler or CPU implementation changes
are made. No fast-memory pointer is introduced. No new per-instruction work is
added; the production change is confined to machine construction.

Qualification-only exports are excluded from a normal module. `psram-tests`
enables just these exports; `jit-tests` additionally enables the existing JIT's
internal validation. The reboot export calls the real `Machine::reboot` path
used by the software-reset handler, at a stopped-machine slice boundary. It does
not simulate a browser reset button or intercept a guest firmware API.

## Observed evidence

`production-guests.json` contains 24/24 native/actual-WASM boot comparisons:
12 successive machine creations within one WASM instance, two boots per machine,
JIT off/on, P5/P6 present and absent, and repeated present configurations. Every
fixture `P5 ` or `P6 ` serial line is byte-identical across targets, modes, reset
and repeated construction for the matching configuration. The hash covers those
bounded result lines joined with LF, not the SDK/ROM timestamped boot log. No
claim of identical native/WASM scheduling quanta or total instruction counts is
made. JIT-enabled cases verify positive retired compiled-instruction counters
on both targets; disabled cases verify zero.

Both genuine guest 1 MiB readbacks report zero errors and independent FNV-1a
`20c19dc5` / `9dce9dc5`, including free/reallocation and reset. P5 keeps its
accepted IDF 4.4.7 accounting: Arduino size/free 4192123, heap 4194303. P6 keeps
Arduino size/heap 4194304 and free 4192124; its split heap-add and duplicate-state
serial outcomes are unchanged. Absent-device cases cannot allocate SPIRAM.
The exact retained stock firmware inputs match the P5/P6 committed provenance;
`inputs.json` lists the six loaded images and mask ROM hashes. No SDK rebuild or
new firmware binary is needed for this cross-target qualification.

The same target-independent assertion function passes natively and in real WASM.
It covers all 128 reversed 32 KiB mappings, byte/halfword/word accesses, independent
SPI-command physical-page readback, physical remap/alias, a noncontiguous page
crossing, atomic rejection of invalid crossings, bounds, cache disable/mask and
restore. Reboot invalidates both complete SRAM tables; remapping alone leaves
cache/device controls forbidden; re-enabling and reading every word across all
4 MiB returns zero. Removing/re-adding the configured device invalidates mappings
and bytes. Repeated 4/0/4/0 MiB factory creation is isolated. Classic `fast_mem`
is absent before and after reset: no published raw PSRAM pointer exists to retain.
This is a functional coherence/lifecycle test, not cache-line or silicon timing.

`controls.json` compares normal production modules against accepted P6, using
three seconds of no-PSRAM classic/S3 hello with JIT off/on. All four snapshots
match exact console SHA-256, byte length, cycles and instruction counts. The 55
tracked golden files and all S3/core/bus/peripheral production sources remain
unchanged.

## Unresolved negative: extra JIT validation on reboot

The `jit-tests` module passes the bus/lifecycle function and the first P5 present
boot. After reboot its second run aborts in
`xtensa-lx7/src/jit/wasm.rs:186`, `Hot::assert_matches`:

    cached (slot, chunk, length, copy): (1268, 3, 4, 0)
    selected owner:                   (1628, 1, 4, 0)

The full run stops after 13 completed comparisons out of 24 planned; the remaining
11 are not reported as passes. `asserted-guests.json` retains those completed
results with `complete:false`. Isolated P5 absent and P6 absent also abort on the
second boot, with mismatches `(1617,15,2,0)` vs `(1614,9,2,0)` and
`(1289,3,2,0)` vs `(1203,1,2,0)` respectively. Isolated P6 present passes four
comparisons, including two successive machines and their second boots.
`jit-negative.json` records the module identity and assertions.

Explanation, not an established root cause: the live cached hot-region facts
and the region chosen by the diagnostic owner lookup disagree after reset.
The same failure with an absent device means actual PSRAM backing is not needed.
All 24 bounded fixture comparisons pass when only the additional internal JIT
validation is off, but that does not establish whether the cached-owner mismatch
is a stale/unsafe admission fact or an overstrong assertion about overlapping
regions. No production byte-corruption, silicon issue, production-user recovery,
or baseline same-machine JIT-reset result is claimed. Resolve this separately and
repeat the asserted fixture matrix before declaring complete acceptance.

## Reproduce

Build the unchanged public P5/P6 recipes when retained firmware is unavailable:
`docs/fixtures/classic-psram-arduino2/recipe.sh OUTPUT2` and
`docs/fixtures/classic-psram-arduino3/recipe.sh OUTPUT3`.
Set `ARDUINO2_FIRMWARE_DIR=OUTPUT2/fw-A2`,
`ARDUINO3_FIRMWARE_DIR=OUTPUT3/fw-p6`, `ESP32SIM_ROM_DIR` to the fetched mask ROMs,
and `CARGO_TARGET_DIR` to an isolated target directory. Use four build jobs and
nice all builds. The normal build helper expects `target` to refer to that target.

    nice -n 10 cargo +1.99.0 build --release -p esp32sim-wasm --features psram-tests --example psram-qualification
    # WASM: in a subshell, source tools/wasm-rustflags.sh, set RUSTC from rustup
    # and DYLD_FALLBACK_LIBRARY_PATH to the toolchain lib directory on macOS.
    nice -n 10 cargo +1.99.0 build --release --target wasm32-unknown-unknown -p esp32sim-wasm --features psram-tests
    PSRAM_RESULTS=results.json nice -n 10 node tools/psram-wasm-test.mjs

Repeat the WASM build with `--features jit-tests`, then repeat the Node command.
Expected unresolved result: nonzero exit and the assertion above. To isolate it,
set `PSRAM_CASE=p5:4:1`, `p5:0:1`, or `p6:0:1`. `ESP32SIM_WASM` selects a frozen
module, avoiding interference from another build overwriting the target artifact.
Run `node tools/psram-wasm-test.mjs --lifecycle` for just the identical native/WASM
bus assertions. In a disposable checkout, `python3 tools/psram-wasm-mutations.py
OUTPUT` rebuilds and assertion-checks each mutation on both targets. It restores
the source after every probe; build failures do not count as kills.

For normal-module controls, run `ESP32SIM_WASM=module.wasm node
tools/psram-wasm-controls.mjs output.json` for baseline and candidate and compare
the `results` arrays. `ESP32SIM_ROM_DIR` must also contain the committed hello
fixture paths as fetched by `tools/fetch-demo-assets.sh --no-linux`.

## Standard checks

Both required Clippy gates pass with warnings denied. Required workspace suites
pass with an isolated empty HOME: 1006 ROM-enabled tests; 976 plain tests and 58
ignored. Normal WASM build, eight manifest demos and shared-memory JIT handoff
pass. The optional full JIT differential run passes 111125 cases and releases
99677 compiled modules. Its shell wrapper's separate waveform process initially
read a concurrently replaced non-test artifact; rerunning that process against
the frozen original test module succeeds. This artifact-selection mistake is
separate from the reproducible guest reboot assertion. `checks.json` records the
individual outcomes and the mutation table is in `mutations.json`.

## CPU comparison

PENDING. No CPU benchmark or timing table was run. Only the factory changes on
the production path; the hot instruction paths are unchanged.

## Limits

Local feature/qualification work only. No publication, product/add-on changes,
new upstream issue, PSRAM camera integration, JPEG, arbitrary classic PSRAM sizes,
BOOT_INIT=y, fresh stock SDK rebuild, full dual-core cache fidelity, external DMA,
cycle-accurate cache model, or hardware comparison is claimed. Input firmware is
public fixture material, not a user sketch. No production event query, identity
export or replay was used.
