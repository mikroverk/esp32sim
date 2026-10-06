# EX212: Start capture from sensor VSYNC

Base `cbb9edf607a09be78cb199c49c2e4cc0bd27d0dc`. On the public `waveshare-cam`
board, unchanged esp32-camera v2.1.4 initializes the OV5640 but main returns seven
empty captures. This fix clocks VSYNC before CAM_START so the driver can arm
GDMA. The board's existing output is YUYV, regardless of requested pixel format;
the three explicitly configured YUYV captures are the format-correct checks.
No RGB565 conversion or JPEG encoding is claimed by this commit.

## Reproduce

Use arduino-cli 1.5.1, Arduino-ESP32 3.3.8, ESP-IDF 5.5.4 and esp32-camera v2.1.4.
Only the sketch's board configuration changes: Waveshare DVP pins from
[the board reference](../../boards.md#waveshare-cam--waveshare-esp32-s3-cam-ov5640)
and the shared I2C0 bus through the driver's existing sccb_i2c_port API.
The driver sources are unchanged. Build for S3, 8 MiB flash, no PSRAM and one
DRAM framebuffer. The helper checks selection of the v2.1.4 objects in the map.

Install esp32:esp32@3.3.8 in a caller-supplied Arduino configuration. Download
[esp32-camera v2.1.4](https://codeload.github.com/espressif/esp32-camera/tar.gz/refs/tags/v2.1.4).
For macOS arm64, build [ctags 5.8-arduino11](https://codeload.github.com/arduino/ctags/tar.gz/refs/tags/5.8-arduino11):
run ./configure, replace __unused__ with CTAGS_UNUSED in C/H files using byte
replacement, then make -j4. Configure may return 2 after creating Makefile and
config.h. This is a host utility workaround, not a driver edit. Input archives,
firmware segments, ROM and this commit's source hashes are in inputs.json.
Keep scratch paths within the worktree and without whitespace.

```sh
python3 docs/evidence/camera-start-2026-10-02/prepare-firmware.py \
  "$SCRATCH" "$ARDUINO_CONFIG" "$CAMERA_V214_SOURCE" "$CTAGS_DIR" "$ARDUINO_338_CORE"
cargo +1.99.0 build --release --bins
python3 docs/evidence/camera-start-2026-10-02/external_camera.py \
  --emulator target/release/esp32sim --firmware "$SCRATCH/firmware-output" \
  --rom web/wasm/fw/esp32s3_rev0_rom.elf --output "$SCRATCH/candidate" --expect vsync-only
```

Build main in a separate checkout with the same Cargo command; repeat the helper
with its executable and `--expect empty`. Obtain the S3 mask ROM through
`tools/fetch-demo-assets.sh --no-linux`.

The helper uses `--board waveshare-cam --cam-image` with a generated red PPM.
Its action script captures three frames, deinitializes/reinitializes in YUV422,
captures three more, reboots, then captures once. All are 96×96 and 18432 bytes.
YUYV bytes `4c 54 4c ff` have FNV-1a `8c2afdc5`. The helper computes this
expectation independently. results.json records exact serial markers and hashes.
Absolute compiler paths can change firmware hashes without changing these bytes.

## Model limits

VS_EOF closes a short in-progress receive descriptor at VSYNC, then subsequent
frames align. Byte-count mode supports several EOFs per frame on a descriptor
ring. Both paths use the existing scatter DMA walker and write-back helper.
The standalone commit passes 127 S3 library tests. Tests cover partial slices,
owner checks, ring exhaustion, reset, gated clocks,
blanking and stopped capture. Camera activity fields are private and updated by
register/frame setters. New camera state is at the end of LCD_CAM; EOF progress
belongs to GdmaInCh.

DVP timing remains approximate: 10 fps by default, 5% blanking, half-period active
pixels. No physical-camera, PCLK calibration, PSRAM framebuffer or CameraWebServer
claim. Register definitions cite ESP-IDF v5.5.4, compared with v4.4.8.
