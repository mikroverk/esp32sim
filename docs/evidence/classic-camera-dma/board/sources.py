#!/usr/bin/env python3
"""Pin public board wiring and SCCB/grayscale source contracts."""
import hashlib, json, sys, urllib.request
from pathlib import Path
out=Path(sys.argv[1]); out.mkdir(parents=True,exist_ok=True)
sources=[
 ('camera_pins.h','https://raw.githubusercontent.com/espressif/arduino-esp32/5e19e086c43d0fa5e5a596497ff8f11a0a43f6c2/libraries/ESP32/examples/Camera/CameraWebServer/camera_pins.h'),
 ('ov2640_settings.h','https://raw.githubusercontent.com/espressif/esp32-camera/b817e9b81041980608d1a3d3e0b08d86e3666382/sensors/private_include/ov2640_settings.h'),
 ('ll_cam.c','https://raw.githubusercontent.com/espressif/esp32-camera/b817e9b81041980608d1a3d3e0b08d86e3666382/target/esp32/ll_cam.c'),
]
rows=[]
for name,url in sources:
 data=urllib.request.urlopen(url,timeout=30).read(); (out/name).write_bytes(data)
 rows.append(dict(filename=name,url=url,bytes=len(data),sha256=hashlib.sha256(data).hexdigest()))
(out/'sources.json').write_text(json.dumps(rows,indent=2)+'\n')
print(json.dumps(rows,indent=2))
