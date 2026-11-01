#!/usr/bin/env python3
"""Check the post-run counter probe leaves selected hot code unchanged.

OUTPUT NORMAL_BASE PROBE_BASE NORMAL_CANDIDATE PROBE_CANDIDATE.
"""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import types
repo=Path(__file__).resolve().parents[3]
out=Path(sys.argv[1]).resolve()
out.mkdir(parents=True,exist_ok=True)
m=types.ModuleType('asmcmp')
exec((repo/'docs/evidence/classic-core/asmcmp.py').read_text().split('a, b = funcs(sys.argv[1])')[0],m.__dict__)
hot=re.compile(r'riscv_rv32::|Machine<.*>::(?:run|run_unmodeled|run_modeled|present_irqs|step_core|after_round)|SocBus.*::(?:tick|fetch|read32|write32|next_deadline|refresh_irq)|xtensa_lx7::.*::(?:run|step|exec)')
rows=[]
for label,normal,probe in [('base',sys.argv[2],sys.argv[3]),('candidate',sys.argv[4],sys.argv[5])]:
    parsed=[]
    for variant,binary in [('normal',normal),('probe',probe)]:
        dis=out/(label+'-'+variant+'.dis')
        with dis.open('w') as log:
            subprocess.run(['nice','-n','10','xcrun','llvm-objdump','--disassemble','--demangle','--no-show-raw-insn',binary],stdout=log,check=True)
        parsed.append(m.funcs(dis))
    a,b=parsed
    # asmcmp normalizes explicit ADRP page offsets but misses zero offsets,
    # printed as [xN]. Handle only a load directly after its matching ADRP.
    for fs in (a,b):
        for body in fs.values():
            for i in range(1,len(body)):
                prev=re.fullmatch(r'adrp (x\d+)', body[i-1])
                if prev:
                    body[i]=re.sub(r'\['+prev.group(1)+r'\]', '['+prev.group(1)+', #PAGEOFF]', body[i])
    names=[n for n in set(a)|set(b) if hot.search(n) and '{closure' not in n]
    changed=[n for n in sorted(names) if n not in a or n not in b or not m.same(a,b,n)]
    rows.append({'variant':label,'selected':len(names),'identical':len(names)-len(changed),'changed':changed,'normal_sha256':hashlib.sha256(Path(normal).read_bytes()).hexdigest(),'probe_sha256':hashlib.sha256(Path(probe).read_bytes()).hexdigest()})
(out/'probe-codegen.json').write_text(json.dumps(rows,indent=2)+'\n')
print(json.dumps(rows,indent=2))
assert all(not r['changed'] for r in rows), 'probe altered selected hot code'
