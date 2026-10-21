# Classic camera lifecycle fixture

This is a second build of the same pinned ESP-IDF / public esp32-camera project,
using `main/lifecycle.c` rather than the original two-frame `main.c`. The C8
images and checks remain unchanged. The lifecycle variant uses RGB565 QQVGA,
one framebuffer and internal DRAM; no driver changes, symbols or host stubs.

Use the pinned checkout/install procedure in README.md. The IDF installer
currently selects Xtensa GCC esp-14.2.0_20250730. Python 3.12 is supported; when
several Python versions are installed, create/select that environment explicitly
(the Python executable must not itself belong to another virtual environment):

```sh
python3.12 "$IDF_PATH/tools/idf_tools.py" install --targets=esp32
python3.12 "$IDF_PATH/tools/idf_tools.py" install-python-env
export IDF_PYTHON_ENV_PATH="$IDF_TOOLS_PATH/python_env/idf5.4_py3.12_env"
. "$IDF_PATH/export.sh"
# Optional disk-saving shallow fetch; still selects the pinned gitlink:
git -C "$IDF_PATH" submodule update --init --depth 1 components/esp_wifi/lib
```

Then, from the repository root:

```sh
C9_LIFECYCLE=ON bash examples/classic-camera/build.sh "$BUILD_A"
# Second clean source/output directory, no generated sdkconfig copied:
nice -n 10 idf.py -C "$SOURCE_B" -B "$BUILD_B" \
  -D SDKCONFIG="$BUILD_B/sdkconfig" -D C9_LIFECYCLE=ON build
python3 examples/classic-camera/check-builds.py "$BUILD_A" "$BUILD_B" \
  esp32/tests/fixtures/classic-camera lifecycle.bin
python3 examples/classic-camera/expected-frame.py
python3 examples/classic-camera/expected-frame.py --replacement
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" nice -n 10 cargo +1.99.0 test \
  --release -p esp32 --test camera_lifecycle -- --ignored --nocapture
```

The host supplies the original fixture image, waits for the guest's replacement
marker and supplies its channelwise complement without stopping the camera.
The driver captures two initial frames, returns/reacquires a possibly queued
frame, then captures two fresh replacement frames. The queued frame is not a
freshness oracle. The guest deliberately changes the sensor to QVGA/YUV422,
asserting both status fields, before actual driver deinit (including a null sensor pointer). Then,
the host checks no pending enabled I2S interrupt and RX stopped. The guest then
reinitializes and captures two more replacement frames. The same machine is
rebooted while the camera is live: all three I2S IRQ registers must be reset,
and the complete sequence must pass again with a restored original host image.

Every qualified frame must be 160 × 120 RGB565, 38,400 bytes. Independent Python
construction, not an emulator output, supplies both expected SHA-256 values.
No new production query, user sketch, timing, JPEG, PSRAM, arbitrary wiring,
web adapter compatibility or hardware accuracy claim is made.

## Semantic negative controls

With the pinned IDF environment still exported and no concurrent build using
this worktree, choose a caller-owned mutation log directory:

```sh
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" python3 examples/classic-camera/mutate-lifecycle.py \
  "$PWD" "$MUTATIONS" "$BUILD_A"
C9_LIFECYCLE=ON bash examples/classic-camera/build.sh "$BUILD_A"
python3 examples/classic-camera/check-builds.py "$BUILD_A" "$BUILD_B" \
  esp32/tests/fixtures/classic-camera lifecycle.bin
```

The runner restores source files and the test's image even on failure. The
explicit rebuild also restores the caller's build-directory output after its
last firmware mutant. Only compiled assertion failures count as kills; a
compiler failure or timeout aborts the runner. Runtime mutants exercise sample
ordering, replacement-cache invalidation and live-I2S retention across reboot.
Firmware mutants omit actual deinit or reinit, then require the normal null-
sensor / non-null-frame assertions to fail. Results are in the lifecycle receipt.
