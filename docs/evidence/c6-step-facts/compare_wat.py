#!/usr/bin/env python3
"""Compare named wasm-dis bodies. This is a literal textual comparison, not semantic equivalence."""
import hashlib,json,re,sys
from pathlib import Path

def parse(path):
    lines=Path(path).read_text().splitlines(keepends=True)
    result={}; name=None; body=[]
    for line in lines:
        if line.startswith(' (func '):
            if name is not None:
                assert name not in result
                result[name]=''.join(body)
            name=line.split()[1]; body=[line]
        elif name is not None and line.startswith(' (') and not line.startswith('  '):
            assert name not in result
            result[name]=''.join(body); name=None; body=[]
        elif name is not None:
            body.append(line)
    if name is not None:
        assert name not in result
        result[name]=''.join(body)
    signatures={m.group(1):m.group(2) for line in lines if (m:=re.match(r' \(type (\$\d+) (.*)\)\s*$',line))}
    def normalize(body):
        return re.sub(r'\(type (\$\d+)\)',lambda m:'(type '+signatures[m.group(1)]+')',body)
    return {name:normalize(body) for name,body in result.items()}

a,b=map(parse,sys.argv[1:3])
rows=[]
for n in sorted(set(a)|set(b)):
    x,y=a.get(n),b.get(n)
    rows.append(dict(name=n,status=('added' if x is None else 'removed' if y is None else 'identical' if x==y else 'changed'),base_sha256=hashlib.sha256(x.encode()).hexdigest() if x else None,candidate_sha256=hashlib.sha256(y.encode()).hexdigest() if y else None))
r=dict(method='wasm-dis named function text with type indices resolved to exact signatures; other layout differences retained',rows=rows)
Path(sys.argv[3]).write_text(json.dumps(r,indent=2)+'\n')
from collections import Counter
print(dict(Counter(row['status'] for row in rows)))
print('step functions',sum('step_outcome' in n for n in a),sum('step_outcome' in n for n in b))
