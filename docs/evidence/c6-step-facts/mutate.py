#!/usr/bin/env python3
"""Kill codegen regressions and runtime contract mutants. OUTPUT BASE_BINARY.

Run in a disposable worktree with CARGO_TARGET_DIR set. Restores source in finally.
Codegen mutants are killed by the emitted-code assertion, not by semantics tests:
removing an inline request intentionally leaves guest behavior unchanged.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
repo=Path(__file__).resolve().parents[3]
out=Path(sys.argv[1]).resolve()
base=Path(sys.argv[2]).resolve()
assert Path.cwd().resolve()==repo
assert os.environ.get('CARGO_TARGET_DIR'), 'set a dedicated CARGO_TARGET_DIR'
out.mkdir(parents=True,exist_ok=True)
p=repo/'riscv-rv32/src/exec.rs'
original=p.read_text()
rows=[]
mutations=[
    ('outline-step-facts','#[inline(always)]\npub fn step_outcome','pub fn step_outcome','codegen'),
    ('outline-execution','#[inline(always)]\npub fn exec_insn','#[inline]\npub fn exec_insn','codegen'),
    ('drop-retirement','cpu.retired_count += 1; StepKind::Retired','StepKind::Retired','runtime'),
    ('double-cycles','cpu.insn_count += 1; cpu.cycle_count += 1;','cpu.insn_count += 1; cpu.cycle_count += 2;','runtime'),
    ('drop-observer-fetch','bytes: Some(bytes), length: i.len','bytes: None, length: i.len','runtime'),
]
try:
    for name,old,new,kind in mutations:
        assert old in original,name
        p.write_text(original.replace(old,new))
        log=out/(name+'.log')
        if kind=='codegen':
            cmd=['cargo','+1.99.0','build','--release','-p','esp32sim','--bin','esp32sim-c6']
        else:
            selection=['--lib','step_facts_keep_compressed_and_full_fetch_windows'] if name=='drop-observer-fetch' else ['--test','result_only']
            cmd=['cargo','+1.99.0','test','--release','-p','riscv-rv32']+selection
        with log.open('w') as stream:
            r=subprocess.run(['nice','-n','10']+cmd,cwd=repo,stdout=stream,stderr=subprocess.STDOUT)
        if kind=='codegen':
            assert r.returncode==0, 'mutant failed compilation'
            binary=out/(name+'-c6')
            shutil.copy2(Path(os.environ['CARGO_TARGET_DIR'])/'release/esp32sim-c6',binary)
            with log.open('a') as stream:
                r=subprocess.run(['nice','-n','10','python3',str(Path(__file__).with_name('codegen.py')),str(base),str(binary),str(out/name)],cwd=repo,stdout=stream,stderr=subprocess.STDOUT)
            killed=r.returncode!=0 and 'result-only path must inline' in log.read_text()
        else:
            killed=r.returncode!=0 and 'test result: FAILED' in log.read_text() and 'could not compile' not in log.read_text()
        rows.append({'mutation':name,'oracle':kind,'killed':killed,'exit':r.returncode})
        (out/'mutations.json').write_text(json.dumps(rows,indent=2)+'\n')
        print(json.dumps(rows[-1]),flush=True)
        assert killed,name
finally:
    p.write_text(original)
