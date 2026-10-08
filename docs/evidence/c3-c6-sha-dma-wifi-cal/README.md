# EX220: C3/C6 SHA DMA and C6 Wi-Fi calibration/ECC

Base `017af5241f48855c412d912ff91f0a801dd3c1bd` on upstream/main.
Implementation `3df32945e91e61c574ed64de84bbfb541c225681`. This change
extends EX202's station contract to unstubbed C6 PHY startup and TLS accelerator
use. EX210's shared StationLink from merged #193 remains the only Wi-Fi link.
S3, C3 and C6 share SHA DMA execution. The S3 crypto OUT reader moves to `esp-periph`; C3/C6 reuse it with SRAM-only
memory access. The S3 adapter retains its mapped reads, unpriced access and
live channel state, including MMIO writeback effects. Camera receive DMA and
the Wi-Fi frame transport are unchanged.

## Contract and result

The committed Arduino-ESP32 3.3.11 / ESP-IDF 5.5.5 firmware obtains
`10.0.2.15` on C3 and C6 without stubs or an application ELF. Two Mbed TLS peers
complete TLS 1.2 ECDHE-PSK with P-256 and AES-128-CBC-SHA256, then send and check
all 256 byte values in each direction. Memory queues carry TLS records;
`--net none` provides the existing local DHCP service. No host endpoint is used.
C6's three existing `pll_cal exceeds 2ms` warnings remain visible in its golden.

The new goldens pin the complete console, `[emu] stop` exception/interrupt totals,
per-source interrupt counts, and Wi-Fi/network totals. The old station goldens
remain byte-identical. Their C6 fixture still uses its existing `bb_init` stub;
the new firmware supplies the unstubbed proof. No cycle-derived count is added
as a TLS correctness oracle.

SHA handles split descriptors, bounded work, START versus CONTINUE, either
SHA/GDMA arm order, CHECK_OWNER, ownership return and DONE/EOF/error state.
Bad or short input is not hashed. As in the existing S3 reader, a failure may
have returned ownership for descriptors already consumed. C3/C6 errors clear
BUSY, stop the channel and assert OUT_DSCR_ERR. Their descriptors and payloads
must be wholly in SRAM. There is no MMIO fallback that can recursively invoke
another peripheral transfer.

ECC supports P-256 scalar multiplication, point verification and combined
verify/multiply, parameter RAM, reset, raw/masked interrupt state and W1C.
Noncanonical coordinates are rejected, including a point congruent modulo p
to a valid point. Unsupported modes and key lengths do not manufacture completion. Arithmetic
uses the existing big-integer multiplication, reduction and exponentiation.
[OpenSSL reproduction](verify-ecc.py) independently confirms the nontrivial
scalar vectors in [ecc-vectors.json](ecc-vectors.json).

## Register sources

All paths below are in public ESP-IDF **v5.5.5**. The new fixture uses that
version; the older station fixture uses **v5.5.4**. [Header hashes](headers.json)
compare both tags. Register layouts and the SHA HAL are identical. The ECC HAL
adds a power-up assertion and updates its copyright year in 5.5.5; its mode,
parameter and interrupt operations are unchanged. IDF 4.4 is not a fixture input.

* `components/soc/esp32c3/include/soc/hwcrypto_reg.h:42-58` and
  `components/soc/esp32c6/register/soc/sha_reg.h:50-120`: SHA modes, six-bit
  block count, BUSY, DMA_START and DMA_CONTINUE.
* `components/soc/{esp32c3,esp32c6}/include/soc/gdma_channel.h:15`: SHA trigger 7.
  `components/soc/esp32c3/register/soc/gdma_reg.h:1621-1627` and the C6 header
  at `1710-1716`: OUT_CHECK_OWNER bit 12. C3's existing adapter translates
  its combined interrupt bits to the shared OUT_DONE/EOF/DSCR_ERR bits 0/1/2.
  C6's raw OUT interrupt layout is in `gdma_reg.h:691-723`.
* `components/hal/include/hal/dma_types.h`: descriptor size/length, EOF, owner,
  buffer and next-link layout.
* `components/soc/esp32c6/register/soc/ecc_mult_reg.h:14-154`: raw/status/enable/
  clear offsets 0x0c/0x10/0x14/0x18, interrupt bit 0, CONF fields and parameter RAM.
  `components/hal/esp32c6/include/hal/ecc_ll.h:73-90`: work modes 0/2/3.
  `components/soc/esp32c6/include/soc/interrupts.h:98`: ECC source 76.
* `components/soc/esp32c3/register/soc/reg_base.h:12-16` and the C6 header
  at `39-44`: the SHA/GDMA bases and C6 ECC_MULT base 0x6008b000.

The C6 baseband offsets 0x418 and 0x810/0x814, start bit 0, TX DC done bit 22
and calibration done bits 14..16 are **inferred**, not public register facts.
They model the libphy polling contract and ideal zero comparator output.
Completion follows START synchronously and clears when START is cleared,
consistent with the existing IQ-status model. No silicon verification or
80-APB-tick analog latency is claimed.

## Inputs and reproduction

[inputs.json](inputs.json) records image sizes/hashes, SDK archive hashes,
tool versions and the retained link-map archive inventory. Both chips' app,
bootloader and partition table reproduce byte-for-byte from different source
and output directories. The [recipe](../../../examples/crypto-tls/README.md)
fixes the build epoch and maps paths before building. ELF debug information is
removed before hashing the ELF into the flash image. The public fixture's
source and complete linked-component notices accompany the binaries.

```sh
tools/fetch-demo-assets.sh --no-linux
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release \
  -p esp32sim --test goldens crypto_tls -- --ignored
python3 docs/evidence/c3-c6-sha-dma-wifi-cal/verify-ecc.py
python3 docs/evidence/c3-c6-sha-dma-wifi-cal/mutate.py
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" \
  python3 docs/evidence/c3-c6-sha-dma-wifi-cal/mutate.py --firmware
```

Run mutations without concurrent source edits or builds; each mutation restores
its source file before the next one. The machine has no role in the firmware's
protocol inputs. No default Rust toolchain is changed.

## Mutation table

[mutations.json](mutations.json) is the rule-removal table: exact source
replacement, command, failing test names and exit status for each mutation.
It includes both arm orders, hash state, bounds, ownership, interrupts, C6
calibration status, ECC arithmetic/verification/reset and the live S3 writeback
contract. Every retained mutation is killed by a test assertion, not a build
failure. [firmware-mutations.json](firmware-mutations.json) separately removes
C3 SHA DMA, C6 SHA DMA, C6 ECC, TX DC completion and calibration completion;
each prevents the corresponding firmware golden from passing.

| Removed rule | Test that fails |
| --- | --- |
| SHA start wiring C3 | `owner_check_is_optional_and_requested_length_bounds_input` |
| SHA start wiring C6 | `owner_check_is_optional_and_requested_length_bounds_input` |
| GDMA late start C3 | `sha_before_gdma_waits_for_channel_and_never_ticks` |
| GDMA late start C6 | `sha_before_gdma_waits_for_channel_and_never_ticks` |
| DMA first resets digest | `sha_before_gdma_waits_for_channel_and_never_ticks` |
| DMA continue retains digest | `dma_continue_preserves_the_previous_hash_state` |
| DMA initializes only the first block | `one_dma_request_hashes_every_block_without_reinitializing` |
| DMA consumes pending request | `dma_continue_preserves_the_previous_hash_state` |
| DMA clears busy | `invalid_chains_raise_descriptor_error_without_hashing` |
| DMA rejects short input | `invalid_chains_raise_descriptor_error_without_hashing` |
| DMA error interrupt | `cycle_detection_is_independent_of_owner_checking` |
| DMA error stops channel | `invalid_chains_raise_descriptor_error_without_hashing` |
| DMA owner check | `invalid_chains_raise_descriptor_error_without_hashing` |
| DMA check-owner gate | `owner_check_is_optional_and_requested_length_bounds_input` |
| DMA ownership return | `sha256_split_descriptor_completes_on_start_write` |
| DMA EOF interrupt | `sha256_split_descriptor_completes_on_start_write` |
| DMA DONE interrupt | `sha256_split_descriptor_completes_on_start_write` |
| DMA EOF descriptor | `sha256_split_descriptor_completes_on_start_write` |
| DMA cycle detection | `cycle_detection_is_independent_of_owner_checking` |
| DMA descriptor budget | `gdma::crypto_dma_tests::zero_progress_chain_stops_at_descriptor_budget` |
| S3 writeback side effects | `bus::dma_tests::crypto_descriptor_writeback_preserves_mmio_side_effects` |
| TX DC completion | `txdc_comparator_outputs_ignore_software_writes` |
| Calibration completion | `txdc_and_calibration_status_follow_start_without_a_clock` |
| TX DC comparator mask | `txdc_comparator_outputs_ignore_software_writes` |
| ECC on-curve equation | `ecc::tests::verification_rejects_off_curve_and_noncanonical_points` |
| ECC START self-clear | `ecc::tests::scalar_results_match_independent_openssl_vectors` |
| ECC completion interrupt | `ecc::tests::noncanonical_point_congruent_to_valid_point_is_rejected` |
| ECC masked interrupt | `ecc_mmio_routes_cached_interrupts_without_clock_work` |
| ECC canonical coordinates | `ecc::tests::noncanonical_point_congruent_to_valid_point_is_rejected` |
| ECC unsupported key length | `ecc::tests::unsupported_modes_do_not_report_success_and_reset_clears_interrupts` |
| ECC scalar multiplication | `ecc::tests::scalar_results_match_independent_openssl_vectors` |
| ECC verification gates multiplication | `ecc::tests::noncanonical_point_congruent_to_valid_point_is_rejected` |
| ECC verify-only mode | `ecc::tests::verify_only_preserves_parameters_and_result_is_read_only` |
| ECC result read-only | `ecc::tests::verify_only_preserves_parameters_and_result_is_read_only` |
| ECC unsupported mode | `ecc::tests::unsupported_modes_do_not_report_success_and_reset_clears_interrupts` |
| ECC reset | `ecc::tests::unsupported_modes_do_not_report_success_and_reset_clears_interrupts` |
| ECC W1C | `ecc::tests::scalar_results_match_independent_openssl_vectors` |
| ECC MMIO mapping | `ecc_mmio_routes_cached_interrupts_without_clock_work` |
| ECC interrupt routing | `ecc_mmio_routes_cached_interrupts_without_clock_work` |

## Verification

Final command results and test counts are in [checks.json](checks.json).
Native target: `aarch64-apple-darwin`; WASM target: `wasm32-unknown-unknown`.
Rust 1.99.0 is selected per command. Native and WASM Clippy deny warnings. Workspace checks run with a fresh HOME,
with the ROM directory supplied for the CI-policy suite and no emulator input
variables for the plain suite. CARGO_HOME and RUSTUP_HOME select build tools
outside that HOME. Tests receive no local firmware build, ELF or hardware input.
Only the new `crypto-tls-*` goldens are generated; existing goldens are unchanged.

The empty-home test commands, with only build-tool infrastructure retained:

```sh
mkdir -p target/ex220-home target/ex220-tmp
env -i PATH="$PATH" HOME="$PWD/target/ex220-home" TMPDIR="$PWD/target/ex220-tmp" \
  CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
  ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" \
  cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
env -i PATH="$PATH" HOME="$PWD/target/ex220-home" TMPDIR="$PWD/target/ex220-tmp" \
  CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
  cargo +1.99.0 test --release --workspace
```

Final results: **668** CI-policy tests pass; **645** plain tests pass with
**37** ignored. Both Clippy targets, the WASM build, all eight requested WASM
scenarios, evidence privacy and **44/44** mutation checks pass.

ECC source 76 makes `OPTIONAL_SOURCES[2]` non-zero. The central CPU
comparison must include C6 hello.

## CPU comparison

PENDING

SHA work runs only on SHA/GDMA writes. Calibration has no clock, timer, deadline
or added field. ECC is appended to Peripherals and uses the existing optional
source cache on MMIO writes; it never joins the periodic device list. No new
per-tick transfer check or helper is added. S3's DMA call sites are unchanged.
No CPU benchmark was run. Interrupt source refresh still includes the ECC bit;
the central CPU comparison must assess the final build's layout and dispatch.

## Limits and evidence curation

This is a functional model, not accelerator latency, RF calibration, hardware
parity, browser-speed or TLS-over-TCP evidence. P-192 is unsupported: selecting
its key length does not complete an operation. ECC Jacobian/division modes,
certificate verification and TLS 1.3 are outside this fixture. The deterministic
RNG and PSK are public test inputs and provide no production security.

Retained evidence contains source-relative commands, public versions, hashes,
known-answer vectors and test verdicts. Build logs, absolute build paths and
ELF debug information are not retained. The binary strings were checked for
host paths before inclusion. Redaction removes no measured values; there are
no timing samples in this receipt. No private captures or private firmware are
needed to reproduce these tests.
