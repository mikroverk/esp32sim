#!/usr/bin/env python3
"""Compare classic tick/I2S release code, normalizing addresses, not instructions."""
from pathlib import Path
import sys, subprocess, types, re, json, difflib, hashlib
r=Path(sys.argv[1]).resolve();e=Path(sys.argv[2]).resolve()
m=types.ModuleType('asmcmp')
exec((r/'docs/evidence/classic-core/asmcmp.py').read_text().split('a, b = funcs(sys.argv[1])')[0],m.__dict__)
parsed=[];artifacts=[]
for label,target in [('base','base-target'),('candidate','target')]:
 p=e/target/'release/esp32sim';dis=e/(label+'.dis')
 with dis.open('w') as f:subprocess.run(['nice','-n','10','xcrun','llvm-objdump','--disassemble','--demangle','--no-show-raw-insn',str(p)],stdout=f,check=True)
 parsed.append(m.funcs(dis));artifacts.append(dict(label=label,binary_sha256=hashlib.sha256(p.read_bytes()).hexdigest(),disassembly_sha256=hashlib.sha256(dis.read_bytes()).hexdigest()))
a,b=parsed
selector=re.compile(r'(esp32::bus::SocBus.*::(?:tick|i2s_step)|esp32::i2s::ClassicI2s.*::data|Machine<esp32::soc::Esp32>.*::(?:run|step_core|after_round))')
rows=[]
for name in sorted(set(a)|set(b)):
 if not selector.search(name) or '{{closure}}' in name:continue
 same=name in a and name in b and m.same(a,b,name)
 row=dict(name=name,same=same,base_instructions=len(a.get(name,[])),candidate_instructions=len(b.get(name,[])))
 if not same:row['diff']=list(difflib.unified_diff(a.get(name,[]),b.get(name,[]),n=2))
 rows.append(row)
assert rows,'no selected code'
assert len(rows) == 5 and all(row['same'] for row in rows), 'classic codegen differs'
out=dict(base='42d5c9dbe29827bfe2271777b74e7af914559c2f',method='classic-core asmcmp address/page/call-alias normalization; not CPU timing',selector=selector.pattern,artifacts=artifacts,rows=rows)
(e/'codegen.json').write_text(json.dumps(out,indent=2)+'\n')
print(json.dumps(out,indent=2))
