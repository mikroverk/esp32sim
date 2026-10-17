# Classic PSRAM low/high translation (P4b)

Local-only prerequisite on accepted P4
39caacbebf54dbc33718033dbad3975cade2cffa. No publication or timing.
P5 owns stock Arduino allocator/readback qualification; this is not P5 acceptance.

## Source-derived address contract

ESP32 TRM v5.8, section 4.3.2.2, External RAM, printed pages 86–88,
tables 4.3-14, 4.3-16 and 4.3-18:
https://www.espressif.com/sites/default/files/documentation/esp32_technical_reference_manual_en.pdf
PDF SHA-256 4ba58e9fa0405ec2bf80b912a29b483f6edc8c4b2b1058201913a2fe37e582f0.

For either PRO_CPU or APP_CPU in low/high mode:

| Virtual range | Cache / MMU bank | Local PID 0/1 slots | MMIO entries |
|---|---|---|---|
| 0x3f800000–0x3f9fffff | PRO / L VAddrRAM | 1152–1215 | 0x3ff11200–0x3ff112fc |
| 0x3fa00000–0x3fbfffff | APP / R VAddrRAM | 1216–1279 in APP bank | 0x3ff13300–0x3ff133fc |

TRM global APP entry numbers are 3200 + virtual page, i.e. the PRO/APP
bank separation is 2048 entries. The high half does NOT restart at local
slot 1152: the TRM's low/high example explicitly adds virtual page 0x4e
to the R base. Upper PRO and lower APP entries are unused and may differ
or be invalid. This shared mapping applies independently of the CPU issuing
the access. The translator resolves every byte, including a crossing at
0x3f9fffff, before any store modifies the backing.

Pinned ESP-IDF v4.4.7, commit
38eeba213aa695aabfd6d89aa9f5078dbe5a94c3 (all paths below at that revision):

- components/esp_hw_support/port/esp32/cache_sram_mmu.c:19-27,69-71,90-110:
  bank bases 0x3ff10000/0x3ff12000, PID 0/1 slot 1152, 32-KiB pages.
- components/esp_hw_support/port/esp32/spiram.c:31-38: stock non-unicore
  selects LOWHIGH unless EVENODD is explicitly configured; :128-138 maps
  both tables over the full device aperture.
- components/esp_hw_support/port/esp32/spiram_psram.c:1090-1105 clears SPLIT,
  sets PRO HL bit 16 and APP HL bit 14, clears both caches' DRAM1/OPSDRAM
  masks, and sets 32-KiB page mode.
- components/soc/esp32/include/soc/dport_reg.h:186-326,426-554:
  CACHE_CTRL offsets 0x40/0x58, cache enable bit 3; CTRL1 offsets
  0x44/0x5c, masks at bits 3/5 and SRAM page mode at bits 6–8.

TRM page 86 explicitly states that disabling APP cache in HL mode leaves
both CPUs able to use only the lower 2 MiB. Accordingly each half requires
its owning cache enabled, regardless of whether APP_CPU itself is held.
The symmetric PRO-low ownership follows table 4.3-16 and the two-cache
writeback routine below. Disabling one owning cache does not disable the
other half. SPI0 CS1/QIO/read or write command readiness and device QPI
remain required as in P3, with no clock/latency fidelity claim.

Qualification restrictions, NOT claims about silicon:

- Require BOTH DRAM1 and OPSDRAM masks clear and page mode zero on the
  selected cache. The pinned driver clears both; these sources do not
  establish independent local-vs-remote mask routing for each CPU. No new
  per-requester mask behavior is inferred.
- SPLIT (EVENODD) on a participating cache, HL+SPLIT, and mixed live
  normal/HL modes fault. In PRO-only normal startup, held APP's unused
  configuration is ignored exactly as in P4. When PRO
  selects HL, require APP HL even if APP_CPU is held; mixed held-HL is not
  qualified either. The stock driver configures both bits together.
- Normal mode retains P4's matched-table/live-cache restriction unchanged;
  divergent normal-mode mappings are not newly claimed.
- Only 0/4-MiB devices, PID 0/1 mapping, and 32-KiB SRAM pages are qualified.
  No himem bank-switch API or >4-MiB support is added.

The backing remains synchronous functional write-through. IDF spiram.c:254-306,
especially :284-294, reads both the lowest and highest 2-MiB regions to evict
both hardware caches. This is a functional no-op for writeback in this model:
there are no dirty cache lines, hit rates, latency, or delayed bytes. Access
checks still apply to those guest reads.

## Stock 80-MHz path

Stock Arduino-ESP32 2.0.17 A2 sdkconfig has FLASHFREQ_80M=y (:98),
SPIRAM_SPEED_80M=y (:682), BANKSWITCH_ENABLE=y/RESERVE=8 (:720-721), and
no external BSS/noinit (:691-692). IDF spiram.c:45-52 therefore selects
PSRAM_CACHE_F80M_S80M. spiram_psram.c:1030-1033,1050-1071 uses the normal
single-byte eight-bit EB read / 38 write commands, QIO and a 24-bit address;
it skips the legacy double-byte DCLK encodings at :1075-1083. This is the
80/80 command path exercised by P4's unchanged standalone unicore fixture
and P1/P3 command/cache tests. Only the mapping mode differs here. No
frequency mismatch blocks this prerequisite; electrical timing is not modeled.

## Himem and exact expected total heap

The linked unmodified A2 image, not sdkconfig alone, decides reservation:
app SHA-256 a1e48c65f4ffaa2f403ea95caec3157850a2a28c6cc68358ec020244f5b56dff;
ELF SHA-256 794fdb44b8463ee268268511dd74f565ce384765adb109f3892b9ca8281070d6.
The map resolves esp_himem_reserved_area_size to spiram.c.obj, address
0x400f22e4; linked disassembly is entry a1,32; movi.n a2,0; retw.n.
It is the weak zero-return definition in spiram.c:65-70, not the strong
esp_himem.c:126-128 implementation. The latter would reserve 8 * 32768 =
262144 bytes even for a 4-MiB device if linked, but is not linked here.

Both external BSS/noinit boundaries in the linked map are 0x3f800000.
spiram.c:73-78,192-209 gives:

    usable = min(4194304, 4194304) - 0 = 4194304
    start = 0x3f800000
    end = start + usable - 1 = 0x3fbfffff
    heap_caps_get_total_size(MALLOC_CAP_SPIRAM) = end - start = 4194303

The final equation is heap_caps.c:505-514; heap_caps_init.c:215-218 retains
that range. This is the exact source-derived expected total for the retained
A2 image, not free bytes and not 4194304. Free space may be smaller due to
metadata/allocations. P5 must assert the total after actual heap registration.
No himem APIs are invoked or qualified here.

## Reproduction

Use cargo +1.99.0, nice -n 10, four jobs and a caller-owned CARGO_TARGET_DIR.

    cargo +1.99.0 test --release -j4 -p esp32 psram
    P5_FIRMWARE_DIR=STOCK_A2 ESP32SIM_ROM_DIR=ROMS \
      cargo +1.99.0 test --release -j4 -p esp32sim \
      --test classic_psram_lowhigh -- --ignored --nocapture
    python3 docs/evidence/classic-psram-lowhigh/mutate.py IDLE_TREE OUTPUT

The external replay reuses the retained P0 A2 firmware and the native public
setter. It makes no guest cache/MMU writes, substitutes no SDK, and installs
no function stubs. Read-only breakpoints bracket the real SDK memory test
and stop at esp_spiram_add_to_heapalloc before heap metadata changes memory.
All 131072 stride-32 test words across 4 MiB are independently verified against
spiram.c:97-118. The host-side observer verifies completion; SDK informational
serial logs are compiled out in this stock build. The absence replay runs to
P0 result=no-psram and checks ffffffff ID, found=0 and heap=0.

## Verification

Frozen Rust implementation and tests: 090b2f3754ef3dfe65b07658e2d6525c9a80adb4.
checks.json records every required gate with exit zero:

- Native workspace/all-targets Clippy and release WASM jit-tests Clippy,
  warnings denied: PASS.
- Empty-HOME native ROM-enabled workspace: 1005 passed, zero failed.
- Empty-HOME plain workspace without ESP32SIM_*: 975 passed, zero failed,
  54 ignored (including the external A2 replay tests).
- Production WASM build, eight standard demos and JIT handoff: PASS.
- Privacy and whitespace: PASS.
- Stock A2 present/absent native replay: 2 passed, zero stubs, stub hits,
  or reboots. Present stops at heap-add after 6779926 total instructions,
  891 exceptions and 50 interrupts, with all 131072 SDK memory-test words
  verified. The breakpoint PC is 0x400dce40. The unmodified psramInit calls
  testSPIRAM at 0x400d1f48, rejects zero at 0x400d1f4e, and calls heap-add
  at 0x400d1f51; testSPIRAM directly returns esp_spiram_test's result.

The adjoining JSON files record mutations, unchanged golden blobs and linked
disassembly separately. No JIT implementation changes; wasm-jit-test.sh is not
applicable. No classic CLI flag, public API, camera/I2S files, WASM PSRAM-on
claim, or full Arduino qualification is included.

## Semantic mutations and restored controls

All six new named tests are killed by at least one semantic runtime assertion
failure: the four native low/high tests by mutations.json, and both external
A2 tests by a2-mutations.json. New probes: 19/19 killed (17 native + 2 A2).
The A2 probes deny HL acceptance and change the absent-device ID fill to zero,
respectively. Compiler errors never count as kills.

Inherited reruns: P1 24/24, P2 11/11, P3 30/30. inherited.py adapts only P2's
matched-table expression and P3's replaced mode-guard expression to the new
helper names; original test assertions are untouched. P2's bounds-removal kill
is the inherited host bounds panic, not an assertion-quality improvement.
The unchanged P4 fixture kills 5/7: byte read/write corruption, half-sized
backing, physical-page aliasing, and denial of live QPI. Its slot-offset and
page-shift mutations still survive self-consistent guest readback, as disclosed
in P4; complementary native P2 probes kill both. Total: 89/91 killed, including
that one inherited P2 bounds panic; two P4 survivors, no new-probe survivors.
All sources restore cleanly. The restored native PSRAM suite passes 18/18,
and the restored real P4 fixture passes again.

## No-PSRAM regression controls

All 55 tracked tests/golden blobs are byte-identical to accepted P4; all
ROM-enabled golden tests pass without regeneration. controls.json retains
normally linked P4/head no-PSRAM comparisons for classic hello, S3 hello and
the unmodified stock A2 probe. Each has identical console SHA-256, per-core
instruction counts, exception/interrupt totals and IRQ-source reports.
Wall rates and cycle-derived counts are not comparison pins.

| Control | Console SHA-256, identical base/head |
|---|---|
| classic hello, 3 seconds | 8d8b7d143063e8d3d97b278cc43b326fc0378bf89d6a6ff7091a1f72b4c4c4ff |
| S3 hello, 3 seconds | 37bdaab6e21441d8104437c137114cd91d3b1aa3a15b9eaac05dc53fdba09e2a |
| stock A2 absent, 1 second | 0098898c52268f88a381a071be8e75164a45c2b1388abfafd8d8a72f988cc320 |

The A2 absence control still prints 0xffffffff ID, found=0, heap=0, free=0,
and result=no-psram. The separate native setter replay checks the same absence
markers with zero stubs/hits/reboots.

## Normally linked hot-path disassembly versus P4

Both binaries use cargo +1.99.0 build --release -j4 -p esp32sim --bin esp32sim
in separate source trees/targets, not assembly emitted outside the link.
linked/ retains binary hashes, raw compressed disassembly and static-address
contexts. No shared-target cross-worktree executable is used.

| Function | P4 instructions | P4b instructions | Mnemonic sequence |
|---|---:|---:|---|
| tick | 2207 | 2207 | identical |
| fetch | 133 | 133 | identical |
| read8 | 113 | 113 | identical |
| read16 | 133 | 133 | identical |
| read32 | 188 | 188 | identical |
| shared store | 2752 | 2752 | identical |

Raw bytes are not claimed identical: code/call/import-stub addresses relocate,
as do ADRP-derived static addresses. All 20 changed ADD/LDR address-low parts
follow ADRP and resolve to the same objects shifted by 0x480 (including page
rollovers), not changed bus-field offsets. No new bus fields or tick checks.
Read/fetch differences are branch/call target addresses only. New mode/table
and owning-cache logic executes through the existing #[cold] PSRAM entry
paths; reads still call the out-of-line psram_load instantiations. No cache
polling, new fast-memory mapping or no-PSRAM access guard is added by P4b.
Static equality is not a timing or performance claim.

## CPU comparison

PENDING. No timing performed.
