#!/usr/bin/env python3
"""The browser build of two trees on s31-mosaico under Node, alternating pairs, for EX269.

    ESP32SIM_ROM_DIR=<roms> node_pairs.py --out FILE.json [--pairs 5] [--max-load 3] MAIN_TREE NEW_TREE

Each tree has its web/wasm/esp32sim.wasm (`tools/wasm-build.sh`). A run is `node tools/wasm-test.mjs
s31-mosaico`: the wall time and the counts of its line. Load is checked as in pairs.py.
"""
import argparse, hashlib, json, os, re, statistics, subprocess, sys, time

def run(tree):
    load = os.getloadavg()[0]
    p = subprocess.run(['node', 'tools/wasm-test.mjs', 's31-mosaico'], cwd=tree, capture_output=True, text=True)
    m = re.search(r'ok   s31-mosaico: .*?([\d.]+) M insns in ([\d.]+) s wall .*?, (\d+) console lines, (\d+) binary frames', p.stdout)
    if not m: sys.exit(p.stdout + p.stderr)
    return {'wall_s': float(m.group(2)), 'insns_m': float(m.group(1)), 'console_lines': int(m.group(3)), 'frames': int(m.group(4)), 'load': round(max(load, os.getloadavg()[0]), 2)}

ap = argparse.ArgumentParser()
ap.add_argument('--out', required=True); ap.add_argument('--pairs', type=int, default=5); ap.add_argument('--max-load', type=float, default=3.0)
ap.add_argument('main'); ap.add_argument('new')
a = ap.parse_args()
sides = [('main', a.main), ('new', a.new)]
runs, discarded = {'main': [], 'new': []}, []
for _, t in sides: run(t)
while len(runs['main']) < a.pairs:
    while os.getloadavg()[0] >= a.max_load - 0.5: time.sleep(5)
    one = {l: run(t) for l, t in (sides if len(runs['main']) % 2 == 0 else sides[::-1])}
    if max(r['load'] for r in one.values()) >= a.max_load: discarded.append(one)
    else:
        for l, r in one.items(): runs[l].append(r)
work = {(r['insns_m'], r['console_lines'], r['frames']) for rs in runs.values() for r in rs}
result = {'wasm_sha256': {l: hashlib.sha256(open(os.path.join(t, 'web/wasm/esp32sim.wasm'), 'rb').read()).hexdigest() for l, t in sides}, 'node': subprocess.run(['node', '-v'], capture_output=True, text=True).stdout.strip(),
          'identical_counts': len(work) == 1, 'paired_median_pct': round(statistics.median((n['wall_s'] / m['wall_s'] - 1) * 100 for m, n in zip(runs['main'], runs['new'])), 2),
          'new_slower_pairs': sum(n['wall_s'] > m['wall_s'] for m, n in zip(runs['main'], runs['new'])), 'discarded_pairs': discarded, 'runs': runs}
with open(a.out, 'w') as f: f.write(json.dumps(result, indent=1) + '\n')
print(result['paired_median_pct'], result['new_slower_pairs'], [r['wall_s'] for r in runs['main']], [r['wall_s'] for r in runs['new']])
