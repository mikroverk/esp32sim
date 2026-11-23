#!/usr/bin/env python3
"""CPU time of several emulator builds in alternating rounds, for EX269.

    ESP32SIM_ROM_DIR=<roms> pairs.py --out FILE.json [--rounds 7] [--max-load 3]
        --workloads s31-mosaico,s31-hello,c3-hello,c6-hello,s3-hello SIDE=DIR[:NAME=VALUE] ...

The first SIDE is the one the others are compared with; a SIDE is a directory of release binaries
and, optionally, one environment variable (`off=build:RV32_JIT=0`). Run from the repository root.
Per workload: one warm-up run of every side, then ROUNDS rounds, the order reversed every other
round. The 1-minute load average must be 0.5 below --max-load before a round (it waits) and below
--max-load before and after every run of it: a round that misses that is not used, and listed in
`discarded_rounds` with its highest load and its user times. User CPU is the child's rusage; host
instructions and cycles come from macOS's `/usr/bin/time -l`; the guest work check is the
emulator's stop line and the SHA-256 of stdout.
"""
import argparse, hashlib, json, os, re, statistics, subprocess, sys, tempfile, time

ROM = os.environ['ESP32SIM_ROM_DIR']
FW, S31 = 'web/wasm/fw/public', 'esp32s31/firmware'
# the golden's script (cli/tests/goldens.rs, mosaico_demo_s31)
SCRIPT = '4.5 touch 300 200 1\n4.8 touch 300 200 0\n5.0 press ai 150\n5.5 press boot 150\n6.5 press boot 150\n'

def image(prefix, app): return ['--bootloader', f'{prefix}-bootloader.bin', '--ptable', f'{prefix}-ptable.bin', '--app', app, '--no-dump']

def workloads(script):
    rom = lambda n: ['--boot', 'rom', '--rom', f'{ROM}/{n}']
    return {
        's31-mosaico': ('esp32sim-s31', rom('esp32s31_rev0_rom.elf') + ['--board', 'mosaico', '--script', script, '--imu-motion', '1', '--max-seconds', '7.5'] + image(f'{FW}/s31-mosaico-demo', f'{FW}/s31-mosaico-demo.bin')),
        's31-hello': ('esp32sim-s31', rom('esp32s31_rev0_rom.elf') + ['--flash-mb', '16', '--console', 'uart0', '--max-seconds', '30'] + image(f'{S31}/s31-hello', f'{S31}/s31-hello_world.bin')),
        'c3-hello': ('esp32sim-c3', rom('esp32c3_rev3_rom.elf') + ['--flash-mb', '4', '--max-seconds', '30'] + image(f'{FW}/c3-hello', f'{FW}/c3-hello_world.bin')),
        'c6-hello': ('esp32sim-c6', rom('esp32c6_rev0_rom.elf') + ['--flash-mb', '4', '--max-seconds', '30'] + image(f'{FW}/c6-hello', f'{FW}/c6-hello_world.bin')),
        's3-hello': ('esp32sim', rom('esp32s3_rev0_rom.elf') + ['--board', 'none', '--max-seconds', '30'] + image(f'{FW}/hello', f'{FW}/hello_world.bin')),
    }

def run(directory, env, exe, args):
    e = {k: v for k, v in os.environ.items() if not k.startswith('RV32_JIT')}
    e.update(env)
    load, t0 = os.getloadavg()[0], os.times()
    p = subprocess.run(['/usr/bin/time', '-l', os.path.join(directory, exe)] + args, capture_output=True, env=e)
    user = os.times().children_user - t0.children_user
    err = p.stderr.decode(errors='replace')
    stop = re.search(r'stop: .*?(?:core0 (\d+) \+ core1 (\d+)|(\d+)) insns', err)
    if not stop or p.returncode: sys.exit(f'{exe}: exit {p.returncode}\n{err[-1500:]}')
    n = lambda what: int(re.search(rf'(\d+)\s+{what}', err).group(1))
    return {'user_s': round(user, 4), 'host_insns': n('instructions retired'), 'host_cycles': n('cycles elapsed'),
            'insns': [int(g) for g in stop.groups() if g], 'console_sha256': hashlib.sha256(p.stdout).hexdigest(), 'load': round(max(load, os.getloadavg()[0]), 2)}

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', required=True); ap.add_argument('--rounds', type=int, default=7); ap.add_argument('--max-load', type=float, default=3.0)
    ap.add_argument('--workloads', required=True); ap.add_argument('sides', nargs='+')
    a = ap.parse_args()
    sides = []
    for s in a.sides:
        label, rest = s.split('=', 1)
        directory, _, env = rest.partition(':')
        sides.append((label, directory, dict([env.split('=', 1)]) if env else {}))
    script = tempfile.NamedTemporaryFile('w', suffix='.txt', delete=False); script.write(SCRIPT); script.close()
    table, result = workloads(script.name), {'rounds': a.rounds, 'max_load': a.max_load, 'workloads': {}}
    result['binaries'] = {l: {x: hashlib.sha256(open(os.path.join(d, x), 'rb').read()).hexdigest() for x in sorted({table[w][0] for w in a.workloads.split(',')})} for l, d, _ in sides}
    result['environment'] = {l: e for l, _, e in sides}
    def quiet():
        while os.getloadavg()[0] >= a.max_load - 0.5: time.sleep(5)
    for w in a.workloads.split(','):
        exe, args = table[w]
        quiet()
        for l, d, e in sides: run(d, e, exe, args)
        runs, discarded = {l: [] for l, _, _ in sides}, []
        while len(runs[sides[0][0]]) < a.rounds:
            quiet()
            one = {}
            for l, d, e in (sides if len(runs[sides[0][0]]) % 2 == 0 else sides[::-1]):
                one[l] = run(d, e, exe, args)
                if one[l]['load'] >= a.max_load: break
            top = max(r['load'] for r in one.values())
            if top >= a.max_load: discarded.append({'load': top, 'user_s': {l: r['user_s'] for l, r in one.items()}})
            else:
                for l, r in one.items(): runs[l].append(r)
            print(f"  {w}: {len(runs[sides[0][0]])}/{a.rounds} rounds, {len(discarded)} discarded, load {top}", file=sys.stderr, flush=True)
        first = sides[0][0]
        work = {(tuple(r['insns']), r['console_sha256']) for rs in runs.values() for r in rs}
        rec = {'identical_work_and_console': len(work) == 1, 'insns': runs[first][0]['insns'], 'console_sha256': runs[first][0]['console_sha256'],
               'load': [min(r['load'] for rs in runs.values() for r in rs), max(r['load'] for rs in runs.values() for r in rs)], 'discarded_rounds': discarded, 'summary': {}, 'runs': {}}
        for l, rs in runs.items():
            base = runs[first]
            rec['summary'][l] = {'user_median_s': statistics.median(r['user_s'] for r in rs), 'host_insns_median': statistics.median(r['host_insns'] for r in rs), 'host_cycles_median': statistics.median(r['host_cycles'] for r in rs)}
            if l != first:
                pct = lambda key: round(statistics.median((r[key] / b[key] - 1) * 100 for r, b in zip(rs, base)), 2)
                rec['summary'][l].update({'user_paired_median_pct': pct('user_s'), 'host_insns_paired_median_pct': pct('host_insns'), 'host_cycles_paired_median_pct': pct('host_cycles'),
                                          'slower_pairs': sum(r['user_s'] > b['user_s'] for r, b in zip(rs, base))})
            rec['runs'][l] = {k: [r[k] for r in rs] for k in ('user_s', 'host_insns', 'host_cycles', 'load')}
        result['workloads'][w] = rec
        print(f"{w}: identical {rec['identical_work_and_console']} load {rec['load']}", flush=True)
        for l, s in rec['summary'].items():
            print(f"  {l:>10} {s['user_median_s']:.3f} s" + (f"  user {s['user_paired_median_pct']:+.2f}% ({s['slower_pairs']}/{a.rounds} slower), host insns {s['host_insns_paired_median_pct']:+.2f}%, cycles {s['host_cycles_paired_median_pct']:+.2f}%" if l != first else ''), flush=True)
    os.unlink(script.name)
    with open(a.out, 'w') as f: f.write(json.dumps(result, indent=1).replace('\n     ', ' ').replace('\n    ]', ']') + '\n')

main()
