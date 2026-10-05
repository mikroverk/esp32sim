# Arduino LEDC and MCPWM validation

This receipt covers EX204. It validates the same Arduino sketch on ESP32-S3, C3 and C6 and records the register-level MCPWM check. No prior experiment matched the searched `LEDC`, `MCPWM`, `PWM`, `tone`, `Arduino`, `timer duty` or `duty timer` aliases.

## Provenance and conditions

- Upstream base: `dddb128052dca15250e2169b92ab73c4d87f524c`.
- Port source: Schematik fork `pr-1` at `221080ffb5ee8c19b8ce8b31d8953a93603b7fda`, merge base `d5446b4`.
- Executed candidate: `9cc58f1` (the following source-clock test and this receipt do not alter the executed model).
- PlatformIO Core 6.1.19, pioarduino platform `55.03.38+sha.fbdfc29`, Arduino-ESP32 3.3.8.
- Rust 1.96.0 (`aarch64-apple-darwin`), Darwin 25.6.0 arm64.
- The firmware boots from the matching Espressif mask-ROM ELF and the PlatformIO bootloader, partition table and application image. S3/C6 use 8 MiB flash; C3 uses 4 MiB.

The shared `platformio.ini` was:

```ini
[platformio]
default_envs = s3, c3, c6

[env]
platform = https://github.com/pioarduino/platform-espressif32.git#55.03.38-1
framework = arduino
monitor_speed = 115200

[env:s3]
board = esp32-s3-devkitc-1

[env:c3]
board = esp32-c3-devkitm-1

[env:c6]
board = esp32-c6-devkitc-1
```

Every environment compiled this unchanged sketch (SHA-256 `3a67a3a50f4744edc931bd5fd8f8870ee280944476dbd1408c7dc0e40fc4e734`):

```cpp
#include <Arduino.h>

constexpr uint8_t kPin = 4;

void setup() {
  Serial.begin(115200);
  delay(1000);

  const bool attached = ledcAttach(kPin, 5000, 8);
  const bool written = ledcWrite(kPin, 64);
  Serial.printf("LEDC pin=%u attach=%u write=%u requested_hz=5000 requested_duty=64\n",
                kPin, attached, written);

  delay(10000);
  ledcDetach(kPin);
  tone(kPin, 440);
  Serial.printf("TONE pin=%u requested_hz=440\n", kPin);
}

void loop() { delay(1000); }
```

## Exact work and checks

The build command was:

```sh
platformio run -d target/ledc-arduino
```

It completed all three environments successfully. The application image hashes were:

| Environment | `firmware.bin` SHA-256 |
| --- | --- |
| S3 | `b8c0b37d58b0fbc08f8ab2f622565fc2b1095af839773e28c77b2ae697be4eac` |
| C3 | `1e76d379994d7e2d509e9241bab2a5a2407df98452209bed2623dfca371368d0` |
| C6 | `b16c25151364941b64e3feac033116935d31920e135d1d9e53e366b6ec3e2fc8` |

Each image was booted twice with the release CLI. `CHIP` and `ENV` were respectively `s3/s3`, `c3/c3` and `c6/c6`; `ROM` was the matching `esp32s3_rev0_rom.elf`, `esp32c3_rev3_rom.elf` or `esp32c6_rev0_rom.elf`; `FLASH_MB` was 8, 4 or 8. The two runs used `SECONDS=5` and `SECONDS=15`:

```sh
target/release/esp32sim --chip "$CHIP" --boot rom --rom "$ROM_DIR/$ROM" \
  --bootloader "target/ledc-arduino/.pio/build/$ENV/bootloader.bin" \
  --ptable "target/ledc-arduino/.pio/build/$ENV/partitions.bin" \
  --app "target/ledc-arduino/.pio/build/$ENV/firmware.bin" \
  --elf "target/ledc-arduino/.pio/build/$ENV/firmware.elf" \
  --board none --flash-mb "$FLASH_MB" --max-seconds "$SECONDS" \
  --console all --pwm 4 --no-dump
```

| Chip | Stop | Console check | GPIO4 observation |
| --- | ---: | --- | --- |
| S3 | 5.000 s | `attach=1 write=1 requested_hz=5000 requested_duty=64` | 5000.000 Hz, 25.00% |
| S3 | 15.000 s | `TONE pin=4 requested_hz=440` | 440.005 Hz, 49.90% |
| C3 | 5.000 s | `attach=1 write=1 requested_hz=5000 requested_duty=64` | 5000.000 Hz, 25.00% |
| C3 | 15.000 s | `TONE pin=4 requested_hz=440` | 440.005 Hz, 49.90% |
| C6 | 5.000 s | `attach=1 write=1 requested_hz=5000 requested_duty=64` | 5000.000 Hz, 25.00% |
| C6 | 15.000 s | `TONE pin=4 requested_hz=440` | 440.005 Hz, 49.90% |

LEDC and MCPWM register tests run under `cargo test -p esp-periph`. They check timer/divider configuration, duty and compare latching, output routing, interrupt state, 20-bit C6 values and source-clock scaling. An Arduino MCPWM path was not used: the generic Arduino `ledcAttach`, `ledcWrite` and `tone` APIs exercise LEDC, while direct MCPWM setup would require a chip-specific ESP-IDF program.

## Limits and privacy

`--pwm` reports the configured steady-state frequency and duty through the GPIO matrix. It does not sample individual output edges. Hardware fades and MCPWM sync, capture, fault, carrier and dead-time behavior are not modelled. ESP32-C3 has no MCPWM peripheral.

No raw host capture is retained. Home-directory paths are normalized to `$ROM_DIR`; no hostname, login name, device identifier, unrelated process inventory or session data is recorded. The redaction does not affect firmware hashes, emulator values or the commands needed to reproduce the result.

## PR 167 review follow-up

Candidate `5075cd07429f78d8e6b5cae9f2b1db4d51eb6e37` corrects the matrix layout on C3/C6, uses 17.5 MHz for C6 RC_FAST,
and rejects MCPWM dead time and UTEP/UT0/UT1 actions. Five SoC tests write the actual peripheral
addresses and observe `pwm_output`. All 13 deliberate mutations failed assertions: eight wiring
mutations, RC_FAST, both old matrix decoders, dead-time bypass and extra generator actions.
Exact substitutions, commands and results are in [review167.json](review167.json).

Unconfigured LEDC returns no clock domain, so dispatch skips its tick and before/after IRQ reads.
Disabled MCPWM also returns no clock domain. LEDC phase units now cancel the factor of 256;
ordinary ticks use u64 arithmetic and remainder, with u128 only for overflowing large batches.
The exact-phase test includes `u64::MAX`; existing fractional-divider and latch checks pass.

This extends EX204 with stronger wiring checks and the requested c3-hello idle comparison.
Both executables use `cargo +1.99.0 build --release -p esp32sim`. Run each with:

```sh
target/release/esp32sim --chip c3 --rom web/wasm/fw/esp32c3_rev3_rom.elf --boot rom --flash-mb 4 --bootloader web/wasm/fw/public/c3-hello-bootloader.bin --ptable web/wasm/fw/public/c3-hello-ptable.bin --app web/wasm/fw/public/c3-hello_world.bin --max-seconds 30 --no-dump
```

CPU seconds are child user + system time, measured with Python `resource.getrusage` around
`subprocess.run`. The JSON retains revisions, binary/input hashes, run order and all samples.
Each recorded run retired 4,800,000,000 instructions over 30.000 emulated seconds.
The paired runs all printed Hello world and had the same normalized console SHA-256
`dbe0f29616984936966a482e1f4703e4bd3d6c4f4c41a2f2448981760739ab76`. This is also the hash of the baseline console confirmation run.

| Measurement | Published head CPU seconds | Candidate CPU seconds | Median change |
| --- | --- | --- | --- |
| Initial separate batches | 2.587584, 2.537439, 2.504536 | 2.640012, 2.651190, 2.732550 | +4.48% |
| Interleaved B/A, A/B, B/A | 2.641291, 2.673320, 2.633051 | 2.683554, 2.709617, 2.626860 | +1.60% |

The initial batches ran several minutes apart, so the second measurement changed the order to
reduce drift. The paired result is mixed, with a 1.60% slower candidate median;
it does not demonstrate a net speedup. Host activity and CPU frequency were uncontrolled.
Retain this negative result: removing idle device work is supported by the dispatch guard and
test, but does not establish zero whole-program overhead.

Both Clippy commands with warnings denied passed. The full release workspace suite with
`--include-ignored --skip external_` passed 485 tests, zero ignored, including unchanged goldens.
The wasm build and all eight requested demos passed. The restored-source PWM tests passed after
mutation checks. The evidence privacy check passed. No JIT code changed.

The earlier receipt's nonexistent base hash was corrected to
`dddb128052dca15250e2169b92ab73c4d87f524c`; earlier firmware results and artifact hashes are unchanged.
No personal paths, hostnames, usernames or raw private captures are included in this follow-up.

## Quiet-machine idle follow-up

The acceptance claim in this section was not reproduced independently. See the
[independent reproduction follow-up](#independent-reproduction-follow-up) for the
replacement implementation and two confirmation runs; the original samples remain unchanged.

The supplied quiet-machine result was main 2.430 s versus PR 167 2.504 s, +3.0%.
This extends EX204 and EX114 with explicit unused-device call counts and a comparison against
upstream `ed34b22`, using seven interleaved rounds after a warmup and a load threshold.
Earlier negative measurements above remain part of the record.

At `71ceb16`, c3-hello made 229,519,576 LEDC clock queries and 75,256,512 IRQ queries despite
zero LEDC writes and ticks. The earlier clock guard had removed ticking, not polling.
`f1abd7d` registers PWM blocks for ticking only while configured and caches their IRQ bits on
writes and ticks. Clock/reset changes refresh registration; paused and one-shot timers leave it,
while asserted IRQ bits remain until cleared. The optional tick helper stays outside the inlined
static-device loop. No scheduling quantum, instruction batch or timer deadline was changed.
The repeated profile made only 61 clock and 61 IRQ queries, all at configuration boundaries.
[Counter receipt](idle-profile.json) and [instrumentation script](instrument-idle.py).

The first implementation put the optional-device work inside the tick function; its partial
measurement was stopped while waiting above load 3. Its [patch](idle-inline.patch.gz) and
[samples](idle-inline-partial.json) are preserved; it has no seven-round estimate.
The outlined version still measured +0.976% versus main, above the requested +0.5% limit.
That [complete result](idle-intermediate.json) is retained. `f544d6d` then replaced the per-source
mask-and-merge with one cached merge per affected status word.

Final seven-round CPU user+system results:

| Arm | Revision | Median CPU s | Range CPU s |
| --- | --- | ---: | --- |
| Main | `ed34b22` | 2.425023 | 2.397953–2.446829 |
| Published PR 167 | `d59b5d2` | 2.462887 | 2.449728–2.524860 |
| After | `f544d6d` | 2.432365 | 2.414160–2.483557 |

After is **+0.303% versus main**, with overlapping ranges, satisfying the
requested limit. Published is +1.561% versus main in this comparison.
All 21 accepted samples retired 4,800,000,000 instructions and cycles over 30.000 emulated
seconds, with zero exceptions, 3028 interrupts and the same console SHA-256
`3b9d8d3eb053a97670f11896674e70ffc968fd7a803cb2cdc5c051414af94efd`.
The accepted start/end 1-minute loads were below 3, maximum
2.936523.
Pauses were required while other jobs ran; attempts ending at load 3 or higher were discarded.
These ranges include remaining host/frequency variation. The claim is the requested native
idle regression bound, not a general emulator speedup.

[Full final samples, input and binary hashes](idle-final.json).
The [benchmark script](benchmark-idle.py) adapts the supplied `/tmp/bench-quiet.py` only to add
load gating, separate output names and correct stale three-round metadata. Build every arm in
its own source directory and target directory with `cargo +1.99.0 build --release -p esp32sim`.
The measured builds used `CARGO_TARGET_DIR=/tmp/pr167-main-target`,
`/tmp/pr167-published-target` and `/tmp/pr167-candidate2-target` respectively.
Then run:

```sh
python3 docs/evidence/pwm-arduino-2026-10-02/benchmark-idle.py "$FW" \
  main=/tmp/pr167-main-target/release/esp32sim \
  published=/tmp/pr167-published-target/release/esp32sim \
  after=/tmp/pr167-candidate2-target/release/esp32sim
```

`FW` is the caller's firmware directory containing the ROM ELF and `public/` demo images.
The JSON records the exact CLI arguments, firmware hashes and toolchain. No personal paths,
process inventories, hostnames or login names are retained. The benchmark JSON was already
free of those fields; only provenance, summary statistics and clarified limitations were added.
Its original file hash is retained as `source_result_sha256`.

Final validation on the word-merge implementation: both Rust 1.99.0 Clippy commands with
warnings denied passed; the release workspace suite with `--include-ignored --skip external_`
passed 487 tests, zero ignored, 13 filtered. Goldens stayed bit-identical. The wasm build and all
eight requested demos passed. The new dispatch and SoC tests cover zero idle polling, activation,
clock gating, pause, one-shot removal, asserted IRQ retention, clear and reset. No JIT code changed.

## Independent reproduction follow-up

The earlier +0.303% result did not reproduce independently. A supplied quiet run of
894ffa4 measured main 2.408 s, range 2.379–2.437, versus the branch 2.449 s,
range 2.436–2.488, at load 1.7–2.2. An earlier loaded comparison was +1.6%.
Those supplied summary values have no raw samples in this receipt. A fresh local
seven-round comparison reproduced the regression: main 2.402527 s versus
894ffa4 at 2.438191 s, +1.484%. The original published branch remains d59b5d2;
its historical seven-round comparison was 2.462887 s against main 2.425023 s.
Neither historical number is substituted into the new paired comparisons.

EX204 is extended with compiled-code inspection and two required confirmation
runs. EX114 covers inactive dispatch; EX133's existing Machine layout keeps
optional state at the tail. This follow-up concerns the PWM peripheral fields,
not virtual quanta or a scheduler change.

The local arm64 release disassembly showed that the old optional_words array and
loop were already constant-folded. The problem was merging an unrestricted
cached u32: Lines::update could no longer eliminate unrelated source bits. The
exact compile-time mask restores that elimination. A pending-IRQ flag was also
tried, but LLVM converted it to unconditional loads and a conditional select;
it added four instructions over the mask-only form. It failed both comparisons
at +0.675% and +0.555%, so it was dropped. Mask alone measured +0.785% and was
insufficient. Moving the inline PWM fields behind the existing peripherals
restored C3's interrupt-map offset from 0xa30 to main's 0x9a0. That variant
measured +0.358% then +0.760%, so it did not satisfy the two-run requirement.

The original 894ffa4 C3 refresh_irq function had 354 static instructions and
21 vector loads, versus main's 341 and 19. The exact mask reduces that to
344 and 19. These are static disassembly counts, not dynamic CPU instruction
counts. Ordinary register writes already omit refresh_optional. The extended
OptionalChip test verifies an ordinary write and a static interrupt scan do
not poll the optional device. Existing tests preserve a stopped timer's asserted
interrupt, which prevents gating cached sources on active_optional alone.

The final change in `529815e` also moves optional ticking before the static device
loop. This shortens the delta-count lifetime across device calls. The compiler
saves three register pairs instead of four and reduces C3's tick stack frame
from 192 to 176 bytes. It partially unrolls the static loop, increasing its static
code size from 156 to 283 instructions. No scheduler quantum, clock advancement,
divider, static-device order or deadline behavior changed. The exact compiled
summaries and binary hashes are in [idle-compiled-profile.json](idle-compiled-profile.json).
Rust field layout is compiler-dependent; the offset comparison describes these
Rust 1.99.0 arm64 builds, not a portable ABI guarantee.

Two consecutive seven-round comparisons of the same final binary passed:

| Run | Main median CPU s | After median CPU s | Difference | Main range s | After range s |
| --- | ---: | ---: | ---: | --- | --- |
| 1 | 2.405154 | 2.407156 | +0.083% | 2.396368–2.424965 | 2.373718–2.419441 |
| 2 | 2.409781 | 2.399303 | −0.435% | 2.379705–2.416869 | 2.377932–2.420488 |

Both are within +0.5% with overlapping ranges. These results establish the
requested local idle bound, not a general speedup. Each comparison used one
excluded warmup per arm, seven interleaved rounds, reverse order on even rounds,
and child user+system CPU time from getrusage. Main was `ed34b22`; the final
implementation is `529815e`. The maximum accepted start/end 1-minute loads were
2.673828 and 2.203613 respectively. Run 1 waited four times and discarded one
attempt that ended above load 3; run 2 needed neither. No builds or tests ran
concurrently with accepted benchmark samples.

All 28 accepted samples retired 4,800,000,000 instructions and cycles over
30.000 emulated seconds, with zero exceptions, 3028 interrupts and console SHA-256
`3b9d8d3eb053a97670f11896674e70ffc968fd7a803cb2cdc5c051414af94efd`.
[Run 1](idle-confirmation-run1.json) and [run 2](idle-confirmation-run2.json) retain
all samples, toolchain, firmware hashes, binary hashes, load and acceptance checks.
The [reproduced baseline](idle-recheck-baseline.json), rejected
[flag run 1](idle-mask-flag-run1.json), [flag run 2](idle-mask-flag-run2.json),
[mask-only run](idle-mask-only.json), [layout run 1](idle-layout-run1.json) and
[layout run 2](idle-layout-run2.json) remain available. Their patches apply to
`894ffa4`: [mask with flag](idle-mask-flag.patch.gz), [mask only](idle-mask-only.patch.gz),
and [mask with tail fields](idle-layout.patch.gz). The last variant passed once
and failed its confirmation; both results are retained.

Reproduce with the existing [benchmark script](benchmark-idle.py), a main checkout
at `ed34b22`, this checkout at `529815e`, and caller-supplied firmware directory `FW`.
Build main with `CARGO_TARGET_DIR=/tmp/pr167-confirm-main-target` and this branch
with `CARGO_TARGET_DIR=/tmp/pr167-tick-target`, each using
`cargo +1.99.0 build --release -p esp32sim --bin esp32sim` from its own checkout.
Then run twice, preserving each output before the next run:

```sh
for run in 1 2; do
  python3 docs/evidence/pwm-arduino-2026-10-02/benchmark-idle.py "$FW" \
    main=/tmp/pr167-confirm-main-target/release/esp32sim \
    after=/tmp/pr167-tick-target/release/esp32sim
  cp /tmp/pr167-quiet.json "/tmp/pr167-confirmation-$run.json"
done
```

The measured local script copies differed only in output prefixes, so they did
not overwrite prior captures. Original result-file hashes are retained in the
curated JSON. No personal fields required removal; raw CLI logs and full
assembly remain under `/tmp`. Only relevant compiled-code summaries and aggregate
load are committed. Patches were inspected after decompression.

Final validation on `529815e`: both required Rust 1.99.0 Clippy commands passed
with warnings denied. The release workspace suite passed 487 tests, zero ignored,
13 filtered, with bit-identical goldens. The wasm build and all eight requested
demos passed. The extended dispatch test covers ordinary writes without optional
polling, shared clock deltas, activation, one-shot removal, stopped pending IRQs
and clear. Existing SoC tests cover gates and resets. JIT code is unchanged.
Privacy validation after staging the new receipts passed: 1492 tracked evidence files,
19 gzip files, no configured patterns. Manual review included the JSON and decompressed patches.


## C6 idle review on main 5ab228fa

EX204 follow-up on `5ab228fadf92e385d1b175b4d12f48f57aa22c8d`; fix
`f3eb0b69e6e9fa8a20f4d4cf887def777b5e2f6d`. This checks C6 IRQ code generation
on the new main revision, extending the earlier C3 idle work rather than changing
timer scheduling. The existing LEDC/MCPWM clock methods already reject unconfigured
timers. A diagnostic build asserted an empty optional-device schedule on every tick
of the 30-second c6-hello workload: 4.8 billion instructions/cycles, zero exceptions.
The hermetic `reset_pwm_stays_unscheduled_with_clocks_enabled` test also covers
reset PCR values, explicitly enabled gates, and reboot.

The C6 dispatcher lacked the existing C3 `inline always` hint. With cached PWM IRQ
sources, the compiled C6 IRQ refresh had an unconditional 128-byte stack frame and
497 instructions, versus main's 474. Applying the hint restores the entry without
that frame and reduces the function to 477 instructions. Main and the fix both
retain a conditional 48-byte frame elsewhere in the function. Moving cache merging
earlier or into individual table entries did not remove the unconditional frame;
these two alternatives were rejected without timing. Their patches are retained.
This establishes the code-generation difference, not a measured CPU-time delta.

[Code generation and diagnostic reproduction](c6-idle-codegen.json),
[checks](c6-idle-checks.json), [cache-first attempt](c6-cache-first.patch),
[per-entry attempt](c6-cache-entry.patch).

Both release builds use Rust 1.99.0 and separate `CARGO_TARGET_DIR`s with
`cargo +1.99.0 build --release --bins`. Reproduce each five-round comparison using:

```sh
python3 docs/evidence/pwm-arduino-2026-10-02/benchmark-review3.py c6-hello c6.json main=MAIN_TARGET/release after=PR_TARGET/release --fw web/wasm/fw
python3 docs/evidence/pwm-arduino-2026-10-02/benchmark-review3.py pocket-tank pocket.json main=MAIN_TARGET/release after=PR_TARGET/release --fw web/wasm/fw
```

The script warms each arm once, reverses arm order in even rounds, captures user and
system CPU time separately, and checks instruction counts, cycles and console hashes.
It waits for `uptime` one-minute load below 3 and rejects runs ending at load 3 or
higher. The main Speed example names C3; C6 uses the same options with its executable
and manifest files. Pocket-tank uses its manifest's board, PSRAM and model image.
CPU comparisons were blocked after 42.0 minutes of retries: 85 one-minute load samples ranged from 3.07 to 21.13, all above the required 3. No warmup or measured round ran; C6 and pocket-tank ±1% parity remain unverified. See the [load gate receipt](c6-idle-load-gate.json).


## C6 idle USER-time follow-up

The load restriction was removed for this comparison. The PCR reset-clock guess
was not the cause: the reset test and earlier every-tick assertion establish that
unused PWM remains unscheduled. Ten-second samples of both binaries place most
samples in IRQ refresh, tick and deadline handling, with none in optional ticking.
Disassembly identifies the remaining optional-list check and cached IRQ merge;
C6 tick grows from 2346 static instructions on main to 2353 at `696a92f`.

The adopted change sizes clock-delta scratch storage to `Self::CLOCKS.len()`.
`ClockTree::advance` emits at most once per declared clock, so no entry is lost.
This removes eight scalar initialization stores in C6 tick and reduces its frame
from 496 to 448 bytes (2345 static instructions). The oversized scratch array also
exists on main: this saving offsets the added optional bookkeeping; it does not
remove the optional-list check or IRQ merge. Clock gates, configuration-triggered
registration, interrupt caching and tick order are unchanged.

Boxing C6 PWM state reduced the peripheral structure from 8544 to 8280 bytes
(main: 8224), but its initial +0.61% result did not confirm (+1.92%). Marking
optional ticking cold measured +1.11%. Both alternatives were rejected; their
patches and measurements are retained in the [receipt](c6-idle-user-time.json).

The unmodified supplied harness measured C6 at −0.11%; a confirmation copy measured
+0.20%, S3 hello −0.83%, C3 hello −1.10%, and pocket-tank −0.20%.
Instruction counts match in both arms for every workload. Each comparison uses nine alternating
rounds after one excluded warmup per arm and median child USER CPU time. Main is
`5ab228fa`, with separate release target directories and Rust 1.99.0. Background
load was uncontrolled; no task builds overlapped measurement. These measurements
supersede the earlier blocked comparison, not its historical load samples.

Reproduce the confirmation with the retained copy (only paths parameterized):

```sh
python3 docs/evidence/pwm-arduino-2026-10-02/benchmark-user-time.py --fw ROM_AND_FW_DIR --model MODEL_Q4_BIN --main MAIN_TARGET/release --pr PR_TARGET/release
```

The receipt retains every printed round, instruction counts, input and binary
hashes, rejected measurements and function-level profile counts. Original rounds
were printed to 0.01 seconds; the confirmation prints six decimals. Raw profiler
headers and process/binary inventories are omitted; raw hashes are retained.
Redaction does not change the reported function counts. No browser-speed or
hardware-timing claim follows from these local native measurements.

Validation on the adopted change: both Clippy commands pass; 544 CI-style tests
and 529 plain tests pass (29 ignored); all eight WASM demos pass. Goldens remain
bit-identical. No JIT code changed. Exact commands are in the receipt.
