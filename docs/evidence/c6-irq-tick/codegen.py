#!/usr/bin/env python3
"""Static release comparison, no timing. Usage: codegen.py BASE FIX OUT_DIR."""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import types

base, candidate, output = map(Path, sys.argv[1:])
output.mkdir(parents=True, exist_ok=True)
repo = Path(__file__).resolve().parents[3]
source = (repo / 'docs/evidence/classic-core/asmcmp.py').read_text().split('a, b = funcs(sys.argv[1])')[0]
normalizer = types.ModuleType('asmcmp')
exec(source, normalizer.__dict__)
parsed = []
artifacts = []
for label, binary in [('base', base), ('candidate', candidate)]:
    dis = output / (label + '.dis')
    with dis.open('w') as f:
        subprocess.run(['nice', '-n', '10', 'xcrun', 'llvm-objdump', '--disassemble', '--demangle', '--no-show-raw-insn', str(binary)], stdout=f, check=True)
    parsed.append(normalizer.funcs(dis))
    artifacts.append(dict(label=label, binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest()))
a, b = parsed
hot = re.compile(r'(Machine<.*>::(run|run_unmodeled|run_modeled|present_irqs|step_core|after_round)|::SocBus.*::(tick|read32|write32|fetch|next_deadline|periph_read|periph_write|load|store)|xtensa_lx7::.*::(run|step|exec)|esp32::periph::.*::(read|write)|esp_periph::.*::(tick|next_deadline|irq_sources))')
rows = []
for name in sorted(set(a) | set(b)):
    if not hot.search(name) or '{{closure}}' in name:
        continue
    same = name in a and name in b and normalizer.same(a, b, name)
    rows.append(dict(name=name, base_instructions=len(a.get(name, [])), candidate_instructions=len(b.get(name, [])), same=same))
    if not same:
        (output / ('difference-' + str(len(rows)) + '.json')).write_text(json.dumps(dict(name=name, base=a.get(name, []), candidate=b.get(name, [])), indent=2) + '\n')
assert rows
result = dict(method='EX230 normalizer; addresses normalized, field offsets/opcodes preserved; no timing', base_revision='142805d5399e0c3cf25995fe21ba524e75a2b3cb', artifacts=artifacts, selected=len(rows), same=sum(r['same'] for r in rows), rows=rows)
(output / 'codegen.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps({k:v for k,v in result.items() if k != 'rows'}, indent=2))
for row in rows:
    if not row['same']:
        print(row['base_instructions'], row['candidate_instructions'], row['name'])
