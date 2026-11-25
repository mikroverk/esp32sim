"""Alternating host-instruction rounds (/usr/bin/time -l): in each round every fixture runs on every
binary in turn. usage: ir_rounds.py <rounds> <seconds> '<fixtures>' <bin-dir>... (first is the
reference; a bin-dir holds esp32sim, esp32sim-c3, -c6, -p4). One JSON line per run with the stop
line and console hash checked against the reference's run in the same round."""
import subprocess, sys, re, json, hashlib, os
from pathlib import Path
sys.path.insert(0, str(Path(__file__).parent))
from fixtures import fixture_args, binary
rounds, secs, fixtures, bins = int(sys.argv[1]), sys.argv[2], sys.argv[3].split(), sys.argv[4:]
for r in range(rounds):
    for n in fixtures:
        ref = None
        for b in bins:
            p = subprocess.run(['/usr/bin/time', '-l', str(binary(b, n))] + fixture_args(n) + ['--max-seconds', secs], capture_output=True)
            e = p.stderr.decode(errors='replace')
            rec = {'round': r, 'fixture': n, 'bin': os.path.basename(b.rstrip('/')), 'rc': p.returncode,
                   'host_insns': int(re.search(r'(\d+)\s+instructions retired', e)[1]),
                   'stop': re.sub(r' in [0-9.]+s wall = [0-9.]+ Minsn/s', '', (re.findall(r'\[emu\] stop: .*', e) or [''])[0]),
                   'console': hashlib.sha256(p.stdout).hexdigest()[:12]}
            if ref is None: ref = rec
            rec['pct'] = round(100 * (rec['host_insns'] / ref['host_insns'] - 1), 3)
            rec['same_work'] = rec['stop'] == ref['stop'] and rec['console'] == ref['console']
            print(json.dumps(rec), flush=True)
