# EX222: camera host settings across guest reboot

Base: upstream/main `017af524`. The final source is the commit containing this receipt.
Related EX212 implements camera capture. This change tests a different lifecycle
contract: a guest reboot must not replace host-selected frame cadence with 10 fps.

The S3 reboot path restores frame_cycles and reapplies stored debug flags through
the same dispatcher used by set_debug. This also preserves other devices' log
flags and MMIO logging. The separate log_unknown copy stays because the dispatcher
does not cover it.
Guest registers, counters, pending frame and capture activity still reset.
The board's picture and Machine's input queue already survive; they are reused.
No new state, per-tick work, helper in a hot path or input channel is introduced.

The regression uses the real waveshare-cam board and Machine::reboot once,
with a period corresponding to 25 fps. It checks the retained period,
the actual frame_due boundary, logging set through Machine::set_debug, cleared capture state,
picture dimensions/content, queue identity and delivery of a post-reboot frame.
Pictures are synthetic two-pixel RGB arrays committed in the test. No firmware
fixture or golden was added or regenerated. The CLI assigns --cam-fps to
frame_cycles; --cam-size configures the persistent stream reader, and both
--cam-image and streamed frames use the persistent board picture.

No guest firmware reboot fixture or hardware timing was measured. This tests
the shared reset entry point used by software/watchdog resets. No changes to
LCD_CAM register meanings, guest sensor programming or DMA paths.

## CPU comparison

Measured on this PR's commit against main `017af524`, with the same Rust 1.99.0 release build settings for both.

Pinned Rust 1.99.0 release binaries and inputs reused from the original run; main 017af524 built once. Child user CPU seconds from getrusage, startup included. One warmup pair B→M, then seven measured pairs M→B, B→M, alternating. Before EVERY pair attempt, require 1-minute load <3, polling every 15 seconds. Sample load every second during both runs and before/after each run; discard the whole pair if any observed load >4 and retry the same pair/order. Warmup is subject to the same rules. Medians/ranges exclude discarded attempts and warmup. Exact console hashes and per-core counts required, except #197 Pocket Tank permits only main 10073833775 vs PR 10073833665 instructions and the verified +55 main bus-cycle overshoot. Positive change means more CPU time.

Times are user CPU seconds. 

| Workload | Main median (range) | PR median (range) | Median change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| S3 hello | 25.5501 (25.1834–26.0609) | 25.6416 (25.2962–26.1963) | +0.36% | 4/7 | 1789819657 | 3.494 |
| Pocket Tank | 35.9427 (35.5282–36.7095) | 35.8410 (35.3920–36.9167) | -0.28% | 2/7 | 10073833775 | 3.746 |

### S3 hello

Status: PASS. All attempts, including discarded attempts, follow.

| Pair | Attempt | Order | Disposition | Main user s | PR user s | Main load before/after | PR load before/after | Gate load | max load |
| --- | ---: | --- | --- | ---: | ---: | --- | --- | ---: | ---: |
| warmup | 1 | B → M | accepted warmup | 26.213177 | 26.046506 | 3.021/2.558 | 2.609/3.021 | 2.609 | 3.422 |
| 1 | 2 | M → B | accepted | 25.846647 | 25.354617 | 2.558/2.422 | 2.422/2.347 | 2.558 | 2.731 |
| 2 | 3 | B → M | accepted | 25.219191 | 25.296199 | 2.347/2.398 | 2.347/2.347 | 2.347 | 2.498 |
| 3 | 4 | M → B | accepted | 25.183444 | 25.771272 | 2.398/2.590 | 2.590/2.928 | 2.398 | 3.281 |
| 4 | 5 | B → M | accepted | 25.550108 | 26.196346 | 3.142/3.106 | 2.928/3.142 | 2.928 | 3.494 |
| 5 | 6 | M → B | accepted | 25.598709 | 25.730941 | 2.824/2.617 | 2.617/2.577 | 2.824 | 2.838 |
| 6 | 7 | B → M | accepted | 26.060932 | 25.641579 | 3.086/2.802 | 2.577/3.086 | 2.577 | 3.319 |
| 7 | 8 | M → B | accepted | 25.439475 | 25.350099 | 2.802/2.598 | 2.598/3.314 | 2.802 | 3.364 |

### Pocket Tank

Status: PASS. All attempts, including discarded attempts, follow.

| Pair | Attempt | Order | Disposition | Main user s | PR user s | Main load before/after | PR load before/after | Gate load | max load |
| --- | ---: | --- | --- | ---: | ---: | --- | --- | ---: | ---: |
| warmup | 1 | B → M | accepted warmup | 36.777022 | 36.697477 | 2.597/3.606 | 2.881/2.597 | 2.881 | 3.746 |
| 1 | 2 | M → B | accepted | 35.528158 | 35.969320 | 2.855/2.507 | 2.507/2.024 | 2.855 | 3.027 |
| 2 | 3 | B → M | accepted | 36.503697 | 36.276254 | 2.269/1.908 | 2.024/2.269 | 2.024 | 2.311 |
| 3 | 4 | M → B | accepted | 36.709509 | 35.784340 | 1.908/3.494 | 3.494/3.427 | 1.908 | 3.686 |
| 4 | 5 | B → M | accepted | 35.976366 | 35.841000 | 2.844/3.063 | 2.710/2.844 | 2.710 | 3.160 |
| 5 | 6 | M → B | accepted | 35.538290 | 36.916725 | 2.823/2.121 | 2.121/2.917 | 2.823 | 2.917 |
| 6 | 7 | B → M | discarded: load >4 | 37.211781 | 37.132121 | 3.680/4.286 | 2.917/3.680 | 2.917 | 4.286 |
| 6 | 8 | B → M | accepted | 35.942725 | 35.391954 | 2.249/2.386 | 2.732/2.249 | 2.732 | 2.732 |
| 7 | 9 | M → B | discarded: load >4 | 36.862268 | 47.902206 | 2.386/2.953 | 2.953/40.307 | 2.386 | 40.307 |
| 7 | 10 | M → B | accepted | 35.552099 | 35.476470 | 2.876/3.067 | 3.067/2.515 | 2.876 | 3.067 |

## Mutation table

Command for each mutation:
`cargo +1.99.0 test --release -p esp32s3 --test machine reboot_preserves_camera_host_settings_and_stream_but_resets_capture`.
Each mutation compiled and failed that regression test. Restore between runs.

| Mutation | Assertion that fails |
| --- | --- |
| Remove frame_cycles restoration | Configured period survives reboot |
| Remove debug dispatch | Host LCD_CAM logging stays enabled |
| Remove MMIO logging restoration | MMIO logging stays enabled |
| Remove log_unknown restoration | Unknown-register logging stays enabled |
| Copy all of old.lcd_cam instead of restoring only host fields | Capture counters and pending state reset |

The unmutated focused test passes. No private captures or machine identifiers
are retained; synthetic RGB input arrays are in the test source.

## Verification

Darwin arm64, cargo 1.99.0, Node v22.23.1. Fetch ROMs and demos with
`tools/fetch-demo-assets.sh --no-linux`; [input SHA-256 hashes](inputs.json)
identify the downloaded public assets.

Workspace checks use an empty HOME, retaining the installed CARGO_HOME and
RUSTUP_HOME only for the toolchain/cache. Remove all ESP32SIM_* variables first.
For CI policy only, set `ESP32SIM_ROM_DIR="$PWD/web/wasm/fw"`.
The CI-policy release workspace run passes 642 tests. The plain release workspace
run with no ESP32SIM_* variables passes 621, with 35 ignored. Both Clippy checks
pass with warnings denied. Existing golden files are byte-identical to the base.
No JIT code changed.

All eight production WASM demos and the evidence privacy check pass.
[Exact commands and exit codes](checks.json); [additional CI Node/Python checks](extra-checks.json).
