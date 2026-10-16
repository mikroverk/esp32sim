#!/usr/bin/env python3
"""Public idle/non-CRC controls. Usage: controls.py BASE FIX OUTPUT_DIR. No CPU timing."""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

base, fix, out = map(lambda p: Path(p).resolve(), sys.argv[1:])
out.mkdir(parents=True, exist_ok=True)
repo = Path(__file__).resolve().parents[3]
rows = []
for chip, prefix, rom in [('esp32','classic-hello','esp32_rev300_rom.elf'),('esp32s3','hello','esp32s3_rev0_rom.elf'),('esp32c3','c3-hello','esp32c3_rev3_rom.elf'),('esp32c6','c6-hello','esp32c6_rev0_rom.elf')]:
    args = ['--chip',chip,'--board','none','--rom','web/wasm/fw/'+rom,'--boot','rom','--flash-mb','4','--console','uart0','--no-dump',
            '--bootloader','web/wasm/fw/public/'+prefix+'-bootloader.bin','--ptable','web/wasm/fw/public/'+prefix+'-ptable.bin',
            '--app','web/wasm/fw/public/'+prefix+'_world.bin','--max-seconds','3']
    results = []
    for name, binary in [('base',base),('fix',fix)]:
        p = subprocess.run(['nice','-n','10',str(binary)]+args,cwd=repo,capture_output=True,check=True)
        (out/(chip+'-'+name+'.stdout')).write_bytes(p.stdout)
        (out/(chip+'-'+name+'.stderr')).write_bytes(p.stderr)
        log = p.stderr.decode()
        stop = next(line for line in log.splitlines() if line.startswith('[emu] stop:'))
        work = re.search(r'core0 (\d+)(?: \+ core1 (\d+))?',stop)
        if work:
            instructions = [int(n) for n in work.groups() if n]
        else:
            single = re.search(r'— (\d+) insns', stop)
            assert single, stop
            instructions = [int(single[1])]
        results.append(dict(instructions=instructions,console_sha256=hashlib.sha256(p.stdout).hexdigest(),events=stop.split('); ')[1],irq=[l for l in log.splitlines() if l.startswith(('[irq]','  core'))]))
    assert results[0] == results[1], (chip,results)
    rows.append(dict(chip=chip,results=results))
(out/'controls.json').write_text(json.dumps(rows,indent=2)+'\n')
print(json.dumps(rows,indent=2))
