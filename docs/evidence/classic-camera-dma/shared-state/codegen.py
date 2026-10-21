#!/usr/bin/env python3
"""Compare release step loops with the repository's existing address normalizer."""
from pathlib import Path
import hashlib,json,re,subprocess,types,difflib,sys
r=Path(sys.argv[1]).resolve()
e=Path(sys.argv[2]).resolve(); e.mkdir(parents=True,exist_ok=True)
m=types.ModuleType('asmcmp')
exec((r/'docs/evidence/classic-core/asmcmp.py').read_text().split('a, b = funcs(sys.argv[1])')[0],m.__dict__)
selector=re.compile(r'(Machine<esp32(?:::|s3::|c3::|c6::).*>::(?:run|step_core|after_round|present_irqs)|esp32(?:::|s3::|c3::|c6::)bus::SocBus.*::(?:tick|read32|write32|fetch|next_deadline)|(?:xtensa_lx7|riscv_rv32)::.*::(?:run|step|exec).*esp32(?:::|s3::|c3::|c6::)bus::SocBus)')
rows=[];artifacts=[]
for binary in ['esp32sim','esp32sim-c3','esp32sim-c6']:
 parsed=[]
 for label,target in [('base','base-target'),('candidate','target')]:
  p=e/target/'release'/binary
  dis=e/f'{label}-{binary}.dis'
  with dis.open('w') as f:subprocess.run(['nice','-n','10','xcrun','llvm-objdump','--disassemble','--demangle','--no-show-raw-insn',str(p)],stdout=f,check=True)
  parsed.append(m.funcs(dis))
  artifacts.append(dict(label=label,binary=binary,sha256=hashlib.sha256(p.read_bytes()).hexdigest(),disassembly_sha256=hashlib.sha256(dis.read_bytes()).hexdigest()))
 a,b=parsed
 for name in sorted(set(a)|set(b)):
  if not selector.search(name) or '{{closure}}' in name:continue
  same=name in a and name in b and m.same(a,b,name)
  row=dict(binary=binary,name=name,same=same,base_instructions=len(a.get(name,[])),candidate_instructions=len(b.get(name,[])))
  if not same:row['diff']=list(difflib.unified_diff(a.get(name,[]),b.get(name,[]),n=2))
  rows.append(row)
assert rows
out=dict(base='142805d5399e0c3cf25995fe21ba524e75a2b3cb',method='EX230 asmcmp: addresses/page relocations and equivalent call aliases normalized; selected hot functions, not entire binary equivalence',selector=selector.pattern,artifacts=artifacts,rows=rows)
(e/'codegen.json').write_text(json.dumps(out,indent=2)+'\n')
print('identical',sum(x['same'] for x in rows),'total',len(rows))
for x in rows:
 if not x['same']:print(x)
