#!/usr/bin/env python3
"""Compare named WASM function instruction bodies (not module offsets)."""
from pathlib import Path
import hashlib,json,sys
class Reader:
 def __init__(self,b):self.b=b;self.p=0
 def u(self):
  n=s=0
  while True:
   b=self.b[self.p];self.p+=1;n|=(b&127)<<s
   if b<128:return n
   s+=7
 def take(self,n):
  b=self.b[self.p:self.p+n];self.p+=n;return b
 def string(self):return self.take(self.u()).decode()
def parse(path):
 b=Path(path).read_bytes();r=Reader(b);r.p=8
 names={};bodies=[];imports=0
 while r.p<len(b):
  id=r.take(1)[0];p=Reader(r.take(r.u()))
  if id==0 and p.string()=='name':
   while p.p<len(p.b):
    sub=p.take(1)[0];q=Reader(p.take(p.u()))
    if sub==1:
     for _ in range(q.u()):
      i=q.u();names[i]=q.string()
  elif id==2:
   for _ in range(p.u()):
    p.string();p.string();kind=p.take(1)[0]
    if kind==0:p.u();imports+=1
    elif kind in (1,2):
     if kind==1:p.take(1)
     flags=p.u();p.u()
     if flags&1:p.u()
    elif kind==3:p.take(2)
    elif kind==4:p.u();p.u()
    else:raise ValueError(kind)
  elif id==10:
   bodies=[p.take(p.u()) for _ in range(p.u())]
 return {names.get(i+imports,str(i+imports)):v for i,v in enumerate(bodies)},hashlib.sha256(b).hexdigest()
a,ha=parse(sys.argv[1]);b,hb=parse(sys.argv[2]);common=set(a)&set(b)
changed=[{'function':n,'base_bytes':len(a[n]),'fixed_bytes':len(b[n]),'base_sha256':hashlib.sha256(a[n]).hexdigest(),'fixed_sha256':hashlib.sha256(b[n]).hexdigest()} for n in sorted(common) if a[n]!=b[n]]
result=dict(base_wasm_sha256=ha,fixed_wasm_sha256=hb,base_functions=len(a),fixed_functions=len(b),common=len(common),identical=sum(a[n]==b[n] for n in common),changed=changed,added=sorted(set(b)-set(a)),removed=sorted(set(a)-set(b)))
Path(sys.argv[3]).write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
