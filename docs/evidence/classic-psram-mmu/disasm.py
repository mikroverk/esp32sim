#!/usr/bin/env python3
"""Compare pre-LTO release classic-bus disassembly; not a CPU timing run."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import gzip

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--base', type=Path, required=True)
p.add_argument('--head', type=Path, required=True)
p.add_argument('--out', type=Path, required=True)
a = p.parse_args()
out = a.out.resolve()
out.mkdir(parents=True, exist_ok=True)
functions = ['tick', 'fetch', 'read8', 'read16', 'read32', 'write8', 'write16', 'write32', 'code_page', 'store']
collected = {}
for label, root in [('base', a.base.resolve()), ('head', a.head.resolve())]:
    env = os.environ.copy()
    target = out / (label + '-target')
    env.update(CARGO_TARGET_DIR=str(target), CARGO_BUILD_JOBS='2')
    subprocess.run(['nice', '-n', '10', 'cargo', '+1.99.0', 'rustc', '--release', '-p', 'esp32', '--lib', '--', '--emit=asm'],
                   cwd=root, env=env, check=True)
    asm = max((target / 'release/deps').glob('esp32-*.s'), key=lambda f: f.stat().st_mtime)
    obj = out / (label + '.o')
    subprocess.run(['xcrun', 'clang', '-c', '-x', 'assembler', str(asm), '-o', str(obj)], check=True)
    text = subprocess.run(['xcrun', 'llvm-objdump', '--disassemble', '--demangle', str(obj)], check=True, text=True, capture_output=True).stdout
    funcs = {}
    key = None
    for line in text.splitlines():
        m = re.match(r'^([0-9a-f]+) <(.+)>:$', line)
        if m:
            key = m[2]
            funcs[key] = []
        elif key and re.match(r'^\s+[0-9a-f]+:', line):
            funcs[key].append(line)
    selected = {}
    for name in functions:
        key = '<esp32::bus::SocBus>::store' if name == 'store' else '<esp32::bus::SocBus as emu_core::bus::Bus>::' + name
        selected[name] = funcs[key]
        # Only symbol + instructions, no source paths or debug metadata.
        with gzip.open(out / (label + '-' + name + '.disasm.gz'), 'wt') as stream:
            stream.write(key + '\n' + '\n'.join(funcs[key]) + '\n')
    collected[label] = selected
summary = []
for name in functions:
    base, head = collected['base'][name], collected['head'][name]
    def opcodes(lines):
        result = []
        for line in lines:
            match = re.match(r'^\s*[0-9a-f]+:\s*([0-9a-f]+)', line)
            assert match is not None
            result.append(match[1])
        return result
    summary.append(dict(function=name, base_instructions=len(base), head_instructions=len(head), identical_opcodes=opcodes(base) == opcodes(head)))
(out / 'disassembly.json').write_text(json.dumps(summary, indent=2) + '\n')
print(json.dumps(summary, indent=2))
