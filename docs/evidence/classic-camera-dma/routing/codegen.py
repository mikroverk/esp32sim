#!/usr/bin/env python3
"""Compare selected classic hot functions with the inherited EX230 normalizer."""
import difflib, hashlib, json, re, subprocess, sys, types
from pathlib import Path
root, base, candidate, output = map(lambda x: Path(x).resolve(), sys.argv[1:5])
output.mkdir(parents=True, exist_ok=True)
m = types.ModuleType('asmcmp')
exec((root/'docs/evidence/classic-core/asmcmp.py').read_text().split('a, b = funcs(sys.argv[1])')[0], m.__dict__)
selector = re.compile(r'(Machine<esp32::.*>::(?:run|step_core|after_round|present_irqs)|esp32::bus::SocBus.*::(?:tick|read32|write32|fetch|next_deadline)|xtensa_lx7::.*::(?:run|step|exec).*esp32::bus::SocBus)')
parsed=[]; artifacts=[]
for label, binary in [('base', base), ('candidate', candidate)]:
 dis=output/(label+'.dis')
 with dis.open('w') as f:
  subprocess.run(['nice','-n','10','xcrun','llvm-objdump','--disassemble','--demangle','--no-show-raw-insn',str(binary)],stdout=f,check=True)
 parsed.append(m.funcs(dis))
 artifacts.append(dict(label=label,sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),disassembly_sha256=hashlib.sha256(dis.read_bytes()).hexdigest()))
a,b=parsed; rows=[]
for name in sorted(set(a)|set(b)):
 if not selector.search(name) or '{{closure}}' in name: continue
 same=name in a and name in b and m.same(a,b,name)
 row=dict(name=name,same=same,base_instructions=len(a.get(name,[])),candidate_instructions=len(b.get(name,[])))
 if not same: row['diff']=list(difflib.unified_diff(a.get(name,[]),b.get(name,[]),n=2))
 rows.append(row)
result=dict(base='de9f3978df0a25af2e4fbcafc337ca22c75146af',method='EX230 address/page relocation and equivalent call alias normalization; differences retained, not whole binary equivalence',selector=selector.pattern,artifacts=artifacts,rows=rows)
(output/'codegen.json').write_text(json.dumps(result,indent=2)+'\n')
print('identical',sum(row['same'] for row in rows),'total',len(rows))
for row in rows:
 if not row['same']: print(row['name'],row['base_instructions'],row['candidate_instructions'])
assert rows
