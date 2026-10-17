# Classic PSRAM native/WASM qualification

Base: accepted P6 `805b886e028dcc91c92bdf7c4bb2f56928cd3921`.
Factory/lifecycle implementation: `dea5cbc73b9995367b9a5d71f08d1a13b68039be`;
production-settings probe support: `2e322af2b1d2916746e82edb18197846cef02826`.
JIT ownership/reset fix: `291bc1371fffd81a30f7f521a88d47c114487768`;
minimized regression: `b252084923f51e53a4e513a5d8ffe86621d3523f`.

The bounded native/WASM fixture qualification passes with both production JIT
settings and the stronger `jit-tests` validation. The earlier reboot assertion
is resolved, not ignored or bypassed. The implementation tree at `b2520849` is
identical to reviewed remediation `6d2abbbee23bb97819e84e6c906b3de7bf882220`.

## Change and scope

The classic WASM factory passes its existing PSRAM-size argument to the existing
validated bus configuration method. Only 0 (absent) and 4 MiB are supported;
invalid classic sizes return null with a diagnostic instead of being silently
ignored. The default remains absent. No new memory-size API or raw PSRAM pointer
is introduced. No bus, peripheral, scheduler or S3 implementation change is
needed by this qualification.

The separate JIT change validates cached Hot fields against their selected live
region, not the coverage map's preferred owner. All Hot field checks and the
epoch guard remain; the selected chunk PC is also checked. Cold cache reset
clears owner-index hints before rebuilding coverage after compaction. Neither
production hot admission nor generated instruction bodies are changed. AArch64
uses a separate linear native cache without this WASM Region/Hot ownership map.

Qualification-only exports are excluded from a normal module. `psram-tests`
enables just these exports; `jit-tests` additionally enables internal JIT
validation. The reboot export invokes the real `Machine::reboot` path at a
stopped-machine slice boundary. It does not emulate a browser reset button or
intercept a guest firmware API.

## Observed evidence

`fixed-asserted-guests.json` and `fixed-production-guests.json` each contain
24/24 native/actual-WASM boot comparisons: 12 successive machine creations
within one WASM instance, two boots per machine, JIT off/on, P5/P6 present and
absent, and repeated present configurations. Every fixture `P5 ` or `P6 ` serial
line is byte-identical across targets, modes, reset and repeated construction
for the matching configuration. Hashes cover those bounded result lines joined
with LF, not timestamped SDK/ROM logs. JIT-enabled cases retire compiled
instructions on both targets; JIT-disabled counters are zero. No equality of
native/WASM scheduling quanta or total instruction counts is claimed.

Both genuine guest 1 MiB readbacks report zero errors and FNV-1a `20c19dc5` /
`9dce9dc5`, including free/reallocation and reset. P5 preserves the accepted IDF
4.4.7 accounting: Arduino size/free 4192123, heap 4194303. P6 preserves Arduino
size/heap 4194304 and free 4192124, plus its split heap-add and duplicate-state
outcomes. Absent-device cases cannot allocate SPIRAM. The six retained stock
public firmware images and mask ROM were rehashed against `inputs.json`; no SDK
rebuild or new firmware binary is needed for this cross-target qualification.

The identical target-independent lifecycle assertions pass natively and in
actual WASM. They cover all 128 reversed 32 KiB mappings; byte/halfword/word
accesses; independent SPI physical-page readback; remap/alias; noncontiguous
page crossing; atomic invalid-crossing rejection; bounds; cache disable/mask
and restore. Reboot invalidates both complete SRAM mapping tables; remapping
alone leaves cache/device controls forbidden. After re-enabling, every word
across 4 MiB reads zero. Removing/re-adding the configured device invalidates
mappings and bytes. Repeated 4/0/4/0 MiB factory creation is isolated. Classic
`fast_mem` is absent before and after reset, so no published raw PSRAM pointer
can survive. This is functional coherence, not cache-line or silicon timing.

All four no-PSRAM classic/S3 JIT-off/on console/work snapshots equal accepted
P6: exact serial SHA-256, byte length, cycles and instruction counts. This holds
for the normal production module, production-setting qualification module and
asserted module. All 55 tracked golden files remain unchanged.

## Retained pre-fix negative and explanation

`1ea5e3f879f9af0312b019370ef7d990d8791ac6` passes the production-setting
matrix but fails extra JIT validation after reboot. Its asserted matrix stops
after 13 of 24 planned comparisons; those unexecuted cases were not passes.
The second P5-present boot reports cached `(1268,3,4,0)` versus preferred
`(1628,1,4,0)`. P5/P6 absent-device configurations also reproduce the failure.
`jit-negative.json`, the original partial `asserted-guests.json`, original
`checks.json`, `production-guests.json`, `controls.json` and `mutations.json`
are retained pre-fix records. They do not describe the fixed qualification;
current results are the explicitly `fixed-` prefixed files.

The minimized regression establishes why a live overlapping selected region
need not be the preferred owner: coverage insertion retains the first indexed
owner, while a valid existing hint may still select another covering region.
The previous diagnostic compared cached facts belonging to that selected region
against a fresh preferred-owner lookup. This was an overstrong assertion, not
proof of freed/stale production execution. Reset reindexing separately requires
clearing old owner indices. The regression checks guest register/PC results and
module release, and both ownership mutations fail the intended WASM assertions.
Those ownership mutants are inapplicable to the native linear cache; native
coverage comes from the guest matrix and dual-target PSRAM lifecycle mutants.

## Reproduce

Build the public P5/P6 recipes if retained firmware is unavailable:
`docs/fixtures/classic-psram-arduino2/recipe.sh OUTPUT2` and
`docs/fixtures/classic-psram-arduino3/recipe.sh OUTPUT3`.
Set `ARDUINO2_FIRMWARE_DIR=OUTPUT2/fw-A2`,
`ARDUINO3_FIRMWARE_DIR=OUTPUT3/fw-p6`, `ESP32SIM_ROM_DIR` to fetched mask ROMs,
and `CARGO_TARGET_DIR` to an isolated target. Nice builds and use bounded jobs.
The build/JIT shell helpers require `target` to refer to that target directory.

    nice -n 10 cargo +1.99.0 build --release -p esp32sim-wasm --features psram-tests --example psram-qualification
    # WASM: source tools/wasm-rustflags.sh in a subshell; set RUSTC via rustup
    # and DYLD_FALLBACK_LIBRARY_PATH to the toolchain lib directory on macOS.
    nice -n 10 cargo +1.99.0 build --release --target wasm32-unknown-unknown -p esp32sim-wasm --features psram-tests
    PSRAM_RESULTS=results.json nice -n 10 node tools/psram-wasm-test.mjs

Repeat both builds with `--features jit-tests`, then repeat the full Node
command. Both matrices must now exit successfully. `ESP32SIM_WASM` selects a
frozen module to avoid concurrent artifact replacement. `--lifecycle` runs just
the shared lifecycle assertions. In a disposable checkout,
`python3 tools/psram-wasm-mutations.py OUTPUT` rebuilds and assertion-checks
each semantic mutant on both targets; compile failures do not count as kills.
The normal JIT suite includes the minimized owner/reset regression.

For no-PSRAM controls, run `ESP32SIM_WASM=module.wasm node
tools/psram-wasm-controls.mjs output.json` on baseline and candidate and compare
`results`. ROM input must also contain the committed hello fixture paths as
fetched by `tools/fetch-demo-assets.sh --no-linux`.

## Standard checks

Both required Clippy gates pass with warnings denied, as does the native
`psram-tests` qualification Clippy gate. Workspace tests with isolated empty
HOME pass: 1006 ROM-enabled tests; 976 plain tests, 58 ignored. The normal WASM
build, eight manifest demos and shared-memory JIT handoff pass. The literal
`tools/wasm-jit-test.sh` completes both phases: 111126 differential cases / 99679
compiled modules released, then 24 waveform cases / 738 modules released.
`fixed-checks.json` and `fixed-mutations.json` record current qualification,
semantic mutation builds/assertion failures and control comparisons. Privacy
pattern checks and manual review supplement, rather than prove, privacy.

## CPU comparison

PENDING. No benchmark or timing table was run. The only production JIT delta is
on cold reset; production hot bodies remain unchanged by the standalone fix.
No performance claim is made.

## Limits

Local feature/qualification work only. No publication, product/add-on changes,
new upstream issue, PSRAM camera integration, JPEG, arbitrary classic PSRAM
sizes, BOOT_INIT=y, fresh stock SDK rebuild, full dual-core cache fidelity,
external DMA, cycle-accurate cache model, hardware comparison, universal absence
of stale-code defects or production-user recovery claim. Inputs are public
fixtures, not user sketches. No production query, identity export or replay.
