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
- SPLIT (EVENODD), HL+SPLIT, and mixed live normal/HL modes fault. When PRO
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

Final measured checks, mutations and disassembly are recorded in the adjoining
JSON files. No JIT implementation changes; wasm-jit-test.sh is not applicable.
No classic CLI flag, public API, camera/I2S files, WASM PSRAM-on claim, or full
Arduino qualification is included.

## CPU comparison

PENDING. No timing performed.
