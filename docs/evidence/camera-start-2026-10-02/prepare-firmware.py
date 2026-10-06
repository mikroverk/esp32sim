#!/usr/bin/env python3
"""Build the preserved sketch using caller-supplied Arduino 3.3.8 and camera 2.1.4."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("scratch", type=Path)
parser.add_argument("arduino_config", type=Path)
parser.add_argument("camera_source", type=Path)
parser.add_argument("ctags_dir", type=Path)
parser.add_argument("arduino_core", type=Path)
args = parser.parse_args()
scratch, config, camera, ctags, core = (
    p.resolve() for p in vars(args).values()
)
assert not any(c.isspace() for c in str(scratch)), "Use a scratch path without whitespace"
scratch.mkdir(parents=True, exist_ok=True)
library = scratch / "Camera"
src = library / "src"
for folder in ["driver", "conversions", "sensors", "target"]:
    shutil.copytree(camera / folder, src / folder, dirs_exist_ok=True)
# Match v2.1.4 CMakeLists.txt's ESP32-S3 / IDF >= 5.4 source selection.
for name in ["driver/sccb.c", "target/xclk.c"]:
    (src / name).unlink()
for name in ["esp32", "esp32s2"]:
    shutil.rmtree(src / "target" / name)
(library / "library.properties").write_text(
    "name=Camera\nversion=2.1.4\nauthor=Espressif\nmaintainer=Espressif\n"
    "sentence=Camera fixture\nparagraph=Unmodified v2.1.4 driver\n"
    "category=Other\narchitectures=esp32\n"
)
(src / "esp_camera.h").write_text('#include "driver/include/esp_camera.h"\n')
(src / "CameraFixtureDriver.h").write_text('#include "esp_camera.h"\n')
sketch = scratch / "CameraFixture"
sketch.mkdir(exist_ok=True)
(sketch / "CameraFixture.ino").write_text("#include <CameraFixtureDriver.h>\n")
shutil.copyfile(Path(__file__).with_name("firmware.cpp"), sketch / "firmware.cpp")
includes = " ".join(
    f"-I{src / path}" for path in [
        "driver/include", "conversions/include", "conversions/private_include",
        "driver/private_include", "sensors/private_include", "target/private_include",
    ]
)
output = scratch / "firmware-output"
subprocess.run([
    "arduino-cli", "--config-file", str(config), "compile",
    "--fqbn", "esp32:esp32:esp32s3:FlashSize=8M,PartitionScheme=default_8MB",
    "--library", str(library), "--build-path", str(scratch / "firmware-build"),
    "--output-dir", str(output),
    "--build-property", f"runtime.tools.ctags.path={ctags}",
    "--build-property", f"compiler.c.extra_flags={includes}",
    "--build-property", f"compiler.cpp.extra_flags={includes}", str(sketch),
], env={**os.environ, "TMPDIR": str(scratch)}, check=True)
link_map = (output / "CameraFixture.ino.map").read_text()
for name in ["driver/esp_camera.c.o", "driver/cam_hal.c.o",
             "driver/sccb-ng.c.o", "target/esp32s3/ll_cam.c.o"]:
    assert f"libraries/Camera/{name}" in link_map, name
files = []
for path, offset in [
    (output / "CameraFixture.ino.bootloader.bin", 0),
    (output / "CameraFixture.ino.partitions.bin", 0x8000),
    (core / "tools/partitions/boot_app0.bin", 0xe000),
    (output / "CameraFixture.ino.bin", 0x10000),
]:
    data = path.read_bytes()
    files.append(dict(filename=path.name, offset=offset, bytes=len(data),
                      sha256=hashlib.sha256(data).hexdigest()))
(output / "flash-artifacts.json").write_text(json.dumps(dict(files=files), indent=2))
(scratch / "frame.ppm").write_bytes(b"P6\n1 1\n255\n" + bytes([255, 0, 0]))
