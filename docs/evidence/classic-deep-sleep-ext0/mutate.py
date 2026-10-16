#!/usr/bin/env python3
"""Compile-valid removal mutations; run in an isolated candidate worktree.
Usage: CARGO_TARGET_DIR=... python3 mutate.py OUTPUT_DIR
Restores every source even after a failure. Full logs remain local in OUTPUT_DIR.
"""
import json, os
from pathlib import Path
import re, subprocess, sys
root=Path(__file__).resolve().parents[3]
out=Path(sys.argv[1]).resolve(); out.mkdir(parents=True,exist_ok=True)
sleep='esp32/src/sleep.rs'; soc='esp32/src/soc.rs'
cases=[
 ('late-irq-parking','esp-soc/src/machine.rs','            if self.bus.refresh_irq() { self.present_irqs(); }\n            S::peripheral_event(&self.bus, &mut self.cores);','            if self.bus.refresh_irq() { self.present_irqs(); }'),
 ('slow-force-pd',sleep,'pwc & (1 << 15) != 0 || ',''),
 ('fast-force-pd',sleep,'pwc & (1 << 12) != 0 || ',''),
 ('board-event','esp32/src/bus.rs','            self.periph.gpio.set_input(edge.pin, edge.level);\n            self.ext0_wake();','            self.periph.gpio.set_input(edge.pin, edge.level);'),
 ('release-event',soc,'self.periph.gpio.release_input(pin); self.ext0_wake();','self.periph.gpio.release_input(pin);'),
 ('rtc-selector','esp32/src/adc.rs','PADS.get(index)?','PADS.get(0)?'),
 ('nmi-mask',soc,'core.interrupt = 0;',''),
 ('entry',sleep,'self.deep_sleep = true;','self.deep_sleep = false;'),
 ('digital-power-config',sleep,'rtc.read(0x84) & (1 << 31) == 0','false'),
 ('timer-powerdown',sleep,'self.periph.timg = std::array::from_fn(crate::timers::ClassicTimer::new);',''),
 ('radio-powerdown',sleep,'self.periph.wifi.clock_enabled = false;','self.periph.wifi.clock_enabled = true;'),
 ('pwm-powerdown',sleep,'self.periph.ledc.clock_enabled = false;','self.periph.ledc.clock_enabled = true;'),
 ('rmt-powerdown',sleep,'self.periph.rmt.clock_enabled = false;','self.periph.rmt.clock_enabled = true;'),
 ('i2s-powerdown',sleep,'i2s.clock_enabled = false;','i2s.clock_enabled = true;'),
 ('slow-code-version',sleep,'self.written(0x5000_0000, self.rtc_slow.len());',''),
 ('fast-code-version',sleep,'self.written(0x400c_0000, self.rtc_fast.len());',''),
 ('slow-force-pu',sleep,'pwc & (1 << 16) == 0','true'),
 ('fast-force-pu',sleep,'pwc & (1 << 13) == 0','true'),
 ('slow-retention',sleep,'self.rtc_slow.fill(0);','self.rtc_slow.fill(0x55);'),
 ('fast-retention',sleep,'self.rtc_fast.fill(0);','self.rtc_fast.fill(0x55);'),
 ('wake-enable',sleep,'rtc.ram.read(0x34) & (1 << 11) == 0','false'),
 ('rtc-domain',sleep,'pwc & (1 << 18) != 0 || pwc & (1 << 20) != 0 && pwc & (1 << 19) == 0','false'),
 ('level',sleep,'!= high','== high'),
 ('reset-reason',sleep,'rtc.reset_cause = 5;','rtc.reset_cause = 3;'),
 ('wake-cause',sleep,'& !0x7ff) | 1','& !0x7ff) | 2'),
 ('reset-request',sleep,'rtc.sw_reset = true;','rtc.sw_reset = false;'),
 ('input-event',soc,'        self.periph.gpio.set_input(pin, level);\n        self.ext0_wake();','        self.periph.gpio.set_input(pin, level);'),
 ('cpu-parking',soc,'for core in cores {','for core in cores.iter_mut().take(0) {'),
 ('digital-irq-mask',soc,'core.intenable = 0;','core.intenable = u32::MAX;'),
 ('release-parking',soc,'if bus.deep_sleep {\n            c.waiting = true;','if false {\n            c.waiting = true;'),
 ('rtc-mux','esp32/src/adc.rs','(self.rtc_io.read(off) & (1 << mux) != 0).then_some(pin)','{ let _ = (off, mux); Some(pin) }'),
 ('readonly-cause','esp32/src/periph.rs','(v & (0xfff << 11)) | (self.0.ram.read(0x34) & 0x7ff)','v & (0xfff << 11)'),
]
rows=[]
env=os.environ.copy(); env['CARGO_BUILD_JOBS']='2'
for name,path,old,new in cases:
 p=root/path; original=p.read_text(); assert original.count(old)==1,(name,original.count(old))
 try:
  p.write_text(original.replace(old,new))
  r=subprocess.run(['nice','-n','10','cargo','+1.99.0','test','--release','-p','esp32','--lib','--test','deep_sleep'],cwd=root,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  text=r.stdout.decode(); (out/(name+'.log')).write_text(text)
  failed=re.findall(r'^test (\S+) \.\.\. FAILED$',text,re.M)
  row=dict(mutation=name,compiled='Finished `release`' in text,killed=r.returncode!=0 and bool(failed),failed_tests=failed)
  rows.append(row); (out/'mutations.json').write_text(json.dumps(rows,indent=2)+'\n')
  assert row['compiled'] and row['killed'],row
 finally: p.write_text(original)
print(json.dumps(rows,indent=2))
