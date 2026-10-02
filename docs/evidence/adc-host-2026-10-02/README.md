# EX207: ADC host inputs and completed conversions

The S3 ADC1 path from PR #165 now accepts raw counts and reports completed conversions.
The same host contract covers S3 ADC2, C3 ADC1/ADC2, and C6 ADC1. Existing voltage
sources and S3 ADC1 transfer behavior remain compatible.

Implementation revision: `7ce98837b93a90444d3da93c52352b9b664238c2`.
Base: `0334d422468aa74442ba506bacb4a62f1b66b327`, PR #165, unchanged.
Behavior reference: `221080ffccfa829106b398f896653535853c76c8:esp-periph/src/adc.rs`.
The reference was inspected with `git show` and related diffs against `dddb128`.
Its audio handling, delayed completion, product ABI, and sentinel conventions were not copied.
No dependency on classic ESP32 PR #168 or the fork is introduced.

The experiment catalog at the base has no ADC, analog, SAR, or SENS entry. EX207
adds a different correctness contract from #165: raw input, per-pin generations,
S3 ADC2, and C3/C6 one-shot controllers. This is not a speed or hardware-timing
experiment. EX199–EX204 remain reserved for other open work.

## Results

All four firmware paths passed. Each table cell contains raw count / calibrated mV.
Each phase runs `analogRead` followed by `analogReadMilliVolts`, so each pin's
generation is exactly 2, 4, 6, then 8. Register tests separately prove one increment
per START rising edge, no increment on reads or setters, and no repeated conversion
while START remains high.

| Input, in run order | S3 GPIO1, ADC1 | S3 GPIO11, ADC2 | C3 GPIO0 | C6 GPIO0 |
| --- | --- | --- | --- | --- |
| 0.5 V | 521 / 500 | 528 / 500 | 722 / 500 | 508 / 500 |
| raw 1024 | 1024 / 960 | 1024 / 944 | 1024 / 706 | 1024 / 1008 |
| 1.0 V | 1068 / 1000 | 1087 / 1000 | 1459 / 1000 | 1016 / 1000 |
| raw 3072 | 3072 / 2765 | 3072 / 2718 | 3072 / 2081 | 3072 / 3016 |

[Serial output](serial.txt) preserves all 16 reading pairs and observed generations.
[Results](results.json) records the 32 completed conversions and instruction/cycle totals.
The test asserts raw count, calibrated mV, observed count, and generation delta for every
pair. Firmware waits for a UART byte before each phase, so input changes happen between
completed reads. No sketch, driver, ROM routine, or firmware result is patched at runtime.

The normal workspace run passed 467 tests, failed zero, and ignored 25, including the
three external-firmware checks. The separate release ADC run passed all six tests,
including those three firmware checks. Release and WASM builds passed. See
[checks and artifact hashes](checks.json) and [input hashes](inputs.json).

## Reproduction

Use the committed sketch unchanged. Build outside the repository:

```sh
work=/tmp/esp32sim-up-adc-arduino
mkdir -p "$work/src"
cp docs/evidence/adc-host-2026-10-02/platformio.ini "$work/platformio.ini"
cp docs/evidence/adc-host-2026-10-02/main.cpp "$work/src/main.cpp"
PLATFORMIO_CORE_DIR=/tmp/esp32sim-up-adc-pio-core pio run -d "$work"
ADC_FIRMWARE_DIR="$work/.pio/build" \
ADC_ROM_DIR="$HOME/.platformio/packages/tool-esp-rom-elfs" \
cargo test --release -p esp32sim --test adc -- --include-ignored --nocapture --test-threads=1
cargo build --release
cargo test --workspace
tools/wasm-build.sh
node tools/check-evidence-privacy.mjs
git diff --check
```

The fixture harness in `cli/tests/adc.rs` loads each chip's `esp32<chip>_rev0_rom.elf`
and places `bootloader.bin`, `partitions.bin`, and `firmware.bin` at flash offsets
0, 0x8000, and 0x10000. It boots through ROM with the default interpreter/JIT selection.
The library harness is necessary to inspect the public host API; no product WASM ABI
or extra CLI command was added. The CLI equivalents are documented in `docs/cli.md`.

Measured environment: Darwin arm64, Rust and Cargo 1.96.0, PlatformIO 6.1.19,
platform `https://github.com/pioarduino/platform-espressif32.git#55.03.38-1`,
Arduino 3.3.8, bundled IDF libraries `5.5.4+sha.735507283d`. The isolated PlatformIO
core reused installed tool/toolchain packages through symlinks and downloaded the
3.3.8 core and libraries. The firmware `.pio` directory was deleted after recording
hashes and results. Firmware binaries and ELFs can be regenerated from the retained
sketch, configuration, versions, and commands.

`predict.py <source-directory>` independently calculates the expected table from
Espressif's coefficient files. The source directory contains `esp32s3-curve.txt`,
`esp32c3-curve.txt`, and `esp32c6-curve.txt`, downloaded from these IDF v5.5 paths:

- [S3 coefficients](https://github.com/espressif/esp-idf/blob/v5.5/components/esp_adc/esp32s3/curve_fitting_coefficients.c)
- [C3 coefficients](https://github.com/espressif/esp-idf/blob/v5.5/components/esp_adc/esp32c3/curve_fitting_coefficients.c)
- [C6 coefficients](https://github.com/espressif/esp-idf/blob/v5.5/components/esp_adc/esp32c6/curve_fitting_coefficients.c)

The register definitions and calibration reference points were checked against
`components/hal/<chip>/include/hal/adc_ll.h`,
`components/soc/<chip>/register/soc/apb_saradc_reg.h`, and
`components/efuse/<chip>/esp_efuse_rtc_calib.c` at the same tag. Hashes are in
`inputs.json`. S3 ADC1 predictions retain #165's 1,000,000-scale voltage inverse;
firmware millivolt predictions use IDF 5.5's 65,536-scale arithmetic.

## Failed attempts and limits

The first PlatformIO command failed with
`PermissionError: [Errno 1] Operation not permitted: '/Users/alice/.platformio/platforms.lock'`.
The single retry used the isolated core above and built all three boards successfully.
An internal esptool installation warning appeared during bootstrap but did not prevent
any final firmware build. The first focused test build failed with Rust E0061 because
the C3/C6 test constructors omitted `flash_size`; adding `4 << 20` fixed both errors.
The next focused run passed. The first privacy check rejected a normalized home label
followed by a period; enclosing the label in backticks fixed that pattern match. No ADC run produced an incorrect sample or generation.

Defaults use the existing synthetic eFuses, with zero calibration differences.
S3 and C3 select V1; C6 block revision 0.3 selects V2. No new physical calibration
values were invented. [Host ADC reference](../../peripherals.md#host-adc-inputs)
lists the reference codes and public API. A custom eFuse image does not retune the
voltage curve. Raw injection remains available for alternate calibrations.

One-shot completion is instantaneous, as in #165. No conversion-latency accuracy,
noise, continuous/DMA acquisition, ADC interrupt delivery, or data inversion is
claimed. C3 ADC2 was tested through MMIO, not Arduino, whose default IDF configuration
restricts that unit. No browser runtime or physical-board comparison was run; WASM
validation here is a successful build. The inverse curves use an exhaustive 4096-count
search, matching #165's approach; there is no throughput claim.

Only curated ADC serial lines, public input hashes, aggregate test outcomes, and
reproduction commands are retained. Build paths with a personal home label were
normalized to `/Users/alice` in the error excerpt. Other compiler output, boot banners,
and package-install chatter were omitted; measured samples and work counts are unchanged.
No private source capture or compressed profile is included. Manual review and the
repository's evidence privacy checker cover the retained files.
