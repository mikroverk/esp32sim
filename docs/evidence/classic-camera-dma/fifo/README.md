# Classic camera FIFO packer

Base: `42d5c9dbe29827bfe2271777b74e7af914559c2f` (register-only camera adapter).
This is a standalone, local-only C5 slice, not working classic camera capture.

## Contract

`esp32::camera_fifo::CameraFifo` accepts only modes 1 and 3. Its caller owns the
capture lifecycle and supplies row boundaries/reset; the object has no register,
GPIO, DMA, audio or scheduler connections and adds no fields to existing hot structs.
Mode 1 maps sensor bytes s1,s2 to little-endian bytes `[s2,0,s1,0]`;
mode 3 maps each s1 to `[0,0,s1,0]`. RGB565 bytes are retained, not pixel-swapped.
A single pending mode-1 byte survives arbitrary input splits. `end_row` pads an
odd tail's sample2 with zero and consumes it; the caller retains true row length.
Padding an odd row is a deterministic model policy, not verified silicon behavior.
Reset discards only the unfinished word; caller-owned emitted output is not undone.
Reconfiguration requires a new packer (fallible constructor), not reuse of stale state.

Source: public espressif/esp32-camera v2.1.4,
`b817e9b81041980608d1a3d3e0b08d86e3666382`, `target/esp32/ll_cam.c`:
62-85 (layout/modes), 123-209 (filters), 501-547 (mode/filter selection).
`source.json` pins the full downloaded file SHA-256. No hardware check was performed.

## Reproduce

Use Rust 1.99.0 (`b940084d7`, LLVM 23.1.1), an isolated worktree and OUT under
a separate build/evidence directory. Set `CARGO_TARGET_DIR=OUT/target`.

- `tools/fetch-demo-assets.sh --no-linux`
- `nice -n 10 cargo +1.99.0 test --release -p esp32 --lib camera_fifo`
- `python3 docs/evidence/classic-camera-dma/fifo/verify-driver.py REPO OUT/driver`
- `python3 docs/evidence/classic-camera-dma/fifo/mutate.py REPO OUT/mutations`
- Required native/WASM commands and exits: `checks.json`. Both test suites ran
  with an empty HOME, explicit CARGO_HOME/RUSTUP_HOME, and respectively only the
  ROM override or no ROM override. 996 ROM-enabled / 968 plain tests passed.
  Native and WASM Clippy, WASM build and all eight specified WASM demos pass.
- For codegen, build `cargo +1.99.0 build --release -p esp32sim --bin esp32sim`
  at base in `OUT/base-target` and candidate in `OUT/target` (both nice -n 10).
  `python3 docs/evidence/classic-camera-dma/fifo/codegen.py REPO OUT` asserts all
  five selected classic bodies identical after address/call normalization.
  I2S step/data are inlined into tick (2207 instructions at both revisions).
- `node tools/check-evidence-privacy.mjs`; `git diff --cached --check`.

Six FIFO tests pass, including exact little-endian words, RGB565/YUV round trips,
luma selection, every two-split partition of a 256-byte stream including empty
chunks, row tails/idempotent end-row, reset and all 254 unsupported mode values.
`mutations.json` records 12/12 named assertion kills, zero compilation-error kills.
The shared audio/PCM tests and non-camera goldens pass unchanged; no existing
golden/firmware file was edited. There are no JIT changes.

## Actual public driver extraction and limits

The verifier downloads the hash-pinned source and compiles its actual union and
four filter functions with `cc -std=c11 -Wall -Wextra -Werror`, not rewritten
approximations. The Rust example emits packed vectors. Thirteen exact C-filter
recoveries pass for RGB565/YUV, both sampling modes, luma and aligned rows, plus
the mode-3 grayscale remainder on a 12-byte row. SHA-256 outputs are retained in
`driver-results.json`.

Three negative 12-byte partial-group cases are also reproduced and retained:
mode-1 YUV returns its declared length but only writes the first eight bytes;
mode-1 grayscale writes only four of six luma bytes; mode-3 YUV's highspeed tail
uses sample2 and returns 16 rather than 12 bytes. They are NOT successful
recoveries. The driver's tail path serves overlapping mode 0, which is unsupported
here. Do not fabricate sample2 duplicates or alter mode-3 word packing to make
that tail appear valid. Future DMA integration must deliver driver-compatible
filter chunks (eight sensor-byte groups for raw mode-1/mode-3 RGB/YUV); odd-row
padding alone does not make an arbitrary public-driver chunk supported.
Native packer tests cover arbitrary byte tails independently of that driver
chunk constraint. No JPEG, complete camera configuration, guest capture, sensor
routing, DMA delivery or product-support claim is made.

## CPU comparison

PENDING

No timing was run. Selected native instruction-body equality is not a CPU table.
