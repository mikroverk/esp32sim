#!/usr/bin/env python3
"""Static linked hot-path comparison, no timing. Arguments: BASE_DIR HEAD_DIR OUTPUT."""
import hashlib
import difflib
import json
from pathlib import Path
import re
import subprocess
import struct
import sys
import types

base, head, out = map(Path, sys.argv[1:])
out.mkdir(parents=True, exist_ok=True)
for stale in out.glob('*-difference-*.json'):
    stale.unlink()
root = Path(__file__).resolve().parents[3]
# Reuse the repository's established address normalizer, preserving field offsets,
# opcodes, registers and instruction order rather than duplicating its mechanism.
normalizer = types.ModuleType('asmcmp')
source = (root / 'docs/evidence/classic-core/asmcmp.py').read_text().split('a, b = funcs(sys.argv[1])')[0]
exec(source, normalizer.__dict__)

def literal_bytes(binary, address):
    """Read a linked Mach-O literal by virtual address, without a host identity."""
    data = binary.read_bytes()
    assert struct.unpack_from('<I', data)[0] == 0xfeedfacf
    ncmds = struct.unpack_from('<I', data, 16)[0]
    pos = 32
    for _ in range(ncmds):
        cmd, size = struct.unpack_from('<II', data, pos)
        if cmd == 0x19:
            vmaddr, _, fileoff, filesize = struct.unpack_from('<QQQQ', data, pos + 24)
            if vmaddr <= address and address + 16 <= vmaddr + filesize:
                offset = fileoff + address - vmaddr
                return data[offset:offset+16].hex()
        pos += size
    raise ValueError('literal outside file-backed segments')

def q1_literals(dis, symbol, binary):
    body = dis.read_text().split('<' + symbol + '>:\n')[1].split('\n\n')[0]
    matches = re.findall(r'adrp\tx8, (0x[0-9a-f]+)[^\n]*\n[^\n]*ldr\tq1, \[x8(?:, #(0x[0-9a-f]+))?\]', body)
    return [literal_bytes(binary, int(page, 16) + (int(offset, 16) if offset else 0)) for page, offset in matches]

summary = []
for name in ['esp32sim', 'esp32sim-c3', 'esp32sim-c6']:
    parsed = []
    artifacts = []
    for label, directory in [('base', base), ('head', head)]:
        binary = directory / name
        dis = out / (name + '-' + label + '.dis')
        with dis.open('w') as f:
            subprocess.run(['nice', '-n', '10', 'xcrun', 'llvm-objdump', '--disassemble', '--demangle', '--no-show-raw-insn', str(binary)], check=True, stdout=f)
        parsed.append(normalizer.funcs(dis))
        artifacts.append({'label': label, 'sha256': hashlib.sha256(binary.read_bytes()).hexdigest()})
    a, b = parsed
    hot = re.compile(r'(Machine<.*>::(run|run_unmodeled|run_modeled|present_irqs|step_core|after_round)|::SocBus.*::(tick|read32|write32|fetch|next_deadline|periph_read|periph_write|load|store)|xtensa_lx7::.*::(run|step|exec)|esp32::periph::.*::(read|write)|esp_periph::.*::(tick|next_deadline|irq_sources))')
    rows = []
    for symbol in sorted(set(a) | set(b)):
        if not hot.search(symbol) or '{{closure}}' in symbol:
            continue
        same = symbol in a and symbol in b and normalizer.same(a, b, symbol)
        rows.append({'name': symbol, 'base_instructions': len(a.get(symbol, [])), 'head_instructions': len(b.get(symbol, [])), 'same': same})
        if not same:
            delta = list(difflib.unified_diff(a.get(symbol, []), b.get(symbol, []), fromfile='base', tofile='head', n=2, lineterm=''))
            literals = {label: q1_literals(out / (name + '-' + label + '.dis'), symbol, directory / name) for label, directory in [('base', base), ('head', head)]}
            changed = [line[1:] for line in delta if line.startswith(('+', '-')) and not line.startswith(('+++', '---'))]
            relocation_only = bool(changed) and all(line in ['ldr\tq1, [x8]', 'ldr\tq1, [x8, #PAGEOFF]'] for line in changed)
            equal_literals = bool(literals['base']) and literals['base'] == literals['head']
            rows[-1]['constant_pool_relocation_only'] = relocation_only and equal_literals
            (out / (name + '-difference-' + str(len(rows)) + '.json')).write_text(json.dumps({'name': symbol, 'normalized_diff': delta, 'q1_literal_bytes': literals, 'constant_pool_relocation_only': relocation_only and equal_literals}, indent=2) + '\n')
    assert rows
    record = {'binary': name, 'artifacts': artifacts, 'selected': len(rows), 'same': sum(r['same'] for r in rows), 'rows': rows}
    (out / ('codegen-' + name + '.json')).write_text(json.dumps(record, indent=2) + '\n')
    summary.append({k:v for k,v in record.items() if k != 'rows'})
result = {'base_revision': '04f07602dba76eb6704b8901c989c9c9c1f8b42a', 'method': 'EX230 asmcmp normalizer; linked native release; no timing', 'results': summary}
(out / 'codegen.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps([{k:v for k,v in row.items() if k != 'rows'} for row in summary], indent=2))
