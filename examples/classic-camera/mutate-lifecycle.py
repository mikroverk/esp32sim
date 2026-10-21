#!/usr/bin/env python3
"""Compiled negative controls for lifecycle assertions; no timeout/build-error kills.
Run from a disposable worktree, after exporting pinned IDF for firmware mutants.
Arguments: repository, log-directory, lifecycle-build-directory.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

root, out, build = (Path(p).resolve() for p in sys.argv[1:4])
out.mkdir(parents=True, exist_ok=True)
source = 'examples/classic-camera/main/lifecycle.c'
rows = [
    ('initial frame ordering', 'esp32/src/camera_fifo.rs', '[sample, 0, first, 0]', '[first, 0, sample, 0]', False),
    ('replacement cache stays stale', 'esp-soc/src/camera.rs', 'self.picture = Some(p); self.invalidate(); Ok(())', 'self.picture = Some(p); Ok(())', False),
    ('deinit omitted', source, 'assert(esp_camera_deinit() == ESP_OK);', '/* semantic mutant: leave the initialized driver running */', True),
    ('reinit omitted', source, 'vTaskDelay(pdMS_TO_TICKS(100));\n    assert(esp_camera_init(&config) == ESP_OK);', 'vTaskDelay(pdMS_TO_TICKS(100));\n    /* semantic mutant: do not reinitialize the driver */', True),
    ('reinit sensor format stays YUV', source, 'printf("C9 reinit=ESP_OK\\n");', 'assert(esp_camera_sensor_get()->set_pixformat(esp_camera_sensor_get(), PIXFORMAT_YUV422) == 0);\n    printf("C9 reinit=ESP_OK\\n");', True),
    ('reboot keeps live I2S', 'esp32/src/soc.rs', 'for port in 0..2 { self.periph.i2s[port].inner.keep_rx_input(&mut old.i2s[port].inner); }', 'std::mem::swap(&mut self.periph.i2s, &mut old.i2s);', False),
]
original = {file: (root/file).read_text() for _,file,_,_,_ in rows}
image = root/'esp32/tests/fixtures/classic-camera/lifecycle.bin'
original_image = image.read_bytes()
cmd = ['nice','-n','10','cargo','+1.99.0','test','--release','-p','esp32','--test','camera_lifecycle','--','--ignored','--nocapture']
results = []
try:
    for n, (name, file, before, after, firmware) in enumerate([('baseline',source,None,None,False)] + rows):
        if before:
            assert after is not None
            assert original[file].count(before) == 1, name
            (root/file).write_text(original[file].replace(before, after, 1))
        if firmware:
            with (out/f'{n:02}-build.log').open('w') as log:
                subprocess.run(['nice','-n','10','idf.py','-C',str(root/'examples/classic-camera'),'-B',str(build),'-D',f'SDKCONFIG={build}/sdkconfig','-D','C9_LIFECYCLE=ON','build'], cwd=root, stdout=log, stderr=subprocess.STDOUT, check=True)
            shutil.copyfile(build/'classic_camera.bin', image)
        run = subprocess.run(cmd, cwd=root, env=os.environ, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=300)
        killed = run.returncode != 0 and 'public_driver_camera_lifecycle ... FAILED' in run.stdout and 'panicked at esp32/tests/camera_lifecycle.rs:' in run.stdout and 'error[E' not in run.stdout
        (out/f'{n:02}.log').write_text(run.stdout)
        results.append(dict(name=name,file=file,before=before,after=after,firmware=firmware,exit=run.returncode,assertion_killed=killed))
        (out/'results.json').write_text(json.dumps(results,indent=2)+'\n')
        print(name,run.returncode,killed,flush=True)
        (root/file).write_text(original[file])
        image.write_bytes(original_image)
        assert killed if before else run.returncode == 0, run.stdout[-3000:]
finally:
    for file,text in original.items(): (root/file).write_text(text)
    image.write_bytes(original_image)
