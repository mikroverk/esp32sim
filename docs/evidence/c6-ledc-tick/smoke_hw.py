"""Daytime A/B smoke with host counters: per fixture 1 warmup pair + N alternating pairs (order
alternates per pair); each run under `/usr/bin/time -l`, which reports the child's user CPU (from
wait4) and, on Apple silicon, the child's retired host instructions and elapsed cycles. Load
average before every run; console hash checked per run.
usage: smoke_hw.py <out.json> <guest-seconds> <pairs> <base-bin> <cand-bin>"""
import subprocess, sys, os, json, hashlib, statistics, re
from pathlib import Path
sys.path.insert(0, str(Path(__file__).parent))
from fixtures import fixture_args, FIXTURES
out, secs, pairs, base, cand = Path(sys.argv[1]), sys.argv[2], int(sys.argv[3]), sys.argv[4], sys.argv[5]
def run(b, n):
    load = os.getloadavg()[0]
    r = subprocess.run(['/usr/bin/time', '-l', b] + fixture_args(n) + ['--max-seconds', secs], capture_output=True)
    e = r.stderr.decode(errors='replace')
    user = float(re.search(r'([0-9.]+) user', e).group(1))
    ins = int(re.search(r'(\d+)\s+instructions retired', e).group(1))
    cyc = int(re.search(r'(\d+)\s+cycles elapsed', e).group(1))
    return {'user_s': user, 'host_insns': ins, 'host_cycles': cyc, 'rc': r.returncode, 'load1': load, 'console_sha256': hashlib.sha256(r.stdout).hexdigest()}
res = {'seconds': secs, 'pairs': pairs, 'base': base, 'cand': cand, 'fixtures': {}}
for n in FIXTURES:
    runs = []
    for i in range(pairs + 1):
        order = [('base', base), ('cand', cand)] if i % 2 == 0 else [('cand', cand), ('base', base)]
        pair = {}
        for arm, b in order: pair[arm] = run(b, n)
        pair['warmup'] = i == 0; pair['order'] = [a for a, _ in order]
        runs.append(pair)
        print(n, i, {a: (round(pair[a]['user_s'], 3), pair[a]['host_insns'], pair[a]['host_cycles']) for a in ('base', 'cand')}, 'load1', round(pair[order[0][0]]['load1'], 2), flush=True)
    m = [r for r in runs if not r['warmup']]
    f = {}
    for k in ('user_s', 'host_insns', 'host_cycles'):
        b = [r['base'][k] for r in m]; c = [r['cand'][k] for r in m]
        f[k] = {'base_median': statistics.median(b), 'cand_median': statistics.median(c), 'median_change_pct': (statistics.median(c) / statistics.median(b) - 1) * 100,
                'cand_higher_pairs': sum(ci > bi for bi, ci in zip(b, c)), 'pairs': len(b)}
    hashes = {r[a]['console_sha256'] for r in runs for a in ('base', 'cand')}
    res['fixtures'][n] = {'runs': runs, 'metrics': f, 'console_identical': len(hashes) == 1, 'all_rc0': all(r[a]['rc'] == 0 for r in runs for a in ('base', 'cand')),
                          'load1_range': [min(r[a]['load1'] for r in runs for a in ('base', 'cand')), max(r[a]['load1'] for r in runs for a in ('base', 'cand'))]}
    print(n, json.dumps({k: v for k, v in res['fixtures'][n].items() if k != 'runs'}), flush=True)
    out.write_text(json.dumps(res, indent=1) + '\n')
print('done')
