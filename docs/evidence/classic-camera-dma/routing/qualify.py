#!/usr/bin/env python3
"""Required native/WASM checks in an isolated output and empty HOME."""
import json, os, subprocess, sys
from pathlib import Path
root, out = (Path(s).resolve() for s in sys.argv[1:3])
out.mkdir(parents=True, exist_ok=True)
env = os.environ.copy()
env.update(CARGO_TARGET_DIR=str(out/'target'), RUSTUP_TOOLCHAIN='1.99.0', RUSTUP_HOME=str(Path.home()/'.rustup'), CARGO_HOME=str(Path.home()/'.cargo'), HOME=str(out/'empty-home'))
Path(env['HOME']).mkdir(exist_ok=True)
if not (root/'target').exists(): (root/'target').symlink_to(out/'target', target_is_directory=True)
commands = [
 ('clippy-native',['cargo','+1.99.0','clippy','--workspace','--all-targets','--','-D','warnings']),
 ('clippy-wasm',['cargo','+1.99.0','clippy','--release','--target','wasm32-unknown-unknown','-p','esp32sim-wasm','--features','jit-tests','--','-D','warnings']),
 ('rom-tests',['cargo','+1.99.0','test','--release','--workspace','--','--include-ignored','--skip','external_']),
 ('plain-tests',['cargo','+1.99.0','test','--release','--workspace']),
 ('wasm-build',['tools/wasm-build.sh']),
 ('wasm-demos',['node','tools/wasm-test.mjs','hello','c3-hello','c6-hello','c6-energy-scan','c6-contiki','c6-contiki-net','c6-rpl-net','panel']),
 ('privacy',['node','tools/check-evidence-privacy.mjs']),
 ('build-bins',['cargo','+1.99.0','build','--release','--bins']),
]
results = []
for name, cmd in commands:
 e = env.copy(); e.pop('ESP32SIM_ROM_DIR',None)
 if name == 'rom-tests': e['ESP32SIM_ROM_DIR'] = str(root/'web/wasm/fw')
 with (out/(name+'.log')).open('w') as log:
  run = subprocess.run(['nice','-n','10']+cmd,cwd=root,env=e,stdout=log,stderr=subprocess.STDOUT)
 results.append(dict(name=name,command=cmd,returncode=run.returncode,empty_home=True,rom_enabled=name=='rom-tests'))
 (out/'checks.json').write_text(json.dumps(results,indent=2)+'\n')
 print(name,run.returncode,flush=True)
 if run.returncode: sys.exit(run.returncode)
