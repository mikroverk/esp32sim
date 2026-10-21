# OV2640 SCCB transaction fixtures

Public source: espressif/esp32-camera v2.1.4, commit
`b817e9b81041980608d1a3d3e0b08d86e3666382`. `sources.json` records immutable URLs,
byte lengths and SHA-256 values. See NOTICE and the accompanying licenses.

From the repository root, with Python 3 and a host C compiler:

```
python3 tools/ov2640-driver-trace.py --scratch <task-owned-scratch>
cargo +1.99.0 test --release -p esp-soc --test ov2640
```

The generator downloads the pinned public sources and compiles the unchanged
`ov2640.c` source span from `reg_bank` through `set_framesize`, plus
`esp32_camera_ov2640_detect`. Host SCCB stubs record writes and expected ID reads;
delays and logging are stubbed out. The minimal host sensor structure and selected
4:3 resolution entries replace IDF dependencies, preserving the driver's mode
selection thresholds. QQVGA, VGA and UXGA are exercised; CIF and SVGA enum entries
serve as thresholds. The selected resolutions match `driver/sensor.c:resolution`,
and `ratio_table` comes directly from the public settings header. The entire
reset/ISP initialization write list is retained, not just the modeled registers.

`W rr vv` is one address+register+value transaction followed by STOP. `R rr vv`
sets the pointer, sends STOP, then starts a distinct read transaction and expects
`vv`, matching `driver/sccb.c:SCCB_Read`. Each trace includes detect and reset;
size traces then call `set_framesize`, which also reapplies `set_pixformat`.
The Rust tests replay all six traces both at the sensor interface and through
the existing classic I2C controller. They separately test live format transitions,
ordinary bank-isolated storage, software reset, high dimension bits, standby,
DVP reset and the conservative DSP-bypass output policy.

The trace generator is a source-level driver qualification harness, NOT actual
ESP-IDF guest execution, hardware capture or an `esp_camera_init` test. No new
firmware binary or non-camera golden is introduced. ID and register behavior,
reset and output-state decisions are inferred from the public driver/header;
there is no hardware comparison. VER/manufacturer values describe the modeled
revision rather than a capability discovered by the driver's source.

The model only accepts the single-register transactions used by this driver.
It does not promise SCCB burst auto-increment semantics. Unknown ISP registers
retain their last write; indirect table behavior, exposure/white balance, optics,
JPEG encoding and full silicon power-on defaults are outside scope. Fresh and
software-reset geometry is deliberately zero/unconfigured; `streaming` means a
configured DSP output, not an externally clocked or powered physical sensor.
Grayscale requests use the public YUV422 sequence (two sensor bytes per pixel).
JPEG mode is configuration state only, not encoded data delivery.
