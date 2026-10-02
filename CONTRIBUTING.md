# Contributing to esp32sim

Thanks for helping. Bug fixes, new peripherals, new boards and new chips are all welcome. This page
says what a pull request needs to get merged; [AGENTS.md](AGENTS.md) holds the same rules in short
form for coding agents, plus the conventions for experiments and evidence.

## Before you start

- For a larger change (a new chip, a new subsystem, a new way of attaching devices), open an issue
  first so the approach can be agreed before the code is written.
- Keep one topic per pull request. When one pull request depends on another, register them as a
  stack (see "GitHub pull request stacks" in [AGENTS.md](AGENTS.md)).
- Search [docs/experiments.md](docs/experiments.md) before a speed, timing or execution
  experiment: it lists what has been tried, including what did not work.

## Checks to run before pushing

CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) installs the newest stable Rust on every
run, so update first: `rustup update stable`, plus `rustup target add wasm32-unknown-unknown` once
for the WebAssembly steps. A lint that a recent Clippy adds fails CI even when
an older toolchain passes.

The tests that boot firmware need the ESP32-S3, C3 and C6 mask ROM ELFs. They ship with ESP-IDF
(`~/.espressif/tools/esp-rom-elfs/`); without it, `tools/fetch-demo-assets.sh --no-linux` fetches
them into `web/wasm/fw/`.

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings
ESP32SIM_ROM_DIR=web/wasm/fw cargo test --release --workspace -- --include-ignored --skip external_
tools/wasm-build.sh && node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel
node tools/check-evidence-privacy.mjs
```

The first Clippy step runs before the tests in CI, so a lint failure stops the run before any test
result appears. If your change touches the WebAssembly JIT, also run `tools/wasm-jit-test.sh`; the
remaining CI steps are in the workflow file.

## Tests

[tests/README.md](tests/README.md) describes the test layers. Two rules catch most surprises:

- **Tests that need a developer machine are named `external_*`.** A test that needs a local
  firmware build, a full objdump listing or hardware must be named `external_…` and fail with a
  message naming the input it needs. CI runs ignored tests too (`--include-ignored`) and skips only
  `external_*`, so `#[ignore]` alone makes the test fail in CI.
- **Golden outputs are bit-identical.** The golden-output tests compare console text, audio hashes
  and instruction counts for the committed demo firmware. If a change is meant to alter them,
  regenerate with `UPDATE_GOLDENS=1` and say in the pull request which goldens changed and why.

## Experiments and evidence

Performance and timing work is recorded in [docs/experiments.md](docs/experiments.md), with
receipts under `docs/evidence/`. Follow [the evidence guide](docs/evidence/README.md), keep negative
results, and leave personal information out: run `node tools/check-evidence-privacy.mjs` before
pushing.

## The pull request

- Say what the change does, how you checked it (the commands above and their results, plus any
  firmware you ran), and what it does not cover yet.
- Pull requests from a first-time contributor's fork wait until a maintainer approves the CI run.
  After the first merged pull request, CI starts on its own.

## License

esp32sim is released under the [MIT License](LICENSE). By contributing, you agree that your
contributions are released under the same license.
