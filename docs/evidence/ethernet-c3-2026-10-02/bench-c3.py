#!/usr/bin/env python3
"""Run from the repository root: bench-c3.py BEFORE AFTER ROM > samples.json."""
import hashlib
import json
import os
import platform
from pathlib import Path
import re
import resource
import subprocess
import sys

before, after, rom = sys.argv[1:]
fw = Path('web/wasm/fw/public')
args = ['--rom', rom, '--boot', 'rom', '--bootloader', str(fw / 'c3-hello-bootloader.bin'),
        '--ptable', str(fw / 'c3-hello-ptable.bin'), '--app', str(fw / 'c3-hello_world.bin'),
        '--max-seconds', '30', '--no-dump']
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
result = {'system': platform.system(), 'release': platform.release(), 'architecture': platform.machine(),
          'cpu_count': os.cpu_count(), 'measurement': 'child user + system CPU seconds; sequential alternating pairs',
          'hashes': {name: sha(path) for name, path in [('before', before), ('after', after), ('rom', rom)]}, 'runs': []}
result['hashes'].update({p.name: sha(p) for p in [fw / 'c3-hello-bootloader.bin', fw / 'c3-hello-ptable.bin', fw / 'c3-hello_world.bin']})
for pair in range(1, 4):
    for label, binary in [('before', before), ('after', after)]:
        load = os.getloadavg()
        start = resource.getrusage(resource.RUSAGE_CHILDREN)
        run = subprocess.run([binary, *args], capture_output=True, check=True)
        end = resource.getrusage(resource.RUSAGE_CHILDREN)
        stop = next(line for line in run.stderr.decode().splitlines() if line.startswith('[emu] stop:'))
        insns = int(re.search(r'(\d+) insns', stop)[1])
        assert insns == 4_800_000_000, stop
        result['runs'].append({'pair': pair, 'variant': label, 'load_before': load,
                              'cpu_seconds': end.ru_utime + end.ru_stime - start.ru_utime - start.ru_stime,
                              'instructions': insns, 'console_sha256': hashlib.sha256(run.stdout).hexdigest(), 'stop': stop})
assert len({r['console_sha256'] for r in result['runs']}) == 1, 'console changed'
print(json.dumps(result, indent=2))
