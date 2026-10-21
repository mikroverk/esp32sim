# Classic ESP32 public-driver camera fixture

Boots ESP-IDF normally on the AI-Thinker model, initializes the OV2640 through
unmodified esp32-camera and captures two 160 × 120 RGB565 frames in internal
DRAM. Each frame is 38,400 bytes; guest software SHA-256 must equal the
independent host-source oracle. No user firmware, symbol stubs or direct DMA
injection is used. No JPEG, PSRAM, network streaming or silicon validation is
claimed.

## Pinned build

- ESP-IDF v5.4.3: `ea1c174c1cbb7348bd8ba0ff1eb306246938dd80`
- esp32-camera: `b817e9b81041980608d1a3d3e0b08d86e3666382`
- esp_jpeg 1.3.1: dependency hash in `dependencies.lock` (not linked)
- Xtensa GCC `esp-14.2.0_20250730`, installed by the pinned IDF installer

From the repository root, choose a caller-owned scratch directory:

```sh
export IDF_TOOLS_PATH="$PWD/.camera-tools/tools"
git clone --depth 1 --branch v5.4.3 --filter=blob:none \
  https://github.com/espressif/esp-idf.git .camera-tools/esp-idf
nice -n 10 .camera-tools/esp-idf/install.sh esp32
. .camera-tools/esp-idf/export.sh
bash examples/classic-camera/build.sh "$PWD/.camera-tools/build-a"
```

The recipe fetches/verifies the exact camera revision. Keep its checkout
unmodified. IDF initializes its required submodules during configuration.
`CONFIG_APP_REPRODUCIBLE_BUILD=y` removes timestamps and maps source paths;
the project version is fixed. For a second clean build in a different source
and output directory, copy this example and its fetched components (without
`.git`, build outputs or generated sdkconfig), then run:

```sh
nice -n 10 idf.py -C "$PWD/.camera-tools/source-b" \
  -B "$PWD/.camera-tools/build-b" \
  -D SDKCONFIG="$PWD/.camera-tools/build-b/sdkconfig" build
python3 examples/classic-camera/check-builds.py .camera-tools/build-a \
  .camera-tools/build-b esp32/tests/fixtures/classic-camera
```

Both clean builds must match each other and all three committed input images.
Recorded hashes and licence inventory accompany the fixture in
`esp32/tests/fixtures/`; no SDK, driver source tree or local ELF is committed.

## Exercise

```sh
tools/fetch-demo-assets.sh --no-linux
export CARGO_TARGET_DIR="$PWD/.camera-target"
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" nice -n 10 cargo +1.99.0 test \
  --release -p esp32 --test camera_fixture -- --ignored --nocapture
python3 examples/classic-camera/expected-frame.py
# Optional freshly built images, without an ELF/symbol file:
nice -n 10 cargo +1.99.0 run --release -p esp32 --example camera_capture -- \
  .camera-tools/build-a web/wasm/fw/esp32_rev300_rom.elf
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" python3 examples/classic-camera/mutate.py \
  "$PWD" "$PWD/.camera-mutations"
```

The test requires exactly two indexed frame records, their dimensions/format/
length/hash, successful initialization and final completion. The Python oracle
packs the deterministic RGB source independently, without emulator imports.
Mutations count only compiled assertion failures, not timeouts or build errors.
Clean up caller-owned tools/builds/targets after use. This fixture adds no
emulator runtime code; CPU comparison is PENDING and no timing is claimed.
