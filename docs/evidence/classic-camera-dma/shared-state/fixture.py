#!/usr/bin/env python3
"""Run the existing public OV5640 fixture on base and candidate; no timing."""
from pathlib import Path
import subprocess, json, hashlib, os, re, sys
r=Path(sys.argv[1]).resolve()
e=Path(sys.argv[2]).resolve(); e.mkdir(parents=True,exist_ok=True)
fw=Path(sys.argv[3]).resolve()
inputs=json.loads((r/'docs/evidence/camera-start-2026-10-02/inputs.json').read_text())
manifest=[]
for row in inputs['flash']['files']:
 if row['filename']=='boot_app0.bin':continue
 p=fw/row['filename']; h=hashlib.sha256(p.read_bytes()).hexdigest()
 assert h==row['sha256'], (p.name,h)
 manifest.append(dict(filename=p.name,sha256=h,bytes=p.stat().st_size))
rom=r/'web/wasm/fw/esp32s3_rev0_rom.elf'
assert hashlib.sha256(rom.read_bytes()).hexdigest()==inputs['rom_sha256']
results={}
for label,target in [('base','base-target'),('candidate','target')]:
 cmd=['nice','-n','10','python3',str(r/'docs/evidence/camera-start-2026-10-02/external_camera.py'),'--emulator',str(e/target/'release/esp32sim'),'--firmware',str(fw),'--rom',str(rom),'--output',str(e/('fixture-'+label)),'--expect','frames']
 subprocess.run(cmd,check=True)
 results[label]=json.loads((e/('fixture-'+label)/'results.json').read_text())
assert results['base']['stdout_sha256']==results['candidate']['stdout_sha256']
assert results['base']['serial']==results['candidate']['serial']
normalized=[]
for label in ['base','candidate']:
 text=(e/('fixture-'+label)/'stderr.log').read_text()
 text=text.replace(str(e/('fixture-'+label)/'red.ppm'), '<image>').replace(str(rom), '<rom>')
 text=re.sub(r'in [0-9.]+s wall = [0-9.]+ Minsn/s;', 'in <wall>;', text)
 normalized.append(text)
assert normalized[0]==normalized[1]
results.update(firmware=manifest,rom_sha256=inputs['rom_sha256'],console_equal=True,stderr_normalized_equal=True,stderr_normalization='image/ROM paths and wall time/throughput only; instruction/cycle/exception/interrupt counts preserved',stderr_normalized_sha256=hashlib.sha256(normalized[0].encode()).hexdigest(),frame_lines_equal=True)
(e/'fixture-comparison.json').write_text(json.dumps(results,indent=2)+'\n')
print('base/candidate console, normalized stderr and frame receipts identical')
