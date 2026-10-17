# Classic PSRAM IDF boot/allocator qualification (P4)

Local-only fixture on P3 5585ea5edc8fc9f1fdda29bff9fa2584ab20700d.
Runtime fixture/test implementation: fbf59b0c6250f716a080ad8db312488d4b1d0880;
subsequent additions retain provenance validation and mutation/check receipts.
No emulator implementation, SDK source, guest function or allocator is patched.
This is a new-feature qualification, not a production-failure recovery claim.

## Genuine guest result

[Source, configuration, recipe and input hashes](../../fixtures/classic-psram-idf/README.md).
The pinned official ESP-IDF 4.4.7 SDK is built from source for a standalone
unicore, normal-cache, 80 MHz flash/80 MHz PSRAM BOOT_INIT profile. The SDK's
own memory test and allocator registration execute before app_main.

Observed guest lines (same on two fresh machine creations):

    P4 profile=boot-init idf=v4.4.7
    P4 physical=4194304 heap=4194303 free=4192135
    P4 allocation=1048576 address=3f80087c
    P4 readback=1048576 errors=0 fnv1a=678f9dc5 pages=33
    P4 spiram_dma=rejected plain_dma=internal
    P4 exhaustion=rejected
    P4 result=pass

Every allocated byte is pattern-written and read across all 33 occupied 32-KiB
pages (the first and last are partial), not just the first byte of each page. Guest pointer
range, full readback hash and errors are checked. The physical size is 4 MiB;
the heap's one-byte difference follows unmodified IDF end-start accounting
(exact source citations in the fixture README), not a patched capacity getter.
The available mapping is unreserved 4 MiB; the free heap is separately reported.

The native harness checks every one of the guest-programmed 128 PRO SRAM MMU
entries and the held APP table. It never prepares cache/MMU registers or writes
PSRAM on the guest's behalf, and asserts zero function stubs/hits and reboots.
The same firmware with host PSRAM absent reports ffffffff ID, fails startup,
and never reaches the memory test, external heap registration or app_main.

Pinned host stop state after bounded chunks (instruction counts, not cycles):

| Presence | Instructions | Exceptions | Interrupts | Per-core/source |
|---|---:|---:|---:|---|
| present, each of two runs | 26944062 | 399 | 15 | PRO 6=13, 9=2; others zero |
| absent | 2431527 | 22 | 0 | all zero |

## Reproducibility and linked inputs

Two clean container builds in different host output directories have identical
bootloader, partition-table and app bytes. The recipe pins the public container
index and validates the SDK Git SHA; the effective sdkconfig, its hash, all
contributing archive hashes, linked guest addresses, source hashes and firmware
hashes are committed. validate.py checks the committed source/config/binaries.
The NOTICE lists only map-contributing archives and linked licences. Native CI
needs only the fetched mask ROM, not a compiler install or external firmware.
The binaries contain neither credentials nor personal build paths.

## Checks

checks.json records the commands and zero exits. All Cargo work uses
cargo +1.99.0, nice -n 10, four jobs and a caller-owned out-of-tree target.
Native tests run with an empty HOME, explicit RUSTUP_HOME/CARGO_HOME, no inherited
ESP32SIM_* variables, and only ESP32SIM_ROM_DIR on the ROM-enabled invocation.

- Workspace/all-targets native Clippy, warnings denied: PASS.
- Release wasm32 esp32sim-wasm jit-tests Clippy, warnings denied: PASS.
- Release workspace CI policy: 1001 passed, zero failed.
- Plain release workspace: 971 passed, zero failed, 52 ignored.
- Production WASM build and all eight standard demos plus JIT handoff: PASS.
- Fixture-only present/absent tests: 2 passed; present boots twice.
- Restored inherited native PSRAM tests: 14 passed.
- Privacy and whitespace: PASS. No JIT source changes; wasm-jit-test.sh is not
  applicable. The unchanged no-PSRAM WASM tests do not qualify PSRAM-on WASM.
- All 55 existing tracked tests/golden blobs remain byte-identical to P3.
- No change to Rust emulator/CLI/WASM implementation or hot-path disassembly;
  this diff adds a native test, firmware and documentation only.

The existing wasm-build.sh copies from the literal target/ path. For an
out-of-tree CARGO_TARGET_DIR, create a worktree-local target symlink to that same
caller-owned directory before invoking it; remove the symlink at cleanup. This
is a build harness accommodation, not a source patch or separate build location.

## Semantic mutations

Run in an idle disposable worktree using the caller's target and ROM directory:

    python3 docs/fixtures/classic-psram-idf/mutate.py MUTANT_TREE OUTPUT
    python3 docs/evidence/classic-psram-mmu/mutate.py --repo MUTANT_TREE --out P2_OUTPUT

mutations.json retains seven semantic probes. Five are assertion-killed by the
new guest fixture: byte-store corruption, byte-load corruption, half-sized device
backing, physical-page aliasing and denial of live QPI. Both byte mutations leave
the SDK's word memory test intact but fail the subsequent bytewise guest readback.
A kill requires a named runtime test failure, panic location and missing guest
milestone assertion; compile failures do not count. The restored guest test passes.

Two probes survive the standalone fixture: shifting the SRAM slot by one and
using a 64-KiB shift. CPU-only self-consistent readback is not an independent
physical-address oracle. This limitation is retained, not counted as a kill.
The complementary inherited P2 suite covers physical aliases/remaps and kills
all 11 of its probes, including slot-offset and page-shift changes (p2-mutations.json).
P2's bounds-removal probe retains its previously accepted host bounds panic;
it is not included among P4's five runtime assertion kills. The P2 source is
restored and its 14-test native PSRAM suite passes again afterward.

## Limits

The manual-init profile is identified in the fixture README but not qualified
by this card: at least one profile, BOOT_INIT, is genuinely executed. The stock
Arduino 2.0.17 dual-core low/high mode remains outside P3's normal-cache contract;
its preflight reaches the memory test but faults there. The 40/40 legacy command
profile also remains outside this fixture and rejects ID. Neither failure is
hidden by editing SDK code or claimed as Arduino success. P5/P6 own those SDK
compatibility milestones. There is no full dual-core, camera, DMA expansion,
WASM PSRAM, hardware/electrical-timing or production recovery claim.

## CPU comparison

PENDING. No timing or publication performed; no emulator execution path changed.
