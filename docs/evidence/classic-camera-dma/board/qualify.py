#!/usr/bin/env python3
import json, os, subprocess, sys
from pathlib import Path
root = Path(sys.argv[1]).resolve(); out = Path(sys.argv[2]).resolve()
out.mkdir(parents=True, exist_ok=True)
env = os.environ.copy()
env['CARGO_TARGET_DIR'] = str(out/'target')
env['RUSTUP_TOOLCHAIN'] = '1.99.0'
env['RUSTUP_HOME'] = str(Path.home()/'.rustup')
env['CARGO_HOME'] = str(Path.home()/'.cargo')
env['HOME'] = str(out/'empty-home'); Path(env['HOME']).mkdir(exist_ok=True)
# scripts copy from the conventional target path even with CARGO_TARGET_DIR.
link = root/'target'
if not link.exists(): link.symlink_to(out/'target', target_is_directory=True)
commands = [
 ('mutations', ['python3','docs/evidence/classic-camera-dma/board/mutations.py','.',str(out/'mutations')], {}),
 ('clippy-native', ['cargo','+1.99.0','clippy','--workspace','--all-targets','--','-D','warnings'], {}),
 ('clippy-wasm', ['cargo','+1.99.0','clippy','--release','--target','wasm32-unknown-unknown','-p','esp32sim-wasm','--features','jit-tests','--','-D','warnings'], {}),
 ('rom-tests', ['cargo','+1.99.0','test','--release','--workspace','--','--include-ignored','--skip','external_'], {'ESP32SIM_ROM_DIR':str(root/'web/wasm/fw')}),
 ('plain-tests', ['cargo','+1.99.0','test','--release','--workspace'], {}),
 ('wasm-build', ['tools/wasm-build.sh'], {}),
 ('wasm-demos', ['node','tools/wasm-test.mjs','hello','c3-hello','c6-hello','c6-energy-scan','c6-contiki','c6-contiki-net','c6-rpl-net','panel'], {}),
 ('privacy', ['node','tools/check-evidence-privacy.mjs'], {}),
 ('build-bins', ['cargo','+1.99.0','build','--release','--bins'], {}),
]
results=json.loads((out/'checks.json').read_text()) if len(sys.argv) > 3 else []
for name,cmd,extra in commands:
 if len(sys.argv) > 3 and name not in sys.argv[3:]: continue
 results=[row for row in results if row['name'] != name]
 e=env.copy(); e.pop('ESP32SIM_ROM_DIR',None); e.update(extra)
 with (out/(name+'.log')).open('w') as f:
  run=subprocess.run(['nice','-n','10']+cmd,cwd=root,env=e,stdout=f,stderr=subprocess.STDOUT)
 results.append(dict(name=name,command=cmd,returncode=run.returncode,empty_home=True,rom_enabled=bool(extra)))
 (out/'checks.json').write_text(json.dumps(results,indent=2)+'\n')
 print(name,run.returncode,flush=True)
 if run.returncode:
  print((out/(name+'.log')).read_text()[-6000:]); sys.exit(run.returncode)
