#!/usr/bin/env python3
"""Usage: FW=... ESP32SIM_ROM_DIR=... ESP_EMU=... MAIN=... NEW=... OUT=x.json coremark_bench.py ROUNDS CHIP,CHIP...
CoreMark (Espressif test_apps/coremark; IDF v5.5.4, S31 v6.1) on esp-emu 1.2.0 and esp32sim main vs the
RV32 code-version branch, native AArch64 macOS, alternating order per round. Reports user CPU s and host
instructions retired (/usr/bin/time -l), checks crcfinal 0xa14c and the score."""
import os, re, subprocess, sys, json, statistics
# Caller-supplied: FW (dir of <chip>-{bootloader,ptable,app,merged}.bin), ESP32SIM_ROM_DIR, ESP_EMU (binary),
# MAIN and NEW (esp32sim release dirs), OUT (json).
FW = os.environ['FW']; ROM = os.environ['ESP32SIM_ROM_DIR']; EMU = os.environ['ESP_EMU']
SIDES = {'main': os.environ['MAIN'], 'new': os.environ['NEW']}
ROMS = {'esp32c3': 'esp32c3_rev3_rom.elf', 'esp32c6': 'esp32c6_rev0_rom.elf', 'esp32s3': 'esp32s3_rev0_rom.elf', 'esp32p4': 'esp32p4_rev300_rom.elf', 'esp32s31': 'esp32s31_rev0_rom.elf'}
EXE = {'esp32c3': 'esp32sim-c3', 'esp32c6': 'esp32sim-c6', 'esp32s3': 'esp32sim', 'esp32p4': 'esp32sim-p4', 'esp32s31': 'esp32sim-s31'}
def cmd(side, chip):
    if side == 'esp-emu':
        return [EMU, '--chip', chip, '--firmware', f'{FW}/{chip}-merged.bin', '--exit-on', 'CPU frequency', '--timeout', '600s'] + (['--rom', f'{ROM}/{ROMS[chip]}'] if chip == 'esp32s31' else [])
    extra = (['--board', 'none'] if chip == 'esp32s3' else []) + ['--flash-mb', '16' if chip == 'esp32s31' else '4']
    return [f'{SIDES[side]}/{EXE[chip]}', '--boot', 'rom', '--rom', f'{ROM}/{ROMS[chip]}', *extra, '--bootloader', f'{FW}/{chip}-bootloader.bin',
            '--ptable', f'{FW}/{chip}-ptable.bin', '--app', f'{FW}/{chip}-app.bin', '--no-dump', '--exit-on', 'CPU frequency', '--max-seconds', '120']
def run(side, chip):
    p = subprocess.run(['/usr/bin/time', '-l'] + cmd(side, chip), capture_output=True)
    out, err = p.stdout.decode(errors='replace'), p.stderr.decode(errors='replace')
    crc = re.search(r'\[0\]crcfinal\s*:\s*(0x[0-9a-f]+)', out); score = re.search(r'Iterations/Sec\s*:\s*([\d.]+)', out)
    user = re.search(r'([\d.]+) user', err); ins = re.search(r'(\d+)\s+instructions retired', err)
    stop = re.search(r'stop: .*', err)
    if p.returncode or not crc or crc.group(1) != '0xa14c': sys.exit(f'{side} {chip}: rc {p.returncode}\n{out[-600:]}\n{err[-600:]}')
    return {'user_s': float(user.group(1)), 'host_insns': int(ins.group(1)), 'score': float(score.group(1)), 'stop': stop.group(0) if stop else None, 'load': os.getloadavg()[0]}
rounds = int(sys.argv[1]); chips = sys.argv[2].split(','); res = {}
for chip in chips:
    sides = ['esp-emu', 'main', 'new']
    for s in sides: run(s, chip)
    r = {s: [] for s in sides}
    for i in range(rounds):
        order = sides if i % 2 == 0 else sides[::-1]
        for s in order: r[s].append(run(s, chip))
    med = {s: (statistics.median(x['user_s'] for x in v), statistics.median(x['host_insns'] for x in v)) for s, v in r.items()}
    res[chip] = r
    print(f"{chip}: " + '; '.join(f"{s} {u:.2f} s {h/1e9:.1f} G" for s, (u, h) in med.items()) + f"; new vs esp-emu {med['esp-emu'][0]/med['new'][0]:.2f}x faster (user), main->new {med['main'][0]/med['new'][0]:.2f}x; scores emu {r['esp-emu'][0]['score']:.1f} main {r['main'][0]['score']:.1f} new {r['new'][0]['score']:.1f}; load {min(x['load'] for v in r.values() for x in v):.1f}-{max(x['load'] for v in r.values() for x in v):.1f}", flush=True)
json.dump(res, open(os.environ['OUT'], 'w'), indent=1)
