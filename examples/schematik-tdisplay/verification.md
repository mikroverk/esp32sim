# Verification receipt

Stack: main `142805d5`, RTC CRC `837ec1ce`, deep sleep/EXT0 `91b204cf`,
then unmodified patch-equivalent cherry-picks of #237 (`dd42fc62`,
`b72df388`). Demo changes are firmware, static web content and the browser
smoke-test registration list only; no Rust/emulator changes.

## Inputs and build

`SHA256SUMS` pins the original starter source, images, notices and rebuild
source archive. The source is the public `esp32-digital-hair-clip` seed's
`seed.code.code` plus a final newline (SHA-256
`8419e0f852307fec604a5e1e712823e2780a7090cb3fac24fca95672ce395494`).

All builds use `nice -n 10`; Rust uses `cargo +1.99.0` with a separate
`CARGO_TARGET_DIR`. For `tools/wasm-build.sh`, `RUSTUP_TOOLCHAIN=1.99.0`
is set and the worktree's `target` points at that separate target directory
(the script copies its artifact from `target/`).

- `pio run -d examples/schematik-tdisplay`: pass.
- `python3 examples/schematik-tdisplay/package.py`: pass; deterministic source
  archive metadata (zero uid/gid/mtime, empty owner/group, no local paths).
- From this directory, `shasum -a 256 -c SHA256SUMS`: six inputs pass.
- `cargo +1.99.0 build --release --bin esp32sim`: pass.
- `tools/fetch-demo-assets.sh --no-linux`: pass.

## Behaviour

Use the native command in README, adding the firmware ELF for symbol names
and `--trace-fn '_Z5setupv$'` for setup calls. Button scripts use seconds and
150 ms pulses:

| Script | Stop time | Observed result |
| --- | --- | --- |
| none | 1 s | `Schematik Hairclip`; 207 RAMWR, 33,220 pixels, backlight on |
| `0.6 press 35 150` | 1 s | `Schematik is great`; backlight on |
| `1 press 0 150` | 2 s and 2.5 s | blank; backlight off; both PCs and instruction counts unchanged |
| `1 press 0 150`, `3 press 0 150` | 4 s | `rst:0x5 (DEEPSLEEP_RESET)`, setup at 3.0123 s; first frame redrawn pixel-identically |

During sleep core0 stays at `0x40084795` with 6,973,882 instructions;
core1 at `0x40086b91` with 93,508,796 instructions. CCOUNT continues advancing;
this observation is about parked instruction execution, not frozen clocks.
Initial and next-message landscape PNGs were visually inspected.

## Regression checks

- `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings`: pass.
- With empty HOME, retained Rust installation paths and only the ROM fixture
  directory configured: `cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_`:
  1,003 pass, zero failures.
- Empty HOME, no ROM environment: `cargo +1.99.0 test --release --workspace`:
  975 pass, 50 ignored, zero failures.
- `RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh`: pass.
- `node tools/wasm-test.mjs tdisplay-schematik hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel`:
  all nine pass, including the JIT handoff check; the new demo produces one
  binary display frame. Separately `tdisplay-prices` also passes.
- Mutation: new manifest `min_frames` raised from 1 to 1,000,000; same runner
  exits 1 with `1 binary frames, wanted 1000000`. Original manifest restored.
- `node tools/check-wasm-sections.mjs web/wasm/esp32sim.wasm`: pass,
  DWARF absent, function names retained.
- Headless Chromium: desktop 1440×1000 and mobile 390×844. Document widths
  equal viewport widths, thumbnail contained, rendered/source ratios both
  16:9, catalogue entry grouped under ESP32. Live GPIO35 changes the frame;
  GPIO0 blanks it; a second press restores the initial canvas bytes. No page
  exceptions. Screenshots retained with the local review evidence, not Git.
- `node tools/check-evidence-privacy.mjs`: pass.
- `git diff --check`: pass.

CPU: not applicable (web-only, no emulator code changed)

No physical-board verification. Browser button pulses are wall-clock UI
presses, not a hardware timing comparison. The native replay is the timed
sleep/wake check. No CPU benchmarks were run.
