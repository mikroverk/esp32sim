# PR243 review qualification

## Scope

Raw OV2640 RGB565/YUV422 into classic ESP32 internal DRAM, on the fixed
AI-Thinker wiring, at an approximate fixed 10 FPS. JPEG, PSRAM CameraWebServer,
adjustable FPS, ISP fidelity and per-PCLK/silicon timing remain unsupported.
The guest fixture uses 10 MHz XCLK / FIFO mode 1. Mode 3 has exact-vector
unit and descriptor coverage only; there is no compiled 20 MHz driver fixture.

Runtime/test implementation: `b9d2bea4626417ccd9df4964e2a8994aae3b54dd`.
Upstream base: `77e43c20` (integrated by merge `0a2f6bd3`). Original published
camera commits remain ancestors, preserving the receipt source references.
The original `7352bc81` merged cleanly with main; after adding the user-doc
review fixes, integration required combining only README text with main's new
layout. No published history was rewritten.

EX232 follows main's highest ID, EX231, directly in the catalog table.

## Review changes and proof

- Classic CLI image input now uses the same BMP/PPM loader and board hook as S3.
  The real CLI boots the retained public-driver app and checks two indexed,
  non-zero 160 x 120 RGB565 frame hashes. Removing the option rejection alone
  is insufficient: the loader must also attach the picture to the classic board.
- Unknown-board usage lists `ai-thinker-cam`. README, board, chip and CLI docs
  name the board and the raw-only scope.
- Unsupported output emits one `[camera] unsupported sensor output` notice per
  board instance. A subprocess test checks its wording and exact count across
  repeated JPEG queries, supported output and a later unsupported mode.
- `powered_down` names the active-high PWDN state consistently. The compiler pin
  is `esp-14.2.0_20250730`; the lifecycle source-manifest link resolves.
- During an active capture, inverted sensor bytes are cached in the existing
  boxed capture, keyed by source Arc identity and inversion mask. The original
  source is retained so pointer reuse cannot yield a stale hit. Repeated valid
  MMIO route checks reuse one Arc; changing the picture or mask rebuilds it.
  Sensor bytes are never mutated. No field is added to the bus, ordinary PCM
  channel or per-tick state; camera-free dispatch remains behind its existing
  clock/running-DMA guard. Direct routed-frame queries without a capture retain
  the uncached fallback. This is a bounded camera-path optimization, not a CPU
  speed claim or new idle-path disassembly measurement.
- Route loss raises IN_DSCR_ERR as an explicit model cancellation policy,
  preventing a partial old frame from being spliced into a new one. This is
  not a hardware-derived mapping of every physical fault to that IRQ.

## Inputs and retained evidence

The four binary payloads under `esp32/tests/fixtures/classic-camera/` are
byte-identical to the published inputs at `7352bc81`. Both apps share one
bootloader and partition table; nothing camera-specific is published under
`web/wasm/fw/public`. The notices point to existing repository licence texts.
See [fixture input hashes](review-inputs.json) and the
[build recipes](../../../examples/classic-camera/README.md).

[Receipt curation](CURATION.md) retains compact outcomes and negative results;
[archive manifests](archive.json) fingerprint the immutable originals. All 120
original receipt files were independently verified against the published Git
revision after curation. There are 57 unchanged golden files and harnesses
relative to main `77e43c20`; runtime golden tests are also exercised.

## Executed checks

[Commands and final results](review-checks.json) record the local CI-equivalent
workflow, native/plain/ROM-enabled suites, both denied-warning Clippy targets,
production WASM demos/ABIs, worker/browser contracts, WASM differential tests,
profiling build and native AArch64 tests. Rust is pinned to 1.99.0; builds use
`nice -n 10`, two jobs and owned target directories. Workspace suites use an
empty HOME with explicit Cargo/Rustup locations, with ROMs explicitly set for
the ignored-test run and unset for the plain run. No golden is regenerated.

[Review negative controls](review-mutations.py) exercise the CLI image gate,
actual picture attachment, board usage, diagnostic count and inversion-cache
reuse/source/mask invalidation. The public-driver fixture mutations and focused
inherited PWDN/JPEG/routing controls are also rerun. Only compiled assertion
failures count as kills, never a build failure or timeout. Historical mutation
summaries remain distinct from these reruns.

## CPU comparison

Classic ESP32 CPU comparison is PENDING. No CPU timing was run for this review.
The maintainer supplied the following historical S3-only measurements in the
[October 21, 2026 review](https://github.com/mikroverk/esp32sim/pull/243#issuecomment-6309114561):
main `63670bb1` against camera head `7352bc81`, S3 hello for 30 emulated seconds,
seven alternating pairs per run, identical instructions and console hashes.

| Run | Main median user CPU seconds | Camera median user CPU seconds | Posted change | Camera slower |
| --- | ---: | ---: | ---: | ---: |
| 1 | 24.910 | 24.890 | -0.1% | 2/7 |
| 2 | 24.420 | 24.370 | -0.2% | 5/7 |

These are the maintainer's posted rounded aggregates, not a new measurement.
Round samples, load/warmup metadata and machine conditions were not supplied
with that comment. Neither reaches the slower-in-at-least-6/7 flag threshold.
This shows no S3 regression in those two historical runs; it neither measures
the classic I2S camera path nor clears the current integrated head. No browser,
production-speed or hardware-timing conclusion follows from this table.
