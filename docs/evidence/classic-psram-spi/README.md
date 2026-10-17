# Classic PSRAM SPI command adapter (P1)

Local new-feature slice on the P0 fixture/design commit. This is command-engine
qualification, not a working PSRAM heap or a claim that Arduino/camera sketches
can use external memory. Classic SocBus still passes an empty PSRAM slice; P2/P3
own backing, mapping, cache controls and machine reboot integration. No CPU tick,
DMA reach, memory API, constructor ABI or camera code changes are made here.

## Public-driver contract

Register sources are ESP-IDF v4.4.7 components/soc/esp32/include/soc/spi_reg.h
and v5.1.4 components/soc/esp32/include/soc/spi_reg.h. Offsets and phase bits
agree for these transactions. Driver sources:

- v4.4.7 components/esp_hw_support/port/esp32/spiram_psram.c;
- v5.1.4 components/esp_psram/esp32/esp_psram_impl_quad.c:277-302,403-503.

The pinned P0 A2 (Arduino 2.0.17) and A30 (3.0.7) traces use identical SPI1
values: PIN=5, MOSI-only exit f5 (USER=08008000, MOSI_DLEN=7), then ID read
(USER=f0000000, USER1=5c000001, USER2=7000009f, ADDR=0, MISO_DLEN=3f).
Tests replay those values through ClassicSpi, not directly through a second
PSRAM implementation. P0 could not capture successful entry/mapping: address-only
35 (ADDR=35000000, eight address bits, no command) and reset/re-entry tests are
source-derived functional controls, not fabricated successful guest traces.

The classic adapter opts into an appended optional quad-control state in SpiMem.
Only classic CS1 transactions use the new phase decoder; existing shared ID,
payload and dirty-memory paths are reused. CS0 and the default S3/C3/C6 decoder
remain unchanged. SPI0 does not produce command-execute effects. Both disabled
CS lines return ff for reads and cannot fall through into flash writes.

An empty caller-owned backing means no device and returns eight ff ID bytes.
For 4 MiB, ID bytes are 0d 5d 22 00 00 00 00 00: KGD 5d and EID density 22
satisfy the public driver check. Identity and zero-filled ID tail are deterministic
model choices, not measurements of an AI-Thinker module. Other chips retain
their pre-existing shared density/octal behavior. Classic host size validation
remains P2; these tests supply only absent and 4 MiB backing explicitly.

Functional mode/reset policy (inferred, no electrical/timing fidelity):

- 35 enters QPI; f5 exits it (including recovery from unknown initial mode).
- ID 9f is SPI-only and requires an eight-bit command and 24-bit address phase.
- SPI payload commands 02/03/0b and QPI 38/eb share the caller's backing.
  Wrong-mode or malformed/unknown commands return ff on enabled MISO and do not
  modify payload. Existing address and data phase enables gate payload handling.
- 66 arms a single immediately-following 99 reset, restoring SPI mode. An
  intervening CS1 command consumes the latch; an unarmed reset does nothing.
- Device reset retains RAM bytes. This is not the deterministic machine reboot
  power-cycle policy (zero bytes, same allocation) reserved for P3.

Excluded: old-rev0 delay-clock/2T encodings, line-level timing, arbitrary mixed
phase encodings, complete vendor commands, cache-register/FIFO alias correction
(P3), CPU mappings/allocator/guest success and actual WASM PSRAM-on qualification.

## Verification

Five CI-runnable tests in esp32/src/psram_spi_tests.rs cover pinned probe/absence,
address-only entry/MOSI exit/re-entry, armed/unarmed/consumed reset, CS0/CS1/disabled
CS isolation and payload/phase/mode rules. The test ID validity check uses the
public KGD rule; full eight bytes are asserted separately. No firmware binaries,
new goldens, guest symbol interception or private production input are added.

Run targeted tests:

    cargo +1.99.0 test --release -p esp32 psram_spi

Run semantic mutations with CARGO_TARGET_DIR under a caller-owned evidence dir
and no concurrent command accessing the same source worktree:

    python3 docs/evidence/classic-psram-spi/mutations.py . OUT/mutations

Each mutant is compiled separately, must fail its named Rust assertion (not a
compiler error), and source is restored in finally. The mutation-to-test table
is the mutants list in that script; results.json and per-mutant logs are retained
in the local receipt. Required full native/WASM checks and normalized classic
hot-function comparisons are retained with the exact source SHA in that receipt.
No WASM JIT implementation changes are made.

## CPU comparison

PENDING. No timing performed. Only command dispatch gains mode/phase work; no
periodic device or CPU work is added. Optional state is appended to SpiMem, so
object-layout/code-generation effects must be explained in the final receipt,
not inferred to be free merely from the absence of a new tick call.
