# Curated classic-camera receipts

All eight camera receipt phases live in this directory. These are historical
results, not newly executed builds, tests or CPU measurements.

| Phase | Receipt | Original-capture manifest |
| --- | --- | --- |
| AI-Thinker board | [board](board/README.md) | [archive](board/archive.json) |
| Shared sensor/frame state | [shared-state](shared-state/README.md) | [archive](shared-state/archive.json) |
| OV2640 SCCB | [sensor](sensor/README.md) | [archive](sensor/archive.json) |
| Classic receiver mode | [mode](mode/README.md) | [archive](mode/archive.json) |
| Classic FIFO | [fifo](fifo/README.md) | [archive](fifo/archive.json) |
| Classic routing | [routing](routing/README.md) | [archive](routing/archive.json) |
| DMA, STOP, arming and cancellation | [DMA](README.md), [STOP](rmw-stop.md), [arming](arming-window.md), [cancellation](cancellation.md), [fixture](fixture.md) | [archive](archive.json) |
| Public-driver lifecycle | [lifecycle](lifecycle/README.md) | [archive](lifecycle/archive.json) |

## Archive and provenance

Complete originals remain on the public fork at immutable revision
[`7352bc817954a6e5e287d5b2118c36315635c659`](https://github.com/schematik-engineering/esp32sim/tree/7352bc817954a6e5e287d5b2118c36315635c659/docs/evidence).
All 77 original JSON captures were fetched from that revision's public raw URLs
and verified byte-for-byte against Git before curation. No new release upload is
claimed, and this does not remove any bytes from existing Git history.

Each phase's `archive.json` records the original directory, immutable archive URL,
byte length and SHA-256 for every original file, plus the retained target and its
curated SHA-256. Append the original filename to `archive_url` to inspect its full
capture. The shared archive revision is the publication/retention anchor, **not**
the source revision for every historical measurement; each receipt retains its
own base/candidate revisions and historical source/artifact hashes.

Hashes keyed by old source paths still identify the original measured files.
They are not claims about relocated or edited files in the current tree. The
archive manifests separately fingerprint the current curated documents/scripts.
[Machine-readable relocation and deduplication mapping](curation.json) includes
file exceptions to the directory mapping; parent documents must use those too.

## What summaries retain and omit

- Mutation summaries retain every case name (including baseline), recorded row
  count, outcome flags, exit code and failure class where originally recorded,
  failed-test counts, suite/test selectors and deduplicated exact commands.
  Omitted replacement text, repeated source paths and failed-test name arrays
  remain in the immutable original and runnable mutation harnesses. Grouping
  never upgrades a runtime kill or unwrap failure to an assertion kill.
- Check summaries retain commands, exit status, environment, versions, source
  hashes, numeric results and limitations. Repeated per-binary test-result batches
  are summed field-by-field; batch counts remain explicit.
- Code-generation summaries retain artifact/disassembly hashes, comparison
  method, selected/identical body counts and instruction totals per binary,
  every differing body's name and instruction counts, and explanation results.
  Equal-body inventories, raw unified diffs, example instruction windows and
  call/guard captures remain archived. Equality is selected-body equality under
  the recorded normalizer, never whole-binary equality or a CPU measurement.
- Identical retained summaries share a canonical file; separate archive entries
  still distinguish each historical capture. Source/fixture manifests, frame
  hashes, driver-filter partial-group failures and original regression outcomes
  remain available locally. Numeric samples such as edge/start cycle pairs were
  not replaced by prose.

Negative results include the C-filter partial-group limitations, the original
STOP regression (EOF 512 rather than 0; RAW 768 rather than 0), the surviving
redundant tick-only cancellation mutant, non-assertion runtime kills and changed
code-generation bodies. See the phase receipts and their compact JSONs.

## Reproduction and limits

Retained Python harnesses keep their original positional interfaces; commands in
phase receipts now use the consolidated paths. Run mutation harnesses serially
in a disposable source tree matching the receipt's historical revision, with
caller-owned outputs under ignored `target/`. Replacement selectors are
revision-sensitive: a mismatch is not a kill. Do not run historical mutation
scripts against an unrelated source revision or count compile errors/timeouts as
successful mutations.

Capture/codegen harnesses and `summarize.py` scripts consume full raw outputs,
not the compact schemas in this directory. The latter scripts retain historical
regeneration behavior; use a disposable tree so regenerated raw receipts are not
mistaken for curated results. For a newly generated raw JSON, the standalone
compactor is:

```sh
python3 docs/evidence/classic-camera-dma/curate.py INPUT.json OUTPUT.json
```

Use a raw filename containing `mutations` for mutation compaction. Review the
result and provenance before committing; the compactor does not establish
archive availability or generate archival hashes. Current manifests describe
only this historical curation.

Guest-driver qualification is raw RGB565/YUV422, internal DRAM only. FIFO mode 3
is unit-tested, not guest-driver-qualified. JPEG and adjustable FPS are parked.
Route-loss `IN_DSCR_ERR` is an intentional emulator partial-frame cancellation
policy, not a disconnected-wire silicon claim. Classic ESP32 CPU timing is
pending; none was run during this curation. No claim of improved production
reliability, hardware validation or new browser integration is made.
