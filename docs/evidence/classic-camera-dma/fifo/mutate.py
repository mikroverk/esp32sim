#!/usr/bin/env python3
"""Semantic C5 mutations: require named assertion failures, never compilation errors."""
from pathlib import Path
import os, subprocess, json, re, sys
r=Path(sys.argv[1]).resolve(); out=Path(sys.argv[2]).resolve(); out.mkdir(parents=True,exist_ok=True)
p=r/'esp32/src/camera_fifo.rs'; original=p.read_text()
cases=[
 ('mode1 sample ordering','[sample, 0, first, 0]','[first, 0, sample, 0]'),
 ('mode3 sample lane','[0, 0, sample, 0]','[sample, 0, 0, 0]'),
 ('mode1 zero padding','[sample, 0, first, 0]','[sample, 1, first, 0]'),
 ('mode3 zero padding','[0, 0, sample, 0]','[1, 0, sample, 0]'),
 ('split carry','for &sample in bytes {','self.pending = None; for &sample in bytes {'),
 ('row tail emission','if let Some(first) = self.pending.take() {\n            out.extend_from_slice','if let Some(first) = self.pending.take().filter(|_| false) {\n            out.extend_from_slice'),
 ('row tail lane','[0, 0, first, 0]','[first, 0, 0, 0]'),
 ('row boundary consumes tail','if let Some(first) = self.pending.take() {\n            out.extend_from_slice','if let Some(first) = self.pending {\n            out.extend_from_slice'),
 ('reset discards tail','self.pending = None;','self.pending = Some(99);'),
 ('reject overlap mode zero','1 | 3 => Ok','0 | 1 | 3 => Ok'),
 ('reject mode two','1 | 3 => Ok','1..=3 => Ok'),
 ('mode3 differs from mode1','if self.mode == 3 {','if self.mode == 0 {'),
]
env=dict(os.environ,CARGO_TARGET_DIR=str(out/'mutation-target'))
cmd=['nice','-n','10','cargo','+1.99.0','test','--release','-p','esp32','--lib','camera_fifo::tests::']
rows=[]
try:
 for n,(name,before,after) in enumerate([('baseline',None,None)]+cases):
  if before: assert original.count(before)==1,(name,original.count(before))
  p.write_text(original.replace(before,after,1) if before else original)
  run=subprocess.run(cmd,cwd=r,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  (out/f'mutation-{n:02}.log').write_text(run.stdout)
  failed=re.findall(r'^test (\S+) \.\.\. FAILED',run.stdout,re.M)
  killed=run.returncode!=0 and bool(failed) and 'error[E' not in run.stdout and 'assertion' in run.stdout
  rows.append(dict(name=name,replace=before,with_text=after,exit=run.returncode,killed=killed,failed_tests=failed))
  (out/'mutations.json').write_text(json.dumps(rows,indent=2)+'\n')
  print(name,run.returncode,failed,flush=True)
  assert killed if before else run.returncode==0,run.stdout
finally: p.write_text(original)
