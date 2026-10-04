# Contributing to esp32sim

Thanks for helping. Bug fixes, new peripherals, new boards and new chips are all welcome. This page
says what a pull request needs to get merged; [AGENTS.md](AGENTS.md) holds the same rules in short
form for coding agents, plus the conventions for experiments and evidence.

## Before you start

- For a larger change (a new chip, a new subsystem, a new way of attaching devices), open an issue
  first so the approach can be agreed before the code is written.
- Keep one topic per pull request. When one pull request depends on another, register them as a
  stack (see "GitHub pull request stacks" in [AGENTS.md](AGENTS.md)). If GitHub reports that stacked
  pull requests are disabled, stack the branches by ancestry and name the pull request yours builds
  on at the top of its description. For a long series, open the pull requests one at a time as the
  earlier ones are reviewed.
- Check the open pull requests that touch the same files first (`gh pr list`, then each one's file
  list). A shared piece, such as a register block like IO_MUX, the `device_set!` table or the `Board`
  trait, gets one owner: build on the pull request that adds it instead of adding your own copy.
- Search [docs/experiments.md](docs/experiments.md) before a speed, timing or execution
  experiment: it lists what has been tried, including what did not work.

## Register and clock facts

- Cite the Espressif header **and its ESP-IDF version** for every register, bit, interrupt source or
  clock value, in code comments and in the pull request, for example
  `components/soc/esp32c3/include/soc/gpio_reg.h`, IDF v5.5.4.
- Check against the IDF version the firmware in question uses: Arduino-ESP32 2.x builds on IDF 4.4,
  Arduino-ESP32 3.x on IDF 5.x. Where they differ, say so; the UART's RC clock, for instance, is
  20 MHz in IDF 4.4 (`RTC_CLK_FREQ`) and 17.5 MHz in IDF 5.x (`SOC_CLK_RC_FAST_FREQ_APPROX`).
- Say what was compared with silicon and what was derived from another chip's model. Code copied
  from another chip keeps none of that chip's "verified on silicon" notes.

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

- **Tests that need a developer machine are `#[ignore]`d and named `external_*`.** A test that needs
  a local firmware build, a full objdump listing or hardware needs both: `#[ignore = "set FOO_DIR=…"]`
  naming its input keeps a plain `cargo test` green on any machine, and the `external_` prefix keeps
  it out of CI, which runs ignored tests too (`--include-ignored`) and skips only `external_*`. Without
  its input it fails with a message naming what it needs. Keep `external` out of every other test's
  name: `--skip` matches substrings, so CI would skip that test silently.
- **Golden outputs are bit-identical.** The golden-output tests compare console text, audio hashes
  and instruction counts for the committed demo firmware. If a change is meant to alter them,
  regenerate with `UPDATE_GOLDENS=1` and say in the pull request which goldens changed and why.

## Speed

The goldens pin instruction counts, and code that isn't in use must cost nothing.

- A feature behind a flag (such as `--ble`) leaves instruction counts and console output unchanged
  when the flag is off.
- If you touch the bus, the `device_set!` table, interrupt routing or anything that runs every
  tick, compare CPU time against main on the hello demos (S3 `hello`, `c3-hello`, `c6-hello`; their
  files are in `web/wasm/fw/*.json`), and on pocket-tank (`tools/fetch-pocket-tank.sh`) if you touch
  the S3 core. Build both trees with `cargo build --release --bins`, run each a few times,
  alternating main and your branch, and report the median `user` time; the instruction counts must
  match. For the C3, from the repository root:

```sh
/usr/bin/time -p target/release/esp32sim-c3 --boot rom --rom web/wasm/fw/esp32c3_rev3_rom.elf --flash-mb 4 \
  --bootloader web/wasm/fw/public/c3-hello-bootloader.bin --ptable web/wasm/fw/public/c3-hello-ptable.bin \
  --app web/wasm/fw/public/c3-hello_world.bin --max-seconds 30 --no-dump >/dev/null
```

## Experiments and evidence

Performance and timing work is recorded in [docs/experiments.md](docs/experiments.md), with
receipts under `docs/evidence/`. Follow [the evidence guide](docs/evidence/README.md), keep negative
results, and leave personal information out: run `node tools/check-evidence-privacy.mjs` before
pushing.

- A new row in `docs/experiments.md` goes directly below the previous row. A blank line ends a
  Markdown table, so a row after one renders as plain text.
- Evidence holds what someone needs to reproduce or challenge a result: commands, inputs, hashes,
  numbers and outcomes. Leave out agent-session narrative, and cite commits that exist in this
  repository, or name the public repository they are in.

## The pull request

- Say what the change does, how you checked it (the commands above and their results, plus any
  firmware you ran), and what it does not cover yet.
- Pull requests from a first-time contributor's fork wait until a maintainer approves the CI run.
  After the first merged pull request, CI starts on its own.

## After the review

Reviews here are thorough, and a pull request shouldn't stall on small things, so maintainers may
finish one themselves:

- **Small, clearly correct fixes** from a review may be pushed straight to your branch, without a
  round trip. Please leave "Allow edits by maintainers" on; where your fork can't allow it,
  maintainers use a branch of their own and credit you.
- **After 14 days** with no reply or push following a review, maintainers may finish the pull
  request. Small fixes go on your branch. Larger rework is either merged and fixed in a follow-up,
  or redone in a new pull request that keeps your commits or credits you with a `Co-authored-by:`
  trailer. Each change gets a note on the pull request saying what changed and why.
- **After 90 days** with no activity, a pull request is closed with a note. Reopen it whenever you
  pick it up again.
- If you need more time, say so on the pull request, and the clock stops.

## License

esp32sim is released under the [MIT License](LICENSE). By contributing, you agree that your
contributions are released under the same license.
