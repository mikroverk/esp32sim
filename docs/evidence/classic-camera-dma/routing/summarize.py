#!/usr/bin/env python3
"""Curate real check/mutation logs and pin unchanged goldens."""
import hashlib, json, re, subprocess, sys
from pathlib import Path
root,out=(Path(s).resolve() for s in sys.argv[1:3])
evidence=root/'docs/evidence/classic-camera-dma/routing'
def save(name, value):
 text=json.dumps(value,indent=2)+'\n'
 assert len(text.encode())<50000,(name,len(text))
 (evidence/name).write_text(text)
def load(name): return json.loads((out/name).read_text())
checks=load('checks.json')
assert len(checks)==8 and all(row['returncode']==0 for row in checks)
for row in checks:
 row['command']=[x.replace(str(root),'<tree>').replace(str(out),'<output>') for x in row['command']]
save('checks.json',checks)
totals={}
for name in ['rom-tests','plain-tests']:
 rows=re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out',(out/(name+'.log')).read_text())
 assert rows
 totals[name]=dict(zip(['passed','failed','ignored','measured','filtered'],[sum(int(row[i]) for row in rows) for i in range(5)]))
save('test-totals.json',totals)
for source,dest in [('mutations/mutations.json','mutations.json'),('c3-mutations/results.json','c3-mutations.json')]:
 rows=load(source); assert rows and all(row['killed'] for row in rows)
 save(dest,rows)
c6=load('mutations/mutations.json')
table=['# C6 executable assertion mutations', '', '| Mutation | Test that fails |', '|---|---|']
for row in c6:
 table.append('| '+row['mutation']+' | `'+row['test']+'` |')
(evidence/'mutations.md').write_text('\n'.join(table)+'\n')
rows=load('c4-mutations/mutations.json')
assert rows[0]['name']=='baseline' and rows[0]['exit']==0 and all(row['killed'] for row in rows[1:])
save('c4-mutations.json',rows)
save('codegen.json',load('codegen/codegen.json'))
save('fixture-comparison.json',load('fixture-comparison.json'))
save('sources.json',load('sources/sources.json'))
base='de9f3978df0a25af2e4fbcafc337ca22c75146af'
files=subprocess.check_output(['git','ls-files','tests/golden'],cwd=root,text=True).splitlines()
rows=[]
for file in files:
 old=subprocess.check_output(['git','show',base+':'+file],cwd=root)
 new=(root/file).read_bytes(); assert old==new,file
 rows.append(dict(file=file,sha256=hashlib.sha256(new).hexdigest()))
save('goldens.json',dict(base=base,unchanged=len(rows),files=rows))
print(json.dumps(dict(totals=totals,goldens=len(rows),c6_mutations=len(load('mutations/mutations.json')),c3_mutations=len(load('c3-mutations/results.json')),c4_mutations=len(load('c4-mutations/mutations.json'))-1),indent=2))
