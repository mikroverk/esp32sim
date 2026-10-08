# EX211 review verification

Measured candidate: `270f45c2f8d233f8c28f3ca80183e90c0b34c530`; main: `a9b4b309`.

This repeats EX211 after the camera merge with stronger observable-output and
controller timing contracts. Firmware, ROM and existing goldens are unchanged.
The two additional goldens pin console and exception/interrupt/configuration output.
The 320M accounted instructions are a duration check, not a timing oracle.

## Mutation checks

Run `python3 docs/evidence/ble-c3-advertising/review/mutations.py` with Rust 1.99.0
and the fetched C3 ROM. Each mutation is applied alone and the source is restored
in `finally`. A compiler failure does not count: a named test must fail.
[mutations.json](mutations.json) preserves the exact edit, original source hash,
command, exit code and failing tests. All 18 mutations were killed. The first nine
cover the review's list; these are suite-level guarantees, not claims that the
advertising golden alone detects every timing defect.

| Mutation | Failing test |
| --- | --- |
| `latch_latency` | `latch_is_atomic_and_wraps_in_modeled_time` |
| `fine_latch_counter` | `latch_is_atomic_and_wraps_in_modeled_time` |
| `reset_latency` | `reset_completes_without_polling_and_cancels_alarm` |
| `reset_keeps_alarm` | `reset_completes_without_polling_and_cancels_alarm` |
| `alarm_one_half_us_late` | `alarm_mask_ack_rearm_and_wrap` |
| `end_status_aborted` | `each_completed_event_retains_an_end_and_cancelled_timer_stays_out` |
| `half_slot_bit_removed` | `reset_completes_without_polling_and_cancels_alarm` |
| `event_past_due_wraps` | `channel_map_reset_and_bad_descriptor_do_not_transmit_extra_packets` |
| `power_gate_removed` | `silicon_readbacks_follow_configuration_and_power_state` |
| `event_fine_removed` | `event_fine_timestamp_and_past_due_event` |
| `alarm_past_due_wraps` | `past_due_alarm_is_due_now_not_after_wrap` |
| `end_coalescing` | `each_completed_event_retains_an_end_and_cancelled_timer_stays_out` |
| `stale_timer_retained` | `each_completed_event_retains_an_end_and_cancelled_timer_stays_out` |
| `reboot_disables_controller` | `guest_esp_restart_preserves_full_ble_and_advertises_again` |
| `enable_skips_optional_refresh` | `optional_device_routes_source_eight_only_when_enabled` |
| `enable_skips_work_refresh` | `optional_device_routes_source_eight_only_when_enabled` |
| `half_slot_deadline_removed` | `half_slot_deadline_and_ack_are_chunk_independent` |
| `gated_write_accepted` | `gated_writes_are_ignored_and_bt_reset_cancels_work` |

The event fixture uses ET+6=400 instead of 624. Past-due events and alarms fire
immediately in the model; silicon behavior remains an open hardware question.
The two-END fixture completes two ETs before running the ROM-style pop/ack sequence
and cancels a pending TIMER by masking it. Each END remains separately dispatchable.
The reboot test executes `esp_restart` in the pinned IDF 5.5.5 image, follows its RTC
watchdog reset, and observes 57 advertising PDUs both before and after reboot.
The mapping test compares all 51 ROM lookup pages with `em_base_reg_lut`, then checks
an all-zero mapping and a cross-region span. This is independent of the probe's
mapping decoder.

## Hardware repeat and timing limits

[hardware.json](hardware.json) repeats the retained comparator against the original
C3 rev v0.3 capture using the unchanged probe images. All 32 structural checks pass
on each side and no static mismatch remains under the corrected comparator.
LC+2cc remains unmodeled live error status. The 10 ms deadline tolerance covers the
entire advertising random-delay range. It proves no over-air timing, channel order,
END timing, TX descriptor semantics or FIFO behavior. The two half-us ticks/us rate
and latch completion remain hardware-checked; reset and past-due rules are inferred.

[timing.json](timing.json) repeats the native default/MAC-override experiment and
production WASM ABI run. Both native variants emit the same event times; WASM
times differ. Counts match at 19 events / 57 PDUs. The scheduling-quantum explanation
remains unverified.

## Firmware notice

The application ELF SHA-256 is
`b7eb3e202f00939a769d47201ce5a5c0b3c14a90fe4f7165860ed051ad23f1e8`.
`riscv32-esp-elf-nm ble_advertiser.elf` contains zero `mbedtls` symbols. Mbed TLS is
not listed as linked. The notice includes ESP-IDF's general Firmware Components
inventory, separately labeled as possible components, from
[ESP-IDF v5.5.5 COPYRIGHT.rst](https://github.com/espressif/esp-idf/blob/b774170ff46c393eeb5e495ea37936038d3f4f4f/docs/en/COPYRIGHT.rst).
No firmware bytes changed; the original reproducibility hashes still apply.

## Reproduce mode-off CPU comparison

```sh
mkdir -p target/main
git archive a9b4b309 | tar -x -C target/main
cargo +1.99.0 build --release --bins --manifest-path target/main/Cargo.toml
cargo +1.99.0 build --release --bins
python3 docs/evidence/ble-c3-advertising/review/bench.py \
  target/main/target/release target/release web/wasm/fw > target/review-cpu.json
```

The harness checks instruction totals and console hashes on every pair. It retains
one warmup pair and seven measured main/candidate pairs per workload, process user
CPU time including startup, binary/input hashes and aggregate load. Builds and tests
are stopped during measurement; unrelated host load is uncontrolled. No sub-percent
precision or speedup is claimed. Full samples are in `cpu.json`.

Validation: both Clippy commands pass with warnings denied; 641 CI-policy tests
pass; 620 plain tests pass with 35 ignored; all eight WASM demos, BLE/timing ABI,
VQ and ancillary CI checks pass. `../checks.json` records the commands. JIT sources
are unchanged, so the conditional JIT suite was not rerun.

Median child user CPU, main → candidate: C3 hello 2.444 → 2.424 s (−0.8%); C3 Wi-Fi station 1.454 → 1.450 s (−0.3%); S3 hello 0.259 → 0.258 s (−0.6%); C6 hello 3.222 → 3.223 s (0.0% at this precision).

All measured pairs, seconds rounded to milliseconds; extra digits in JSON preserve
samples, not precision. Each row runs main then candidate. Ranges overlap.

| Pair | S3 hello main | candidate | C3 hello main | candidate | C6 hello main | candidate | C3 Wi-Fi main | candidate |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 0.265 | 0.257 | 2.453 | 2.424 | 3.221 | 3.177 | 1.432 | 1.419 |
| 2 | 0.254 | 0.256 | 2.458 | 2.417 | 3.222 | 3.223 | 1.439 | 1.436 |
| 3 | 0.256 | 0.267 | 2.435 | 2.444 | 3.264 | 3.214 | 1.466 | 1.490 |
| 4 | 0.259 | 0.257 | 2.444 | 2.413 | 3.222 | 3.213 | 1.514 | 1.444 |
| 5 | 0.254 | 0.258 | 2.463 | 2.407 | 3.292 | 3.309 | 1.442 | 1.450 |
| 6 | 0.267 | 0.268 | 2.423 | 2.431 | 3.175 | 3.239 | 1.454 | 1.480 |
| 7 | 0.263 | 0.259 | 2.435 | 2.427 | 3.240 | 3.230 | 1.523 | 1.452 |
| Median | 0.259 | 0.258 | 2.444 | 2.424 | 3.222 | 3.223 | 1.454 | 1.450 |

C3/C6 hello account for 4.8B instructions, C3 Wi-Fi for 2.24B, S3 hello for
18,788,848. Every Wi-Fi run completes five of five pings. Counts and console hashes
match main on every run.

## Empty-HOME and mapping-start follow-up

At `a26e1d67`, `full_ble_accepts_c3_alias_and_rejects_hci_script_without_panic`
fails with an empty HOME both without and with ESP32SIM_ROM_DIR. With CLI BLE
script validation before ROM loading, both runs pass. Syntax validation reuses
`ble::peer::Command`; the existing tests also reject any preceding ROM-load log.
`find_rom` consults ESP32SIM_ROM_DIR first. A one-instruction ROM boot succeeds for
S3, C3 and C6 with only that ROM directory and an empty HOME.

The full release workspace suite also passes with an empty HOME: 620 passed and
35 ignored with ESP32SIM_ROM_DIR unset; 641 passed with the fetched ROM directory
and `--include-ignored --skip external_`. Toolchain caches remain explicitly set
through CARGO_HOME and RUSTUP_HOME; neither supplies a ROM. `../checks.json` retains
the reproduction commands, before/after exit codes and tested source hashes.

The nonzero region-0x2c00 mapping with programmed start 0x3400 is rejected.
`python3 docs/evidence/ble-c3-advertising/review/mutations.py mapping_start_mismatch`
removes only the start comparison and fails `mapping_boundaries_match_rom_em_base_reg_lut`.
This nineteenth row records its own source hash; the original eighteen rows retain
their earlier measured source hashes. Both Clippy gates and the full native/WASM
check set pass. No goldens or firmware bytes change. No per-tick or JIT code changes.
