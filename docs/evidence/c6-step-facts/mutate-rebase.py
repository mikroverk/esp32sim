import json,os,subprocess
from pathlib import Path
repo=Path(os.environ['TREE']); out=Path(os.environ['OUT']); out.mkdir(exist_ok=True)
p=repo/'riscv-rv32/src/exec.rs'; original=p.read_text(); rows=[]
cases=[('drop-retirement','cpu.retired_count += 1; crate::hwlp::retire(cpu, pc);','crate::hwlp::retire(cpu, pc);',['--test','result_only']),('double-cycles','    cpu.insn_count += 1; cpu.cycle_count += 1;\n    let control =','    cpu.insn_count += 1; cpu.cycle_count += 2;\n    let control =',['--test','result_only']),('drop-observer-fetch','bytes: Some(bytes), length: i.len','bytes: None, length: i.len',['--lib','step_facts_keep_compressed_and_full_fetch_windows'])]
try:
 for name,old,new,selection in cases:
  assert original.count(old)==1,(name,original.count(old))
  p.write_text(original.replace(old,new))
  cmd=['nice','-n','10','cargo','+1.99.0','test','--release','-p','riscv-rv32']+selection
  with (out/(name+'.log')).open('w') as f: r=subprocess.run(cmd,cwd=repo,stdout=f,stderr=subprocess.STDOUT)
  text=(out/(name+'.log')).read_text(); killed=r.returncode!=0 and 'test result: FAILED' in text and 'could not compile' not in text
  rows.append(dict(mutation=name,killed=killed,exit=r.returncode)); (out/'mutations.json').write_text(json.dumps(rows,indent=2)+'\n'); print(rows[-1],flush=True)
  assert killed
finally: p.write_text(original)
