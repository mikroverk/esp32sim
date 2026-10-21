#!/usr/bin/env python3
"""Curate compact receipts from actual check logs; no raw captures in Git."""
import hashlib, json, re, subprocess, sys
from pathlib import Path
root, out = (Path(x).resolve() for x in sys.argv[1:3])
evidence=root/'docs/evidence/classic-camera-dma/board'
def save(name, value):
 (evidence/name).write_text(json.dumps(value,indent=2)+'\n')
checks=json.loads((out/'checks.json').read_text())
for row in checks:
 row['command']=[part.replace(str(out),'<output>').replace(str(root),'<tree>') for part in row['command']]
assert len(checks)==9 and all(row['returncode']==0 for row in checks)
save('checks.json',checks)
totals={}
for name in ['rom-tests','plain-tests']:
 text=(out/(name+'.log')).read_text()
 rows=re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out',text)
 assert rows
 totals[name]=dict(zip(['passed','failed','ignored','measured','filtered'],[sum(int(row[i]) for row in rows) for i in range(5)]))
save('test-totals.json',totals)
mutations=json.loads((out/'mutations/results.json').read_text())
assert all(row['killed'] for row in mutations)
save('mutations.json',mutations)
save('s3-mutations.json',json.loads((out/'s3-mutations/mutations.json').read_text()))
save('codegen.json',json.loads((out/'codegen/codegen.json').read_text()))
save('fixture-comparison.json',json.loads((out/'fixture-comparison.json').read_text()))
save('sources.json',json.loads((out/'sources/sources.json').read_text()))
base='935da1221bce40e60581895a9e11f0f268c122ad'
files=subprocess.check_output(['git','ls-files','tests/golden'],cwd=root,text=True).splitlines()
rows=[]
for f in files:
 before=subprocess.check_output(['git','show',base+':'+f],cwd=root)
 after=(root/f).read_bytes(); assert before==after,f
 rows.append(dict(file=f,sha256=hashlib.sha256(after).hexdigest()))
save('goldens.json',dict(base=base,unchanged=len(rows),files=rows))
print(json.dumps(dict(totals=totals,mutations=len(mutations),hot_functions=len(json.loads((out/'codegen/codegen.json').read_text())['rows']),unchanged_golden_files=len(rows)),indent=2))
