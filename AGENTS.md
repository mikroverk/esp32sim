# Checks before a pull request

- Run what CI runs, with the newest stable Rust (`rustup update stable`): CI lints and tests with it, so an older local toolchain can pass where CI fails. [CONTRIBUTING.md](CONTRIBUTING.md) lists the commands; [.github/workflows/ci.yml](.github/workflows/ci.yml) is the full set.
- Both Clippy steps must pass with warnings denied: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings`. Clippy runs before the tests in CI, so a lint failure hides every test result.
- Run the tests as CI does, ignored tests included: `ESP32SIM_ROM_DIR=<roms> cargo test --release --workspace -- --include-ignored --skip external_`.
- A test that needs inputs only a developer machine has (a local firmware build, an objdump listing, hardware) must be named `external_*` and fail with a message naming what it needs. `#[ignore]` alone is not enough: CI runs ignored tests and skips only `external_*`.
- Golden outputs are bit-identical. Regenerate them (`UPDATE_GOLDENS=1`) only for an intentional change, and say in the pull request which goldens changed and why.
- In the pull request, list the checks you ran and their results.

# Experiment history

- Before proposing, implementing or benchmarking an ESP32-S3 execution, browser-speed or timing experiment, search [docs/experiments.md](docs/experiments.md) by mechanism and aliases.
- Cite the existing experiment ID and say what materially differs before retrying: mechanism, workload, correctness contract or measurement quality. A renamed branch is not a new experiment. Inspect preserved patches before rebuilding them.
- Record the outcome in the same entry, keeping earlier results, with revisions, inputs, exact work and output checks, conditions, uncertainty, adoption and a receipt. Negative results count. Use a new stable ID only for a materially different idea and cross-reference related IDs. Do not start a second list.

# Evidence retention and privacy

- Keep curated evidence needed to reproduce or challenge a conclusion: source revisions, commands, inputs, artifact hashes, numeric samples, correctness checks, relevant machine/software configuration and limitations. Retain negative results.
- Minimize personal information before committing. Omit application/process inventories, command lines unrelated to the experiment, login names, hostnames, email addresses, device identifiers and session details. Keep aggregate load or anonymous CPU samples when needed to explain measurement noise. `/Users/alice` and `/home/alice` are normalized labels, not host identities.
- Review text, JSON, scripts and compressed profiles. Require caller-supplied remote targets in reusable harnesses. Do not retain raw private captures just because they were collected; do not add private originals or backups to Git.
- Document redactions and their effect on evidence. Preserve measured values and historical artifact hashes; when sanitizing an existing receipt, record its original and sanitized file hashes so provenance remains explicit. Follow [the evidence guide](docs/evidence/README.md) and run `node tools/check-evidence-privacy.mjs` before publishing. The pattern check supplements manual review.

# GitHub pull request stacks

- Use `gh stack` when one pull request depends on another. Setting a PR's base branch alone does not register a native GitHub stack.
- For existing PRs, use `gh stack link --remote origin --base main <bottom-PR-URL> <top-PR-URL>` in dependency order, bottom to top. Follow any explicitly requested remote or base instead.
- For new stacks, use `gh stack init` and `gh stack submit`; inspect their current `--help` before acting.
- After linking existing PRs remotely, use `gh stack checkout <stack-number>` to import local tracking, then `gh stack view --json` to verify it. An untracked-branch error from `view` alone does not prove that the remote stack is absent.
- Verify the native GitHub stack registration before describing PRs as stacked. Distinguish branch ancestry from GitHub's stack feature.
- Do not rewrite published branch history merely to register an existing stack.
- If merge commits prevent `gh stack modify`, preserve published history and use `gh stack link` to add new PRs. When local tracking needs a clean refresh, use `gh stack unstack --local` followed by `gh stack checkout <stack-number>`; the `--local` flag leaves the GitHub stack intact.
