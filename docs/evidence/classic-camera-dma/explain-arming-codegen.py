#!/usr/bin/env python3
"""Explain, not normalize away, the C7.1 tick spill/unused-argument delta."""
import hashlib, json, re, sys, types
from pathlib import Path
root, data, output = (Path(p).resolve() for p in sys.argv[1:4])
m = types.ModuleType('asmcmp')
exec((root/'docs/evidence/classic-core/asmcmp.py').read_text().split('a, b = funcs(sys.argv[1])')[0], m.__dict__)
receipt=json.loads((data/'codegen.json').read_text())
parsed=[]
for label in ['base','candidate']:
    path=data/(label+'.dis')
    artifact=next(a for a in receipt['artifacts'] if a['label']==label)
    assert hashlib.sha256(path.read_bytes()).hexdigest()==artifact['disassembly_sha256']
    parsed.append(m.funcs(path))
name='<esp32::bus::SocBus as emu_core::bus::Bus>::tick'
a,b=(p[name] for p in parsed)
call='bl\t<<esp32::bus::SocBus>::camera_dma_data>'
i=a.index(call); j=b.index(call)
assert re.fullmatch(r'ldr\tx2, \[sp, #0x[0-9a-f]+\]',a[i-1]),a[i-1]
assert a[i-2]==b[j-1]=='mov\tx1, x19'
spill=lambda s: re.sub(r'\[sp, #0x[0-9a-f]+\]', '[sp, #SPILL]', s)
explained=a[:i-1]+a[i:]
assert list(map(spill,explained))==list(map(spill,b)), 'non-spill delta remains'
result=dict(base=receipt['base'],method=receipt['method'],artifacts=receipt['artifacts'],rows=receipt['rows'],explanation=dict(function=name,removed_instruction=a[i-1],reason='camera_dma_data no longer consumes the cycles argument; its old argument load disappears only in the existing active camera branch. All remaining differences are SP-relative spill slot assignments, verified after this single exact call-site removal. Equality score remains 20/21; not whole-binary equivalence or a CPU measurement.',strictly_identical=sum(r['same'] for r in receipt['rows']),compared=len(receipt['rows']),base_instructions=len(a),candidate_instructions=len(b),sp_slot_normalized_sequences_equal=True,base_guard_window=a[i-45:i+3],candidate_guard_window=b[j-45:j+3]))
output.write_text(json.dumps(result,indent=2)+'\n')
assert output.stat().st_size<50000
print('Verified: one camera-branch argument load removed; other tick deltas are SP spill slots only.')
