#!/usr/bin/env python3
"""Alternate two release builds with camera unused; retain numeric results, not raw logs."""
import argparse
import hashlib
import json
import platform
import re
import resource
import statistics
import subprocess
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('base_bin', type=Path)
p.add_argument('candidate_bin', type=Path)
p.add_argument('--pairs', type=int, default=7)
p.add_argument('--warmup-pairs', type=int, default=1)
p.add_argument('--seconds', type=int, default=30)
p.add_argument('--hello-seconds', type=int, default=3000)
p.add_argument('--base-revision', required=True)
p.add_argument('--candidate-revision', required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--workloads', nargs='+', choices=['hello', 'c3-hello', 'c6-hello', 'pocket-tank'], default=['hello', 'c3-hello', 'c6-hello', 'pocket-tank'])
a = p.parse_args()
root = Path(__file__).resolve().parents[3]
fw = root / 'web/wasm/fw'
results = {'system': platform.system(), 'machine': platform.machine(), 'pairs': a.pairs,
           'seconds': a.seconds, 'hello_seconds': a.hello_seconds, 'base_revision': a.base_revision, 'candidate_revision': a.candidate_revision, 'commands': {}, 'warmup_pairs': a.warmup_pairs, 'timer': 'getrusage children user seconds', 'runs': [], 'inputs': {}, 'binaries': {}}
sha = lambda data: hashlib.sha256(data).hexdigest()
for label, directory in [('base', a.base_bin), ('candidate', a.candidate_bin)]:
    results['binaries'][label] = {name: sha((directory / name).read_bytes()) for name in ['esp32sim', 'esp32sim-c3', 'esp32sim-c6']}
for name in a.workloads:
    manifest = json.loads((fw / (name + '.json')).read_text())
    binary = 'esp32sim-c3' if name.startswith('c3') else 'esp32sim-c6' if name.startswith('c6') else 'esp32sim'
    args = ['--boot', 'rom', '--max-seconds', str(a.hello_seconds if name == 'hello' else a.seconds), '--no-dump', '--board', manifest.get('board', 'none') if binary == 'esp32sim' else 'none', '--flash-mb', str(manifest.get('flash_mb', 4))]
    if binary == 'esp32sim' and 'psram_mb' in manifest:
        args += ['--psram-mb', str(manifest['psram_mb'])]
    for kind, path in manifest['files'].items():
        args += ['--' + kind, str(fw / path)]
        results['inputs'][path] = sha((fw / path).read_bytes())
    for address, path in manifest.get('flash_at', {}).items():
        args += ['--flash-at', address + '=' + str(fw / path)]
        results['inputs'][path] = sha((fw / path).read_bytes())
    results['commands'][name] = [binary, *[arg.replace(str(root) + '/', '') for arg in args]]
    for pair in range(-a.warmup_pairs, a.pairs):
        order = [('base', a.base_bin), ('candidate', a.candidate_bin)]
        if pair % 2: order.reverse()
        for label, directory in order:
            before = resource.getrusage(resource.RUSAGE_CHILDREN).ru_utime
            run = subprocess.run(['/usr/bin/time', '-p', str(directory / binary), *args], cwd=root, capture_output=True, check=True)
            err = run.stderr.decode()
            work = re.search(r'stop: .*? — (.*?) insns in ', err)[1]
            cores = [int(n) for n in re.findall(r'core\d+ (\d+)', work)]
            insns = sum(cores) if cores else int(work)
            user = resource.getrusage(resource.RUSAGE_CHILDREN).ru_utime - before
            result = dict(workload=name, pair=pair, build=label, user=user, insns=insns, core_insns=cores, console_sha256=sha(run.stdout))
            results['runs'].append(result)
            a.output.write_text(json.dumps(results, indent=2) + '\n')
            print(name, pair, label, user, insns, flush=True)
    runs = [r for r in results['runs'] if r['workload'] == name]
    assert len({r['insns'] for r in runs}) == 1, name + ' instruction mismatch'
    assert len({r['console_sha256'] for r in runs}) == 1, name + ' console mismatch'
results['medians'] = {}
for name in a.workloads:
    med = {label: statistics.median(r['user'] for r in results['runs'] if r['workload'] == name and r['build'] == label and r['pair'] >= 0) for label in ['base', 'candidate']}
    med['change_percent'] = (med['candidate'] / med['base'] - 1) * 100
    results['medians'][name] = med
results['correctness'] = 'identical instruction counts and stdout SHA-256 in every pair'
a.output.write_text(json.dumps(results, indent=2) + '\n')
