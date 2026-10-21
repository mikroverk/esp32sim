#!/usr/bin/env python3
"""Configuration-loss semantic mutations; compile errors never count."""
import json, os, re, subprocess, sys
from pathlib import Path
root, out = (Path(p).resolve() for p in sys.argv[1:3])
out.mkdir(parents=True, exist_ok=True)
bus = 'esp32/src/bus.rs'
cam = 'esp32/src/camera_dma.rs'
guard = 'self.periph.i2s[0].camera.as_ref().is_some_and(|c| c.qualified())'
rows = [
    ('ignore transient loss', bus, guard, 'false'),
    ('only route-mask writes cancel', bus, guard, 'camera_route_mask != 0 && ' + guard),
    ('only I2S writes cancel', bus, guard, '(0x3ff4_f000..0x3ff4_f100).contains(&a) && ' + guard),
    ('clear frame but leave DMA running', bus, 'self.periph.i2s[0].abort_camera_dma();', 'self.periph.i2s[0].camera = None;'),
    ('qualified waiting capture ignored', cam, 'pub(crate) fn qualified(&self) -> bool { self.qualified }', 'pub(crate) fn qualified(&self) -> bool { self.qualified && !self.packed.is_empty() }'),
    ('valid configuration cancels capture', bus, '&& self.camera_routed_frame().is_none()', '&& self.camera_routed_frame().is_some()'),
    ('initially unavailable capture cancelled', bus, guard, 'self.periph.i2s[0].camera.is_some()'),
]
original = {f: (root / f).read_text() for _, f, _, _ in rows}
cmd = ['nice', '-n', '10', 'cargo', '+1.99.0', 'test', '--release', '-p', 'esp32', '--test', 'aithinker', '--', '--nocapture']
results = []
try:
    for name, file, old, new in [('baseline', bus, None, None)] + rows:
        if old:
            assert new is not None
            assert original[file].count(old) == 1, name
            (root / file).write_text(original[file].replace(old, new))
        r = subprocess.run(cmd, cwd=root, env=os.environ, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        failed = re.findall(r'^test (\S+) \.\.\. FAILED', r.stdout, re.M)
        killed = r.returncode != 0 and bool(failed) and 'assertion' in r.stdout and 'error[E' not in r.stdout
        results.append(dict(name=name, file=file, before=old, after=new, exit=r.returncode, assertion_killed=killed, failed_tests=failed))
        (out / f'{len(results)-1:02}.log').write_text(r.stdout)
        (out / 'mutations.json').write_text(json.dumps(results, indent=2) + '\n')
        print(name, r.returncode, failed, flush=True)
        (root / file).write_text(original[file])
        assert killed if old else r.returncode == 0, r.stdout
finally:
    for file, text in original.items():
        (root / file).write_text(text)
