#!/usr/bin/env python3
"""Optional public-starter repro; no firmware is distributed here.
Usage: repro.py BASE_BINARY FIX_BINARY ROM_ELF FIXTURE_DIR BUTTON_SCRIPT OUTPUT_DIR
FIXTURE_DIR contains firmware.bin, firmware.elf, bootloader.bin, partitions.bin.
"""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

base, fix, rom, fixture, script, out = map(lambda p: Path(p).resolve(), sys.argv[1:])
out.mkdir(parents=True, exist_ok=True)
inputs = [rom, script] + [fixture/name for name in ['firmware.bin','firmware.elf','bootloader.bin','partitions.bin']]
manifest = {p.name:dict(sha256=hashlib.sha256(p.read_bytes()).hexdigest(),bytes=p.stat().st_size) for p in inputs}
rows = []
for name, binary, seconds in [('main-5s',base,5),('fix-2s',fix,2),('fix-5s',fix,5)]:
    args = ['--chip','esp32','--board','ttgo-tdisplay','--rom',str(rom),'--boot','rom','--console','uart0','--flash-mb','4',
            '--bootloader',str(fixture/'bootloader.bin'),'--ptable',str(fixture/'partitions.bin'),
            '--app',str(fixture/'firmware.bin'),'--elf',str(fixture/'firmware.elf'),
            '--script',str(script),'--max-seconds',str(seconds),'--peek','0x3ff48100,2','--peek','0x3ff48018,1','--peek','0x3ff48040,1']
    run = subprocess.run(['nice','-n','10',str(binary)]+args,capture_output=True,check=True)
    (out/(name+'.stdout')).write_bytes(run.stdout)
    (out/(name+'.stderr')).write_bytes(run.stderr)
    log = run.stderr.decode()
    after = log.split('[peek after run]', 1)[1]
    registers = dict(re.findall(r'^(3ff[0-9a-f]+): ([0-9a-f]+)$', after, re.M))
    pcs = re.findall(r'^core[01]: pc=[^\n]+', log, re.M)
    if name.startswith('main'):
        assert registers['3ff48100'] == '7ff00100' and registers['3ff48104'] == '00000000'
        assert any('pc=400081ac ' in pc for pc in pcs)
    else:
        assert registers == {'3ff48100':'7ff00000', '3ff48104':'064fb977', '3ff48018':'80000000', '3ff48040':'00000000'}, registers
        assert any('pc=40086b97 ' in pc for pc in pcs)
    rows.append(dict(name=name,registers=registers,pcs=pcs,binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest()))
(out/'repro.json').write_text(json.dumps(dict(inputs=manifest,results=rows),indent=2)+'\n')
print(json.dumps(rows,indent=2))
