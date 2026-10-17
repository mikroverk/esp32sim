#!/usr/bin/env python3
"""Retain hot-function disassembly from two normally linked release CLI binaries."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import re
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--base', type=Path, required=True)
p.add_argument('--head', type=Path, required=True)
p.add_argument('--out', type=Path, required=True)
a = p.parse_args()
out = a.out.resolve()
out.mkdir(parents=True, exist_ok=True)
functions = ['tick', 'fetch', 'read8', 'read16', 'read32', 'store']
collected = {}
inputs = {}
for label, exe in [('base', a.base.resolve()), ('head', a.head.resolve())]:
    inputs[label] = hashlib.sha256(exe.read_bytes()).hexdigest()
    text = subprocess.run(['xcrun', 'llvm-objdump', '--disassemble', '--demangle', str(exe)],
                          check=True, text=True, capture_output=True).stdout
    funcs = {}
    key = None
    for line in text.splitlines():
        m = re.match(r'^([0-9a-f]+) <(.+)>:$', line)
        if m:
            key = m[2]
            funcs[key] = []
        elif key and re.match(r'^\s*[0-9a-f]+:', line):
            funcs[key].append(line)
    selected = {}
    for name in functions:
        key = '<esp32::bus::SocBus>::store' if name == 'store' else '<esp32::bus::SocBus as emu_core::bus::Bus>::' + name
        selected[name] = funcs[key]
        assert selected[name], key
        with gzip.open(out / (label + '-' + name + '.disasm.gz'), 'wt') as stream:
            stream.write(key + '\n' + '\n'.join(funcs[key]) + '\n')
    collected[label] = selected
summary = []
for name in functions:
    b, h = collected['base'][name], collected['head'][name]
    def mnemonics(lines):
        return [re.sub(r'^\s*[0-9a-f]+:\s*[0-9a-f]+\s+', '', l).split()[0] for l in lines]
    summary.append(dict(function=name, base_instructions=len(b), head_instructions=len(h),
                        same_mnemonic_sequence=mnemonics(b) == mnemonics(h)))
(out / 'linked.json').write_text(json.dumps(dict(binary_sha256=inputs, functions=summary), indent=2) + '\n')
print(json.dumps(summary, indent=2))
