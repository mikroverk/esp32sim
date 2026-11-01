#!/usr/bin/env python3
"""Compile-valid removal mutations. Use an isolated worktree and private target directory.
Restores the profile source even on failure. Full logs stay in caller-supplied output.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
p = root / 'xtensa-lx7/src/lx106.rs'
original = p.read_text()
cases = []
for name, op in [('window', 'Entry'), ('window-return', 'Retw'), ('float', 'AddS'),
                 ('boolean', 'Andb'), ('loop', 'Loop'), ('divide', 'Quos'),
                 ('remainder', 'Remu'), ('multiply-high', 'Muluh'),
                 ('minmax', 'Min'), ('sign-extension', 'Sext'), ('clamp', 'Clamps'),
                 ('atomic', 'S32c1i'), ('release-sync', 'L32ai'), ('user-register', 'Rur')]:
    cases.append((name, 'Nop | NopN | Break', op + ' | Nop | NopN | Break'))
cases += [
    ('wsr-direction-gate', 'Wsr => writable_sr(i.imm as u32),', 'Wsr => valid_sr(i.imm as u32),'),
    ('xsr-direction-gate', 'Xsr => exchangeable_sr(i.imm as u32),', 'Xsr => valid_sr(i.imm as u32),'),
    ('readonly-directions', 'valid_sr(n) && !matches!(n, sr::CONFIGID0 | sr::CONFIGID1 | sr::DEBUGCAUSE | sr::PRID)', 'valid_sr(n)'),
    ('interrupt-exchange', 'readable_sr(n) && writable_sr(n) && n != sr::INTSET', 'readable_sr(n) && writable_sr(n)'),
    ('undefined-read-direction', 'valid_sr(n) && n != sr::INTCLEAR', 'valid_sr(n)'),
    ('bypass-execution-gate', 'if !supports(&i) {', 'if false {'),
    ('extra-comparators', '230..=238 | 240)', '230..=238 | 240..=242)'),
    ('window-register', 'matches!(n, 3 | 5 | 96', 'matches!(n, 3 | 5 | 72 | 96'),
    ('ps-window-bits', 'sr::PS => self.cpu.ps = v & 0x3f,', 'sr::PS => self.cpu.ps = v & 0x7ff3f,'),
    ('eps-window-bits', 'self.cpu.eps[(n - 192) as usize] = v & 0x3f,', 'self.cpu.eps[(n - 192) as usize] = v,'),
    ('absolute-literals', 'Op::L32r if self.litbase & 1 != 0', 'Op::L32r if false'),
    ('s3-kernel-vector', '{ 0x30 }', '{ 0x300 }'),
    ('s3-nmi-level', 'c.epc[3] = c.pc;', 'c.epc[7] = c.pc;'),
    ('s3-debug-vector', 'c.pc = c.vecbase + 0x10;', 'c.pc = c.vecbase + 0x280;'),
    ('nmi-masked', '(c.interrupt & NMI) | if', '(c.interrupt & c.intenable & NMI) | if'),
    ('timer-match-disabled', 'if self.cpu.ccompare[0].wrapping_sub(before).wrapping_sub(1) < cycles', 'if false'),
    ('intclear-levels', 'v & (SOFTWARE | EDGE)', 'v & (SOFTWARE | EDGE | LEVEL)'),
    ('level-input-mask', 'const LEVEL: u32 = 0x3f;', 'const LEVEL: u32 = 0x8fbe333f;'),
    ('alignment', 'if addr & align != 0 {', 'if false {'),
]
rows = []
for name, old, new in cases:
    assert original.count(old) == 1, (name, original.count(old))
    try:
        p.write_text(original.replace(old, new))
        r = subprocess.run(['nice', '-n', '10', 'cargo', '+1.99.0', 'test', '--release', '-p', 'xtensa-lx7', '--test', 'lx106'], cwd=root, env=dict(os.environ, CARGO_BUILD_JOBS='2'), stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        text = r.stdout.decode()
        (out / (name + '.log')).write_text(text)
        failed = re.findall(r'^test (\S+) \.\.\. FAILED$', text, re.M)
        row = dict(mutation=name, compiled='Finished `release`' in text, killed=r.returncode != 0 and bool(failed), failed_tests=failed)
        rows.append(row)
        (out / 'mutations.json').write_text(json.dumps(rows, indent=2) + '\n')
        print(name, row['compiled'], row['killed'], flush=True)
        assert row['compiled'] and row['killed'], row
    finally:
        p.write_text(original)
