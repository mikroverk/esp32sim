#!/usr/bin/env python3
"""Run C4 semantic mutants; require an assertion failure, never a compile-error kill."""
from pathlib import Path
import os, subprocess, json, re, sys
r=Path(sys.argv[1]).resolve(); e=Path(sys.argv[2]).resolve(); e.mkdir(parents=True,exist_ok=True)
p=r/'esp32/src/i2s.rs'; original=p.read_text()
cases=[
 ('zero camera width','if !self.camera_capable ||','if (self.ram.read(0xb0) >> 18) & 63 == 0 || !self.camera_capable ||'),
 ('camera audio clock validation','if !self.camera_capable ||','if self.ram.read(0xac) & (1 << 21) != 0 || !self.camera_capable ||'),
 ('port identity','camera_capable: port == 0','camera_capable: port < 2'),
 ('LCD required','& 0x21 != 0x21','& 1 != 1'),
 ('CAMERA required','& 0x21 != 0x21','& 0x20 != 0x20'),
 ('CONF2 offset','self.ram.read(0xa8) &','self.ram.read(0xa4) &'),
 ('RX reset','conf & 0xa != 0','conf & 8 != 0'),
 ('RX FIFO reset','conf & 0xa != 0','conf & 2 != 0'),
 ('TX reset isolation','conf & 0xa != 0','conf & 0xf != 0'),
 ('LC input reset','self.ram.read(0x60) & 0xd','self.ram.read(0x60) & 0xc'),
 ('LC AHB FIFO reset','self.ram.read(0x60) & 0xd','self.ram.read(0x60) & 9'),
 ('LC AHB reset','self.ram.read(0x60) & 0xd','self.ram.read(0x60) & 5'),
 ('TX LC reset isolation','self.ram.read(0x60) & 0xd','self.ram.read(0x60) & 0xf'),
 ('start bit','conf & (1 << 5) != 0 && !reset','conf & (1 << 4) != 0 && !reset'),
 ('reset gates start','conf & (1 << 5) != 0 && !reset','conf & (1 << 5) != 0'),
 ('slave bit','slave: conf & (1 << 7)','slave: conf & (1 << 6)'),
 ('FIFO mode','((fifo >> 16) & 7)','((fifo >> 16) & 3)'),
 ('descriptor enable','descriptor_enabled: fifo & (1 << 12)','descriptor_enabled: fifo & (1 << 11)'),
 ('forced mode','fifo_mode_forced: fifo & (1 << 20)','fifo_mode_forced: fifo & (1 << 19)'),
 ('camera bypass','if let Some(camera) = self.camera_rx() {','if let Some(camera) = self.camera_rx().filter(|_| false) {'),
 ('audio rate disabled','Device::write(&mut self.inner, 0x30, 0);','Device::write(&mut self.inner, 0x30, 25 | (2 << 27) | (1 << 26));'),
 ('audio producer stopped','if camera.reset { 3 } else { 1 }','if camera.reset { 3 } else { 4 }'),
 ('mode write synchronizes','8 | 0x2c | 0xa8 | 0xac | 0xb0 =>','8 | 0x2c | 0xac | 0xb0 =>'),
]
if len(sys.argv) > 3:
 selected=set(sys.argv[3:]); cases=[c for c in cases if c[0] in selected]
 assert len(cases)==len(selected),'unknown mutation name'
env=dict(os.environ,CARGO_TARGET_DIR=str(e/'mutation-target'))
cmd=['nice','-n','10','cargo','+1.99.0','test','--release','-p','esp32','--lib','i2s::tests::camera_']
rows=[]
try:
 for n,(name,before,after) in enumerate([('baseline',None,None)]+cases):
  if before:assert original.count(before)==1,(name,original.count(before))
  p.write_text(original.replace(before,after,1) if before else original)
  result=subprocess.run(cmd,cwd=r,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  (e/f'mutation-{n:02}.log').write_text(result.stdout)
  failed=re.findall(r'^test (\S+) \.\.\. FAILED',result.stdout,re.M)
  killed=result.returncode!=0 and bool(failed) and 'error[E' not in result.stdout
  rows.append(dict(name=name,replace=before,with_text=after,exit=result.returncode,killed=killed,failed_tests=failed))
  (e/'mutations.json').write_text(json.dumps(rows,indent=2)+'\n')
  print(name,result.returncode,failed,flush=True)
  if before:assert killed,result.stdout
  else:assert result.returncode==0,result.stdout
  p.write_text(original)
finally:p.write_text(original)
