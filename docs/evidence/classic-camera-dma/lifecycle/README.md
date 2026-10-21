# Classic camera lifecycle qualification

**Classic CPU comparison pending.** No CPU timing run was performed. Any future
comparison requires a quiet machine and identical instruction counts and console
hashes; the historical code-generation comparison below is not a CPU measurement.

Base: `572c940681a063c732e798196d586d072af8f2e2`. This extends its pinned
public-driver fixture, not the emulator runtime or the host API. The original
C8 images, tests, goldens and compiler-provenance documents are unchanged.

## Inputs and observed result

[Build recipe](../../../../examples/classic-camera/lifecycle.md),
[image hashes](builds.json), [source pins](sources.json),
[107-file pinned driver manifest](../fixture-driver-sources.json),
[linked archives](linked-archives.json). ESP-IDF v5.4.3 and esp32-camera
b817e9b81041980608d1a3d3e0b08d86e3666382, Xtensa GCC esp-14.2.0_20250730.
Two clean builds in separate source/output directories reproduce all three
retained images exactly. The final application is 248,256 bytes,
SHA-256 `2746afe90c3d855244339c2b6e505b0e3c7c5b69c09dc9ecf560bbed120dd661`.
All 107 fingerprinted public-driver files match the pinned source; driver code
is unchanged. Application/bootloader archive inventories remain 41/11, matching
C8. The adjacent lifecycle NOTICE names only linked components.

`public_driver_camera_lifecycle` boots the normal ROM/bootloader/application,
with no ELF, symbol stubs or DMA interception. It runs the following sequence
twice on the same machine, separated by a machine reboot while the camera is live:

1. Initialize the public driver and capture/return two QQVGA RGB565 frames.
2. Replace the host picture at the guest marker without stopping capture.
   Return one possibly queued frame, then require two fresh replacement frames.
3. Deliberately change sensor geometry/format to QVGA/YUV422 and assert its
   status fields, then deinitialize. Require a null driver sensor pointer,
   RX stopped and no pending enabled I2S interrupt.
4. Reinitialize QQVGA/RGB565 and require two exact replacement frames, proving
   that the changed sensor configuration does not leak into the new capture.
5. On machine reboot require I2S RAW/ST/ENA all zero, then repeat from the
   original host image. This also checks replacement-cache invalidation in the
   reverse direction after reboot.

There are 14 captured buffers: 12 phase-qualified frames and two explicitly
unqualified queued frames. Qualified frames are 160 × 120, 38,400 bytes RGB565.
Independent Python construction, with no emulator imports or captured-byte
input, gives the expected SHA-256 values:

- Initial: `c67913e0afba1703e8c9c47e02b1ae2ea62e47596a8440d83f1ebba14aed6cf1`.
- Replacement: `1e1f681f056ffcf2cde1b0dd089c7cc69d09b01e3bf61299eb7437488480c669`.

The public driver logs an already-installed GPIO ISR service and an LEDC pin
warning during reinitialization; it returns ESP_OK and all captures/IRQ checks
pass. Those diagnostics are retained, not suppressed or counted as failures.

## Checks

All commands use cargo +1.99.0, nice -n 10 and a caller-owned target directory.
Cargo gates run with empty HOME, explicit existing CARGO_HOME/RUSTUP_HOME, and
ESP32SIM_ROM_DIR only for the ROM suite; it is removed for the plain suite.
[Machine-readable command results](checks.json):

| Command | Result |
| --- | --- |
| `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings` | PASS |
| `ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_` | 1050 passed, 0 failed |
| `cargo +1.99.0 test --release --workspace` | 1020 passed, 0 failed, 52 ignored |
| `RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh` | PASS |
| `node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel` | all 8 demos PASS |
| `node tools/check-evidence-privacy.mjs` | PASS, plus manual new-source/evidence review |

S3 raw controls include `camera_vsync_starts_capture_and_streams_partial_descriptors`
and `camera_host_frames_follow_sccb_configuration_into_dma_memory`. Classic PCM
controls include `i2s_native_dma_packs_pcm_and_raises_eof`,
`camera_bypasses_audio_and_mode_exit_restores_pcm` and
`c7_camera_free_active_pcm_does_not_poll_board_camera`. All pass in the final
workspace run. No golden or emulator runtime source differs from C8. No JIT
source changed, so the separate JIT mutation harness is not applicable.

## Semantic mutations

Run `examples/classic-camera/mutate-lifecycle.py` as described in the recipe.
[Six lifecycle mutants](mutations.json) compile and fail the named test's
assertions; baseline passes. Firmware mutants are rebuilt with the same public
driver. No compiler-error or timeout kills are counted.

| Semantic mutant | Assertion that kills it |
| --- | --- |
| Reverse FIFO sample order | `initial stale/incorrect frame` |
| Retain conversion cache after picture replacement | `replacement stale/incorrect frame` |
| Omit actual driver deinit | guest null-sensor assertion, then host frame-completeness assertion |
| Omit actual driver reinit | guest non-null-frame assertion, then host frame-completeness assertion |
| Leave reinitialized sensor in YUV422 | guest RGB565-format assertion, then host frame-completeness assertion |
| Keep the live I2S peripheral across machine reboot | `reboot retained camera IRQ state` |

The [five inherited C8 fixture mutants](../fixture-mutations.json) were also rerun:
disabled camera dispatch, dropped transport bytes, wrong FIFO mode, wrong sample
order and wrong EOF count unit. All five assertion-fail; baseline passes. This
is not a rerun claim for every older C4–C7 mutation suite. Source files and
retained firmware are restored; final clean builds and gates run on normal code.

## Hot-path code generation

[Normalized comparison against C8](codegen-c8.json): 21/21 selected classic
scheduler/bus/core bodies equal the independently retained C8 CLI binary.
Its SHA-256 `1321070d93ba669ae707779feab3a709a0d8fa52ee3076761faa304d6378fc54`
was verified against the accepted C8 receipt. Classic tick remains 2236 static
instructions. The comparison uses the existing EX230 relocation/call-alias
normalizer; it is not whole-binary equality. C8's inherited differences from
main remain unchanged; no new idle-path cost or source change is introduced.

## CPU comparison

PENDING. No CPU timing run was performed.

## Limits

This qualifies the upstream raw/internal-DRAM fixture only. WASM gates cover the
existing module and eight demos, not a new browser camera integration. No JPEG,
PSRAM, arbitrary wiring, sensor ISP/silicon accuracy, app/add-on integration or
production failure-rate improvement is claimed. No production data or user
firmware was used; all pictures are fixture-owned deterministic inputs. No
private paths or identifiers are needed to reproduce these checks.
