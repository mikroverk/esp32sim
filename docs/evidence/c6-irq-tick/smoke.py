#!/usr/bin/env python3
"""Engineering smoke only, not a formal CPU table.
Usage: smoke.py OUT BASE_PROBE CANDIDATE_PROBE ROM_DIR PUBLIC_STARTER_DIR
Each probe binary uses retired-probe.patch. Fetch the hello assets normally.
"""
import hashlib, json, os, re, resource, statistics, subprocess, sys, time
from pathlib import Path
E,base,candidate,fw,starter=map(lambda x:Path(x).resolve(),sys.argv[1:])
E.mkdir(parents=True,exist_ok=True)
def builds():
    inventory=subprocess.check_output(['ps','-axo','comm='],text=True)
    return sum(s.strip().rsplit('/',1)[-1] in ('cargo','rustc') for s in inventory.splitlines())
common=['--boot','rom','--rom',str(fw/'esp32c6_rev0_rom.elf'),'--no-dump']
cases={
 'hello-delay':common+['--flash-mb','4','--board','none','--bootloader',str(fw/'public/c6-hello-bootloader.bin'),'--ptable',str(fw/'public/c6-hello-ptable.bin'),'--app',str(fw/'public/c6-hello_world.bin')],
 'public-display-starter':common+['--flash-mb','8','--board','waveshare-c6-lcd147']+sum((['--flash-at',f'{offset}={starter/name}'] for offset,name in [('0','bootloader.bin'),('8000','partitions.bin'),('e000','boot_app0.bin'),('10000','firmware.bin')]),[])
}
rows=[]
for pair in range(3):
    for case,args in cases.items():
        for variant in (['base','candidate'] if pair%2==0 else ['candidate','base']):
            cmd=['nice','-n','10',str(base if variant=='base' else candidate)]+args+['--max-seconds','30']
            before_builds=builds();assert before_builds==0, f'builds active: {before_builds}'
            load=os.getloadavg();before=resource.getrusage(resource.RUSAGE_CHILDREN)
            start=time.monotonic();p=subprocess.run(cmd,stdout=subprocess.PIPE,stderr=subprocess.PIPE);wall=time.monotonic()-start
            after=resource.getrusage(resource.RUSAGE_CHILDREN);after_builds=builds()
            stem=f'{case}-{pair}-{variant}'
            (E/(stem+'.stdout')).write_bytes(p.stdout);(E/(stem+'.stderr')).write_bytes(p.stderr)
            retired=re.search(rb'\[probe\] core0 retired=(\d+)',p.stderr)
            row=dict(pair=pair,case=case,variant=variant,load=load,builds_before=before_builds,builds_after=after_builds,wall=wall,user=after.ru_utime-before.ru_utime,system=after.ru_stime-before.ru_stime,exit=p.returncode,console_sha256=hashlib.sha256(p.stdout).hexdigest(),retired_core0=int(retired[1]) if retired else None,command=[s.replace(str(Path.home()),'$HOME') for s in cmd])
            stop=re.search(rb'; (\d+) exceptions, (\d+) interrupts',p.stderr)
            row.update(exceptions=int(stop[1]) if stop else None, interrupts=int(stop[2]) if stop else None, irq_lines=[s.strip() for s in p.stderr.decode().splitlines() if s.startswith('  core')], console_bytes=len(p.stdout))
            rows.append(row);(E/'smoke.json').write_text(json.dumps(rows,indent=2)+'\n');print({k:v for k,v in row.items() if k!='command'},flush=True)
            assert p.returncode==0 and retired and after_builds==0
for case in cases:
    group=[r for r in rows if r['case']==case]
    assert len(set((r['console_sha256'],r['retired_core0'],r['exceptions'],r['interrupts'],tuple(r['irq_lines'])) for r in group))==1
    assert all(r['exceptions']==0 and r['interrupts'] is not None for r in group)
    for metric in ['wall','user']:
        b=statistics.median(r[metric] for r in group if r['variant']=='base');c=statistics.median(r[metric] for r in group if r['variant']=='candidate')
        print(case,metric,b,c,'reduction %',100*(b-c)/b)
