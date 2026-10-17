#!/usr/bin/env python3
"""Run rule-removal checks. Set CARGO_TARGET_DIR outside the checkout."""
import json, os, subprocess, sys
from pathlib import Path
root=Path(__file__).resolve().parents[3]
out=Path(sys.argv[1]);out.mkdir(parents=True,exist_ok=True)
bus=root/'esp32c6/src/bus.rs'
soc=root/'esp32c6/src/soc.rs'
mutations=[
 ('drop-clocked-source-change',bus,'self.irq_dirty |= esp_periph::Dispatch::tick(&mut self.periph, cycles as u64);','let _ = esp_periph::Dispatch::tick(&mut self.periph, cycles as u64);','sleeping_core_wakes_when_the_timer_source_changes'),
 ('remove-line-refresh',soc,'fn refresh_irq(&mut self) -> bool { self.periph.refresh_lines(); true }','fn refresh_irq(&mut self) -> bool { true }','sleeping_core_wakes_when_the_timer_source_changes'),
 ('restore-unconditional-refresh',bus,'u32::from(self.irq_dirty)','1','quiet_tick_does_not_request_an_interrupt_refresh'),
 ('drop-write-invalidation',bus,'        self.irq_dirty = true;\n    }\n\n    /// SHA DMA','        // mutation: no invalidation\n    }\n\n    /// SHA DMA','timer_tick_requests_refresh_at_assertion_not_on_unchanged_level'),
]
rows=[]
for name,path,old,new,test in mutations:
    original=path.read_text();assert original.count(old)==1
    try:
        path.write_text(original.replace(old,new))
        cmd=['nice','-n','10','cargo','+1.99.0','test','--release','-p','esp32c6','--test','irq_tick',test,'--','--exact']
        p=subprocess.run(cmd,cwd=root,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
        text=p.stdout.decode();(out/(name+'.log')).write_text(text)
        killed=p.returncode!=0 and 'test result: FAILED' in text and f'{test} ... FAILED' in text
        rows.append(dict(mutation=name,test=test,exit=p.returncode,killed=killed))
        print(rows[-1],flush=True)
    finally:path.write_text(original)
(out/'mutations.json').write_text(json.dumps(rows,indent=2)+'\n')
assert all(r['killed'] for r in rows)
