#!/usr/bin/env python3
"""Replay the two moved historical sensor mutations and extraction regressions."""
from pathlib import Path
import os,subprocess,json,re,sys
r=Path(sys.argv[1]).resolve(); e=Path(sys.argv[2]).resolve(); e.mkdir(parents=True,exist_ok=True)
path=r/'esp-soc/src/camera.rs'; original=path.read_text()
old=json.loads((r/'docs/evidence/camera-live-2026-10-05/mutations.json').read_text())
cases=[(c['mutation'],c['replace_first'],c['with_text']) for c in old['cases'] if c['mutation'] in ['SCCB reset','window sync']]
cases += [
 ('config cache invalidation','self.cached.as_ref().is_none_or(|(old, _)| *old != config)','self.cached.is_none()'),
 ('picture cache invalidation','self.picture = Some(p); self.invalidate();','self.picture = Some(p);'),
 ('stream gating','w == 0 || h == 0 || !config.streaming','w == 0 || h == 0'),
 ('JPEG rejection','match (config.jpeg, config.format)','match (false, config.format)'),
 ('RGB565 conversion','Some(picture::PixelFormat::Rgb565)','Some(picture::PixelFormat::Yuyv)'),
 ('YUYV conversion','Some(picture::PixelFormat::Yuyv)','Some(picture::PixelFormat::Rgb565)'),
 ('grayscale conversion','Some(picture::PixelFormat::Grayscale)','Some(picture::PixelFormat::Rgb565)'),
 ('black fallback','rgb: vec![0; 3]','rgb: vec![255; 3]'),
 ('preview source','out.extend_from_slice(&p.rgb[o..o + 3]);','out.extend_from_slice(&[0; 3]); let _ = o;'),
 ]
env=dict(os.environ,CARGO_TARGET_DIR=str(e/'mutation-target'))
cmd=['nice','-n','10','cargo','+1.99.0','test','-p','esp-soc','-p','esp32s3','--lib','camera']
rows=[]
try:
 for n,(name,before,after) in enumerate([('baseline',None,None)]+cases):
  if before:
   assert before in original,name
   path.write_text(original.replace(before,str(after),1))
  else:path.write_text(original)
  p=subprocess.run(cmd,cwd=r,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  (e/f'mutation-{n:02}.log').write_text(p.stdout)
  failed=re.findall(r'^test (\S+) \.\.\. FAILED',p.stdout,re.M)
  killed=p.returncode!=0 and bool(failed) and 'error[E' not in p.stdout
  rows.append(dict(name=name,exit=p.returncode,killed=killed,failed_tests=failed))
  (e/'mutations.json').write_text(json.dumps(rows,indent=2)+'\n')
  print(name,p.returncode,failed,flush=True)
  if name=='baseline':assert p.returncode==0,p.stdout
  else:assert killed,p.stdout
  path.write_text(original)
finally:path.write_text(original)
