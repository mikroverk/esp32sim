#!/usr/bin/env python3
"""C7 semantic mutations: require assertion/test failure, never compile failure."""
import json, os, re, subprocess, sys
from pathlib import Path
root, out = (Path(p).resolve() for p in sys.argv[1:3])
out.mkdir(parents=True, exist_ok=True)
cam='esp32/src/camera_dma.rs'; i2s='esp32/src/i2s.rs'; dma='esp-periph/src/gdma/receive.rs'; bus='esp32/src/bus.rs'
cases=[
 ('peripheral reset preserves stale frame','esp32/src/periph.rs','self.i2s[port] = crate::i2s::ClassicI2s::for_port(port);','let _ = port;'),
 ('camera-free board polling',i2s,'let mut bank = self.periph.i2s[0].inner.take_pcm_bank();','let mut bank = self.periph.i2s[0].inner.take_pcm_bank(); let _ = self.board.camera_dvp_pins();'),
 ('camera dispatch cache missing',i2s,'self.camera_mode = self.camera_capable && value & 0x21 == 0x21','self.camera_mode = false'),
 ('transport disabled',cam,'let data = capture.packed[capture.pos..end].to_vec();','let data = Vec::new();'),
 ('ignore VSYNC arm',cam,'let Some(at) = edge.filter(', 'let Some(at) = Some(0).or(edge).filter('),
 ('charge pulse as pixels',cam,'self.cycles.saturating_sub(capture.frame_at?)','self.cycles.saturating_sub(capture.frame_at?).saturating_add(VSYNC_CYCLES)'),
 ('lose fractional word position',cam,'self.cycles.saturating_sub(capture.frame_at?)','_cycles'),
 ('wrong frame rate',cam,'u128::from(FRAME_CYCLES - VSYNC_CYCLES)','u128::from(FRAME_CYCLES)'),
 ('do not consume byte position',cam,'capture.pos = end;','capture.pos = 0;'),
 ('drop final boundary word',cam,'let tail = capture.packed[capture.pos..].to_vec();','let tail = Vec::new();'),
 ('wrong FIFO mode',cam,'CameraFifo::new(rx.fifo_mode)','CameraFifo::new(3)'),
 # Cancellation now has configuration and tick callers; mutate their shared
 # terminal operation rather than only the redundant late-tick fallback.
 ('route loss not terminal',i2s,'self.dma.fail_receive(&mut irq);','let _ = &mut irq;'),
 ('RX resets keep DMA running',i2s,'self.dma.running = false;\n                self.dma.buf_pos = 0;','self.dma.running = true;\n                self.dma.buf_pos = 0;'),
 ('START command retained in readback',i2s,'self.ram.write(off, value & !(1 << 29));','self.ram.write(off, value);'),
 ('inlink stop ignored',i2s,'if value & (1 << 28) != 0 { self.dma.running = false; }','if value & (1 << 28) != 0 { self.dma.running = true; }'),
 ('wrong EOF count unit',i2s,'ram.read(0x24).checked_mul(4)','ram.read(0x24).checked_mul(1)'),
 ('wrong EOF descriptor register',i2s,'self.ram.write(0x3c, eof_desc)','self.ram.write(0x3c, 0)'),
 ('wrong classic IRQ mapping',i2s,'((gdma & 3) << 8)','((gdma & 3) << 7)'),
 ('owner check bypassed',dma,'r.conf1 & (1 << 12) != 0 && !d.owner_dma','false && !d.owner_dma'),
 ('owner not returned',dma,'(dw0 & !(0xfff << 12) & !(3 << 30))','(dw0 & !(0xfff << 12) & !(1 << 30))'),
 ('length not written',dma,'(self.buf_pos << 12)','(0 << 12)'),
 ('EOF flag not written',dma,'if eof { 1 << 30 } else { 0 }','if eof { 0 } else { 0 }'),
 ('wrong descriptor advance',dma,'self.desc = next; self.buf_pos = 0;','self.desc = 0; self.buf_pos = 0;'),
 ('DRAM rule admits CPU-only prefix',bus,'!(DRAM_LOW..DRAM_HIGH).contains(&addr)','!(ROM_DRAM_LOW..DRAM_HIGH).contains(&addr)'),
 ('mode1 sensor sample ordering','esp32/src/camera_fifo.rs','[sample, 0, first, 0]','[first, 0, sample, 0]'),
 ('mode3 sensor sample lane','esp32/src/camera_fifo.rs','[0, 0, sample, 0]','[sample, 0, 0, 0]'),
]
original={f:(root/f).read_text() for _,f,_,_ in cases}
env=dict(os.environ)
env.setdefault('CARGO_TARGET_DIR',str(out/'target'))
cmd=['nice','-n','10','cargo','+1.99.0','test','--release','-p','esp32','--test','aithinker','c7::','--','--nocapture']
rows=[]
try:
 for name,file,before,after in [('baseline',cam,None,None)]+cases:
  if before:
   assert after is not None
   assert original[file].count(before)==1,(name,original[file].count(before))
   (root/file).write_text(original[file].replace(before,after,1))
  run=subprocess.run(cmd,cwd=root,env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  failed=re.findall(r'^test (\S+) \.\.\. FAILED',run.stdout,re.M)
  killed=run.returncode!=0 and bool(failed) and 'error[E' not in run.stdout and 'assertion' in run.stdout
  rows.append(dict(name=name,file=file,before=before,after=after,exit=run.returncode,killed=killed,failed_tests=failed))
  (out/f'{len(rows)-1:02}.log').write_text(run.stdout)
  (out/'mutations.json').write_text(json.dumps(rows,indent=2)+'\n')
  print(name,run.returncode,failed,flush=True)
  (root/file).write_text(original[file])
  assert killed if before else run.returncode==0,run.stdout
finally:
 for file,text in original.items(): (root/file).write_text(text)
