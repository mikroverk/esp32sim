#!/usr/bin/env python3
"""Exact-work checks, NOT a CPU measurement. OUT BASE_PROBE FIX_PROBE ROM_DIR [STARTER_DIR]."""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

out, base, fix, fw = map(lambda s: Path(s).resolve(), sys.argv[1:5])
out.mkdir(parents=True, exist_ok=True)
common = ['--boot', 'rom', '--rom', str(fw / 'esp32c6_rev0_rom.elf'), '--no-dump', '--max-seconds', '30']
cases = {'hello-delay': common + ['--flash-mb', '4', '--board', 'none', '--bootloader', str(fw / 'public/c6-hello-bootloader.bin'), '--ptable', str(fw / 'public/c6-hello-ptable.bin'), '--app', str(fw / 'public/c6-hello_world.bin')]}
if len(sys.argv) == 6:
    starter = Path(sys.argv[5]).resolve()
    cases['public-display-starter'] = common + ['--flash-mb', '8', '--board', 'waveshare-c6-lcd147'] + sum((['--flash-at', f'{offset}={starter / name}'] for offset, name in [('0', 'bootloader.bin'), ('8000', 'partitions.bin'), ('e000', 'boot_app0.bin'), ('10000', 'firmware.bin')]), [])
rows = []
for case, args in cases.items():
    pair = []
    for variant, binary in [('base', base), ('candidate', fix)]:
        p = subprocess.run(['nice', '-n', '10', str(binary)] + args, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        (out / f'{case}-{variant}.stdout').write_bytes(p.stdout)
        (out / f'{case}-{variant}.stderr').write_bytes(p.stderr)
        retired = re.search(rb'\[probe\] core0 retired=(\d+)', p.stderr)
        stop = re.search(rb'; (\d+) exceptions, (\d+) interrupts', p.stderr)
        assert p.returncode == 0 and retired and stop
        work = dict(console_sha256=hashlib.sha256(p.stdout).hexdigest(), console_bytes=len(p.stdout), retired_core0=int(retired[1]), exceptions=int(stop[1]), interrupts=int(stop[2]), irq_lines=[line.strip() for line in p.stderr.decode().splitlines() if line.startswith('  core')])
        rows.append(dict(case=case, variant=variant, binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), exit=p.returncode, **work))
        pair.append(work)
        print(rows[-1], flush=True)
    assert pair[0] == pair[1], case
(out / 'exact-work.json').write_text(json.dumps(rows, indent=2) + '\n')
