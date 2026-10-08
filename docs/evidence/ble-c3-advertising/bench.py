#!/usr/bin/env python3
"""Compare hello demos: bench.py MAIN_BIN_DIR PR_BIN_DIR FW_DIR > receipt.json."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import resource
import statistics
import subprocess
import sys

before, after, fw = map(Path, sys.argv[1:])
result = {
    'system': platform.system(), 'os_release': platform.release(), 'architecture': platform.machine(),
    'measurement': 'child user CPU seconds, seven alternating pairs after one warmup per arm',
    'conditions': 'sequential runs; no load exclusion; no speedup claim',
    'demos': {},
}
sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
for demo, binary in [('hello', 'esp32sim'), ('c3-hello', 'esp32sim-c3'), ('c6-hello', 'esp32sim-c6')]:
    manifest = json.loads((fw / (demo + '.json')).read_text())
    args = ['--boot', 'rom', '--flash-mb', str(manifest['flash_mb']), '--max-seconds', '30', '--no-dump', '--board', 'none']
    for flag in ['rom', 'bootloader', 'ptable', 'app']:
        args += ['--' + flag, str(fw / manifest['files'][flag])]
    arms = [('main', before / binary), ('pr', after / binary)]
    record = {'args': [a.replace(str(fw), '<FW_DIR>') for a in args],
              'input_sha256': {p: sha(fw / p) for p in manifest['files'].values()},
              'binary_sha256': {label: sha(path) for label, path in arms}, 'runs': []}
    for pair in range(8):
        for label, path in arms:
            load = os.getloadavg()[0]
            start = resource.getrusage(resource.RUSAGE_CHILDREN)
            run = subprocess.run([str(path), *args], capture_output=True, check=True)
            end = resource.getrusage(resource.RUSAGE_CHILDREN)
            stop = next(line for line in run.stderr.decode().splitlines() if line.startswith('[emu] stop:'))
            core_counts = [int(n) for n in re.findall(r'core\d+ (\d+)', stop)]
            instructions = sum(core_counts) if core_counts else int(re.search(r'(\d+) insns', stop)[1])
            sample = {'pair': pair, 'arm': label, 'warmup': pair == 0,
                      'user_seconds': end.ru_utime - start.ru_utime,
                      'system_seconds': end.ru_stime - start.ru_stime,
                      'load_before': load, 'instructions': instructions,
                      'console_sha256': hashlib.sha256(run.stdout).hexdigest()}
            if core_counts:
                sample['core_instructions'] = core_counts
            record['runs'].append(sample)
    assert len({r['instructions'] for r in record['runs']}) == 1, 'instruction counts differ'
    assert len({r['console_sha256'] for r in record['runs']}) == 1, 'console output differs'
    record['median_user_seconds'] = {
        label: statistics.median(r['user_seconds'] for r in record['runs'] if r['arm'] == label and not r['warmup'])
        for label, _ in arms
    }
    medians = record['median_user_seconds']
    record['change_percent'] = 100 * (medians['pr'] / medians['main'] - 1)
    result['demos'][demo] = record
print(json.dumps(result, indent=2))
