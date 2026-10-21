#!/usr/bin/env python3
"""Explain, but do not normalize away, C7 native layout/codegen differences."""
import collections, json, re, runpy, sys, types
from pathlib import Path
root, raw, output = (Path(s).resolve() for s in sys.argv[1:4])
# Optional binary inputs: BASE CANDIDATE MAIN. Needed only for a zero-offset
# ADRP/LDR Q relocation that the inherited normalizer represents differently.
binaries = [Path(s).resolve() for s in sys.argv[4:]]
assert len(binaries) in (0, 3), 'optional inputs: BASE_BINARY CANDIDATE_BINARY MAIN_BINARY'
constant_load = runpy.run_path(str(root/'docs/evidence/classic-camera-dma/constant-load.py'))['constant_load']
m=types.ModuleType('asmcmp')
exec((root/'docs/evidence/classic-core/asmcmp.py').read_text().split('a, b = funcs(sys.argv[1])')[0],m.__dict__)
imm=re.compile(r'#-?0x[0-9a-f]+|#\d+')
rows=[]
for label in ['base','main']:
    d=raw/('codegen-'+label)
    a,b=m.funcs(d/'base.dis'),m.funcs(d/'candidate.dis')
    comparison=json.loads((d/'codegen.json').read_text())
    selected=comparison['rows']
    for row in selected:
        name=row['name']
        if row['same']: continue
        if name.endswith('>::tick'):
            old,new=a[name],b[name]
            hist=lambda xs: collections.Counter(i.split()[0] for i in xs)
            h0,h1=hist(old),hist(new)
            delta={k:h1[k]-h0[k] for k in sorted(set(h0)|set(h1)) if h0[k]!=h1[k]}
            windows=[]
            for index,line in enumerate(new):
                if 'camera_dma_data' in line:
                    windows.append(dict(index=index,instructions=new[max(0,index-35):index+12]))
            rows.append(dict(comparison=label,name=name,classification='active-DMA camera dispatch plus layout/stack/register allocation changes',instruction_delta=len(new)-len(old),opcode_delta=delta,camera_call_windows=windows))
            assert windows
            continue
        assert len(a[name])==len(b[name]),name
        changes=[]; constants=[]
        for index,(x,y) in enumerate(zip(a[name],b[name])):
            if x==y: continue
            # A zero page offset may be printed as [xN], not [xN,#PAGEOFF].
            # Accept only that spelling pair, with identical actual constant bytes.
            page=lambda s: re.sub(r', #PAGEOFF(?=\])', '', s)
            if page(x)==page(y) and re.fullmatch(r'ldr\s+q\d+, \[x\d+\]',page(x)):
                assert binaries, 'supply BASE CANDIDATE MAIN binaries for constant verification'
                base_binary=binaries[0 if label=='base' else 2]
                c0=constant_load(base_binary,d/'base.dis',name,index,comparison['artifacts'][0]['sha256'])
                c1=constant_load(binaries[1],d/'candidate.dis',name,index,comparison['artifacts'][1]['sha256'])
                assert c0['bytes_hex']==c1['bytes_hex'],(name,c0,c1)
                constants.append(dict(before=c0,after=c1,index=index))
                continue
            if imm.sub('#IMM',x)==imm.sub('#IMM',y):
                xx=[int(i[1:],0) for i in imm.findall(x)]; yy=[int(i[1:],0) for i in imm.findall(y)]
                assert all(v-u in [0,16] for u,v in zip(xx,yy)),(name,x,y)
                changes.append(dict(before=x,after=y))
            else:
                mi,mj=m.TGT.search(x),m.TGT.search(y)
                assert mi and mj and m.TGT.sub('',x)==m.TGT.sub('',y) and m.shape(a,mi.group(1))==m.shape(b,mj.group(1)),(name,x,y)
        rows.append(dict(comparison=label,name=name,classification='same operations/registers/order; +16-byte struct field displacements, equivalent call aliases and verified constant relocations',verified_constant_loads=constants,changed_field_operands=len(changes),examples=changes[:3]))
output.write_text(json.dumps(dict(method='Additional explanation only; original 10/21 equality score is not changed',rows=rows),indent=2)+'\n')
print('explained',len(rows),'function comparisons')
for row in rows:
    if 'camera_call_windows' in row: print(json.dumps(row,indent=2))
