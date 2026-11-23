import os,subprocess,json
from pathlib import Path
repo=Path(os.environ['TREE']); n=Path(os.environ['OUT']); p=repo/'riscv-rv32/src/exec.rs'; orig=p.read_text(); old='#[inline(always)]\npub fn step_outcome'; assert orig.count(old)==1
try:
 p.write_text(orig.replace(old,'pub fn step_outcome'))
 cmd=['nice','-n','10','cargo','+1.99.0','build','--release','--bins']
 with (n/'outline-build.log').open('w') as f: b=subprocess.run(cmd,cwd=repo,stdout=f,stderr=subprocess.STDOUT)
 assert b.returncode==0,'mutant compilation failed'
 cmd=['nice','-n','10','python3',str(repo/'docs/evidence/c6-step-facts/codegen.py'),str(n/'target-base/release/esp32sim-c6'),str(Path(os.environ['CARGO_TARGET_DIR'])/'release/esp32sim-c6'),str(n/'outline-codegen')]
 with (n/'outline-codegen.log').open('w') as f: r=subprocess.run(cmd,cwd=repo,stdout=f,stderr=subprocess.STDOUT)
 killed=r.returncode!=0 and 'result-only path must inline' in (n/'outline-codegen.log').read_text()
 (n/'outline-result.json').write_text(json.dumps({'mutation':'outline-step-facts','killed':killed,'exit':r.returncode},indent=2)+'\n'); print('killed',killed,flush=True); assert killed
finally:p.write_text(orig)
