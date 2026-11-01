#!/usr/bin/env python3
"""EX168 RV32 result-only codegen gate. BASE CANDIDATE OUTPUT_DIR.

Requires Apple llvm-objdump. Reuses the EX230 address normalizer; this is
an emitted-host-operations check, not an elapsed-time benchmark.
"""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import types

base, candidate, out = map(lambda s: Path(s).resolve(), sys.argv[1:])
out.mkdir(parents=True, exist_ok=True)
repo=Path(__file__).resolve().parents[3]
m=types.ModuleType('asmcmp')
exec((repo/'docs/evidence/classic-core/asmcmp.py').read_text().split('a, b = funcs(sys.argv[1])')[0],m.__dict__)
parsed=[]
for label,binary in [('base',base),('candidate',candidate)]:
    dis=out/(label+'.dis')
    with dis.open('w') as log:
        subprocess.run(['nice','-n','10','xcrun','llvm-objdump','--disassemble','--demangle','--no-show-raw-insn',str(binary)],stdout=log,check=True)
    parsed.append(m.funcs(dis))
a,b=parsed
step='riscv_rv32::exec::step_outcome::<esp32c6::bus::SocBus>'
run='<riscv_rv32::state::Cpu as emu_core::core::Core>::run::<esp32c6::bus::SocBus>'
# No claim about record-store counts: the previous pattern does not match main.

hot=re.compile(r'riscv_rv32::|Machine<.*>::(?:run|run_unmodeled|run_modeled|present_irqs|step_core|after_round)|SocBus.*::(?:tick|fetch|read32|write32|next_deadline|refresh_irq)|xtensa_lx7::.*::(?:run|step|exec)')
rows=[]
for name in sorted(set(a)|set(b)):
    if not hot.search(name) or '{closure' in name: continue
    rows.append({'name':name,'base_ops':len(a.get(name,[])),'candidate_ops':len(b.get(name,[])), 'same':name in a and name in b and m.same(a,b,name)})
gate=run in b and step not in b and not any('riscv_rv32::exec::exec_insn::<' in n for n in b)
result={'method':'EX230 normalized native AArch64 code; static body sizes are not dynamic instruction counts',
        'binary_sha256':{'base':hashlib.sha256(base.read_bytes()).hexdigest(),'candidate':hashlib.sha256(candidate.read_bytes()).hexdigest()},
        'gate':run in b and step not in b and not any('riscv_rv32::exec::exec_insn::<' in n for n in b),
        'selected':len(rows),'identical':sum(r['same'] for r in rows),'rows':rows}
(out/'codegen.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps({k:v for k,v in result.items() if k!='rows'},indent=2))
for r in rows:
    if not r['same']: print(r['base_ops'],r['candidate_ops'],r['name'])
assert result['gate'], 'result-only path must inline event construction and execution; no per-instruction StepOutcome/exec call'
