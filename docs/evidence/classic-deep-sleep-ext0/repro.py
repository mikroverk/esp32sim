#!/usr/bin/env python3
"""Optional public starter: repro.py BASE FIX ROM FIXTURE BUTTON_SCRIPT OUTPUT.
No firmware is distributed. FIXTURE: firmware.bin, firmware.elf, bootloader.bin,
partitions.bin. OUTPUT holds full local captures; only repro.json is curated.
"""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

base, fix, rom, fixture, script, out = [Path(x).resolve() for x in sys.argv[1:]]
out.mkdir(parents=True, exist_ok=True)
inputs = [rom, script] + [fixture/n for n in ['firmware.bin','firmware.elf','bootloader.bin','partitions.bin']]
manifest = {p.name:dict(sha256=hashlib.sha256(p.read_bytes()).hexdigest(),bytes=p.stat().st_size) for p in inputs}
rows=[]
for name,binary,seconds in [('base-2',base,2),('base-5',base,5),('fix-2',fix,2),('fix-2.5',fix,2.5),('fix-5',fix,5)]:
    args=['--chip','esp32','--board','ttgo-tdisplay','--boot','rom','--rom',str(rom),'--console','uart0','--flash-mb','4',
          '--bootloader',str(fixture/'bootloader.bin'),'--ptable',str(fixture/'partitions.bin'),
          '--app',str(fixture/'firmware.bin'),'--elf',str(fixture/'firmware.elf'),'--script',str(script),
          '--max-seconds',str(seconds),'--trace-fn','_Z5setupv','--peek','0x3ff48018,1','--peek','0x3ff48034,2']
    r=subprocess.run(['nice','-n','10',str(binary)]+args,capture_output=True,check=True)
    (out/(name+'.stdout')).write_bytes(r.stdout); (out/(name+'.stderr')).write_bytes(r.stderr)
    log=r.stderr.decode(); console=r.stdout.decode()
    pcs=re.findall(r'^core([01]): pc=([0-9a-f]+).*?insns=(\d+)',log,re.M)
    counts=[dict(core=int(c),pc=p,instructions=int(n)) for c,p,n in pcs]
    after=log.split('[peek after run]',1)[1]
    registers=dict(re.findall(r'^(3ff[0-9a-f]+): ([0-9a-f]+)$',after,re.M))
    setups=re.findall(r'^\[fn\].*_Z5setupv.*$',log,re.M)
    resets=re.findall(r'rst:0x[0-9a-f]+ \([^)]*\)',console)
    row=dict(name=name,cores=counts,registers=registers,setup_entries=setups,resets=resets,
             console_sha256=hashlib.sha256(r.stdout).hexdigest(),binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
    if name.startswith('base'):
        assert any(x['pc']=='40086b97' for x in counts),row
        assert len(setups)==1 and len(resets)==1,row
    elif seconds<3:
        assert registers['3ff48018']=='80000000',row
        assert len(setups)==1 and len(resets)==1,row
    else:
        assert resets==['rst:0x1 (POWERON_RESET)','rst:0x5 (DEEPSLEEP_RESET)'],row
        assert len(setups)==2 and registers['3ff48034']=='00000145',row
    rows.append(row)
assert rows[2]['cores']==rows[3]['cores'], 'sleep must retire no instructions on either core'
assert rows[1]['cores'][1]['instructions']>rows[0]['cores'][1]['instructions'], 'base still spins'
result=dict(inputs=manifest,results=rows)
(out/'repro.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
