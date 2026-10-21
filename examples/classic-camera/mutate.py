#!/usr/bin/env python3
"""Run compiled semantic mutations against the public-driver frame/hash assertion."""
import json
import os
from pathlib import Path
import subprocess
import sys

root, out = (Path(p).resolve() for p in sys.argv[1:3])
out.mkdir(parents=True, exist_ok=True)
rows = [
    ('camera dispatch disabled', 'esp32/src/i2s.rs', 'self.camera_mode = self.camera_capable && value & 0x21 == 0x21', 'self.camera_mode = false'),
    ('transport drops bytes', 'esp32/src/camera_dma.rs', 'let data = capture.packed[capture.pos..end].to_vec();', 'let data = Vec::new();'),
    ('wrong FIFO mode', 'esp32/src/camera_dma.rs', 'CameraFifo::new(rx.fifo_mode)', 'CameraFifo::new(3)'),
    ('wrong sensor sample ordering', 'esp32/src/camera_fifo.rs', '[sample, 0, first, 0]', '[first, 0, sample, 0]'),
    ('wrong classic EOF count unit', 'esp32/src/i2s.rs', 'ram.read(0x24).checked_mul(4)', 'ram.read(0x24).checked_mul(1)'),
]
original = {f: (root / f).read_text() for _, f, _, _ in rows}
cmd = ['nice', '-n', '10', 'cargo', '+1.99.0', 'test', '--release', '-p', 'esp32', '--test', 'camera_fixture', '--', '--ignored', '--nocapture']
results = []
try:
    for n, (name, file, before, after) in enumerate([('baseline', rows[0][1], None, None)] + rows):
        if before:
            assert after is not None
            assert original[file].count(before) == 1, name
            (root / file).write_text(original[file].replace(before, after, 1))
        run = subprocess.run(cmd, cwd=root, env=os.environ, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=240)
        killed = (run.returncode != 0 and 'public_driver_captures_host_frame ... FAILED' in run.stdout
                  and 'assertion' in run.stdout and 'error[E' not in run.stdout)
        (out / f'{n:02}.log').write_text(run.stdout)
        results.append(dict(name=name, file=file, before=before, after=after, exit=run.returncode, assertion_killed=killed))
        (out / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
        print(name, run.returncode, killed, flush=True)
        (root / file).write_text(original[file])
        assert killed if before else run.returncode == 0, run.stdout[-3000:]
finally:
    for file, text in original.items():
        (root / file).write_text(text)
