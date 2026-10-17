# Arduino-ESP32 3.1.1 classic PSRAM compatibility receipt

Base: `c1749d38f82395070787a39865d028e7e4d45edb`.
Only a pinned external-fixture recipe, provenance and documentation are added.
No emulator source, existing assertion, public API, golden or CI input changes.
Hot-path disassembly is therefore structurally unchanged: all Rust source inputs
are byte-identical to the base. No new behavior is inferred from this observation.

See the [recipe and bounded contract](../../fixtures/classic-psram-arduino311/README.md).
`provenance.json` records actual firmware/configuration/source and 56 linked
archive hashes. The installed SDK zip was hashed and matched the expected SHA.
The full recipe was exercised, and its firmware/provenance matched a separate
isolated build. This is not a clean-directory reproducibility claim for ELF paths.

## Observed results

Both existing P6 external tests pass against stock A31, including repeated fresh
boot, real split init/test/heap registration, exact free recovery and no-device
rejection. `minimal-trace.txt` retains the actual compact UART/host milestones.
No stubs or guest-library patches are used for positive qualification.
The guest reports `v5.3.2-282-gcfea4f7c98-dirty`, not vanilla IDF v5.3.2.

Native and WASM Clippy pass with warnings denied; the ROM workspace passes 1006
tests, and the no-ROM workspace passes 976 with 58 explicitly ignored. Both run
with an empty HOME (only explicit Rust installation paths retained). The WASM
build and all eight standard demo smokes pass. All 55 existing golden files,
including no-PSRAM/classic PCM/S3 controls, are byte-identical to the base and
their enabled test suites pass. `checks.json` records exact commands and exits.
No JIT implementation changes were made, so the extra JIT-change gate is not
required for this documentation/fixture extension.

## Mutation evidence and limits

The exact existing `cli/tests/classic_psram_arduino3.rs` is reused; its SHA-256 is
in `summary.json`. There are zero new Rust assertions. Both reused test functions
fail under semantic mutations; compilation errors are never counted as kills.

Run the inherited `docs/fixtures/classic-psram-arduino3/mutate.py` in a clean
caller-owned worktree with ARDUINO3_FIRMWARE_DIR pointing at the A31 output,
ESP32SIM_ROM_DIR set, and an owned CARGO_TARGET_DIR. Seven of nine emulator
mutants are killed by the A31 guest. The normal-mode SRAM slot/shift mutants
survive this low/high guest: they are not claimed as A31 kills. Applying those
two mutants to the existing `sram_all_128_pages_widths_edges_and_physical_aliases`
unit test kills both instead. This confirms separate normal-mode coverage, not
sensitivity of the A31 profile to an inactive normal-cache path.

Run the inherited `state_mutations.py` with its SDK directory/version literals
changed from arduino-A30/3.0.7 to arduino-A31/3.1.1. All four guest-state mutants
are killed (failed reprobe, successful duplicate false, failed retry true,
failed heap-add true). All source bytes are restored in finally blocks; final
source/provenance hashes and the positive A31 tests pass after restoration.
`mutations.json` preserves every outcome, including the two guest survivors.

## Explanation and excluded claims

Stock A31 retains the P6 init/heap/allocation contract despite using a newer
binary SDK. This supports only classic ESP32-CAM with the retained stock profile.
No claim covers every Arduino 3.x, S3 compatibility, all configurations, hardware
fidelity, camera execution or WASM execution of this new A31 firmware itself.
The standard WASM controls are regression checks, not an A31 WASM qualification.

The scope's vanilla-I53 baseline is source-only in the retained inputs. No
executed vanilla-I53 baseline is available here, so runtime equivalence with it
is not claimed. The actual version divergence is named above and the actual
A31 trace is retained; the positive comparator is the unchanged P6 contract.

The recipe requires an explicitly existing config to avoid the CLI's global
fallback. `tools/wasm-build.sh` assumes `target/`; with CARGO_TARGET_DIR elsewhere,
create a caller-owned `target` symlink before running the wrapper.

## CPU comparison

PENDING. No timing or publication performed. Emulator Rust inputs are unchanged.
