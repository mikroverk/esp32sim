#!/usr/bin/env python3
"""Run compile-valid removal mutations; restores source even on failure.
Run in a disposable worktree: python3 docs/evidence/classic-rtc-mem-crc/mutate.py OUT
CARGO_TARGET_DIR must be outside the control checkout. No other build may run here.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

repo = Path(__file__).resolve().parents[3]
out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
assert os.environ.get('CARGO_TARGET_DIR'), 'set an isolated CARGO_TARGET_DIR'
bus = repo / 'esp32/src/bus.rs'
rtc = repo / 'esp32/src/periph.rs'
originals = {p:p.read_text() for p in [bus, rtc]}
mutations = [
    ('no-start', bus, 'self.periph.rtc.memory_crc(&self.rtc_fast);', 'let _ = &self.rtc_fast;'),
    ('no-finish', rtc, 'conf | (1 << 31)', 'conf'),
    ('ignore-start-bit', bus, 'a == 0x3ff4_8100 && v & (1 << 8) != 0', 'a == 0x3ff4_8100'),
    ('finish-writable', rtc, 'self.0.ram.write(off, v & !(1 << 31));', 'self.0.ram.write(off, v);'),
    ('slow-memory', bus, 'memory_crc(&self.rtc_fast)', 'memory_crc(&self.rtc_slow)'),
    ('ignore-address', rtc, 'let start = ((conf >> 9) & 0x7ff) as usize * 4;', 'let start = 0;'),
    ('length-not-plus-one', rtc, 'as usize + 1) * 4;', 'as usize) * 4;'),
    ('wrong-result', rtc, 'self.0.ram.write(0x104, crc);', 'self.0.ram.write(0x104, crc ^ 1);'),
    ('result-writable', rtc, '// CRC result is read-only.', 'self.0.ram.write(off, v);'),
    ('no-address-wrap', rtc, 'memory[(start + offset) & 0x1fff]', 'memory[(start + offset).min(0x1fff)]'),
]
rows = []
try:
    for name, path, old, new in mutations:
        assert originals[path].count(old) == 1, (name, old)
        path.write_text(originals[path].replace(old, new))
        run = subprocess.run(['nice','-n','10','cargo','+1.99.0','test','--release','-p','esp32','--test','rtc_crc'], cwd=repo, capture_output=True, text=True)
        log = run.stdout + run.stderr
        (out/(name+'.log')).write_text(log)
        killed = run.returncode != 0 and 'test result: FAILED' in log
        rows.append(dict(mutation=name, exit_code=run.returncode, compiled='Finished `release`' in log, killed=killed, failed_tests=[l for l in log.splitlines() if l.startswith('test ') and l.endswith('FAILED')]))
        path.write_text(originals[path])
        print(name, rows[-1], flush=True)
        assert killed and rows[-1]['compiled'], name
finally:
    for path, source in originals.items():
        path.write_text(source)
    (out/'mutations.json').write_text(json.dumps(rows,indent=2)+'\n')
