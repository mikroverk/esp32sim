#!/usr/bin/env python3
"""Compare C3/C6 guest work without timing. OUT BASE_PREFIX CAND_PREFIX FW STARTER.

Each prefix names four binaries, e.g. BASE_PREFIX-normal-c3 and
BASE_PREFIX-probe-c6. Check hot-code identity before accepting probe counts.
"""
import hashlib,json,re,subprocess,sys
from pathlib import Path
out,base,cand,fw,starter=(Path(x).resolve() for x in sys.argv[1:])
out.mkdir(parents=True,exist_ok=True)
manifest=json.loads((Path(__file__).parent/'inputs.json').read_text())
for n,h in manifest['starter'].items():
    assert hashlib.sha256((starter/n).read_bytes()).hexdigest()==h,n
for n,h in manifest['firmware'].items():
    assert hashlib.sha256((fw/n).read_bytes()).hexdigest()==h,n
cases={}
for chip in ('c3','c6'):
    rom='esp32c3_rev3_rom.elf' if chip=='c3' else 'esp32c6_rev0_rom.elf'
    cases[chip+'-hello']=(chip,['--boot','rom','--rom',str(fw/rom),'--no-dump','--max-seconds','10','--flash-mb','4','--board','none','--bootloader',str(fw/'public'/f'{chip}-hello-bootloader.bin'),'--ptable',str(fw/'public'/f'{chip}-hello-ptable.bin'),'--app',str(fw/'public'/f'{chip}-hello_world.bin')])
cases['c6-starter']=('c6',['--boot','rom','--rom',str(fw/'esp32c6_rev0_rom.elf'),'--no-dump','--max-seconds','10','--flash-mb','8','--board','waveshare-c6-lcd147']+sum((['--flash-at',f'{off}={starter/name}'] for off,name in [('0','bootloader.bin'),('8000','partitions.bin'),('e000','boot_app0.bin'),('10000','firmware.bin')]),[]))
rows=[]
for case,(chip,args) in cases.items():
    comparison=[]
    for arm,prefix in [('base',base),('cand',cand)]:
        for typ in ('normal','probe'):
            binary=Path(str(prefix)+'-'+typ+'-'+chip)
            proc=subprocess.run(['nice','-n','10',str(binary)]+args,capture_output=True,timeout=600)
            text=proc.stderr.decode(errors='replace')
            stop=re.search(r'\[emu\] stop: (.*)',text)
            retired=re.search(r'\[probe\] core0 retired=(\d+)',text)
            assert proc.returncode==0 and stop and (retired or typ=='normal'),(case,arm,typ,text[-1000:])
            work=dict(console_sha256=hashlib.sha256(proc.stdout).hexdigest(),stop=re.sub(r' in .*?; emulated','; emulated',stop.group(1)),irqs=[l.strip() for l in text.splitlines() if l.startswith('  core')])
            row=dict(case=case,arm=arm,typ=typ,exit=proc.returncode,binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),**work,retired=int(retired.group(1)) if retired else None)
            rows.append(row); comparison.append(work)
            print(json.dumps(row),flush=True)
    assert all(x==comparison[0] for x in comparison),case
    assert rows[-1]['retired']==rows[-3]['retired'],case
(out/'exact-work.json').write_text(json.dumps(rows,indent=2)+'\n')
