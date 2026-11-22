"""Classify every differing text symbol, base vs candidate, after codegen_nop.py normalization:
'renumbered' = only call targets that are local symbols with a different numeric suffix (.591 -> .593),
'pageoff' = a page-offset load whose offset is 0 on one side (masking artifact), possibly with renumbered calls, 'suffix-only' = the
symbol itself only changed its local numeric suffix, 'code' = anything else. Writes codegen-classes.json:
per binary the count of each class and the 'code' rows (the other classes are listed on stdout only).
usage: classify.py  (reads bins/base and bins/cand next to this script)"""
import sys, json, re, difflib
from pathlib import Path
sys.argv = sys.argv[:1]
D = Path(__file__).parent
sys.path.insert(0, str(D))
import cg_lib
def unsuffix(s): return re.sub(r'\.\d+(?=>|$)', '.N', s)
res = {}
for name in ('esp32sim', 'esp32sim-c3', 'esp32sim-c6', 'esp32sim-p4'):
    sb, sc = cg_lib.load(D / 'bins/base' / name), cg_lib.load(D / 'bins/cand' / name)
    ub = {unsuffix(k): v for k, v in sb.items()}; uc = {unsuffix(k): v for k, v in sc.items()}
    rows = {}
    for k in sorted(set(sb) | set(sc)):
        if sb.get(k) == sc.get(k): continue
        u = unsuffix(k)
        a, c = ub.get(u), uc.get(u)
        if a is not None and c is not None and [unsuffix(x) for x in a] == [unsuffix(x) for x in c]:
            cls = 'suffix-only' if (k in sb) != (k in sc) else 'renumbered'
        elif a is not None and c is not None and len(a) == len(c) and all(re.sub(r', #PAGEOFF\]', ']', x) == re.sub(r', #PAGEOFF\]', ']', y) for x, y in zip(map(unsuffix, a), map(unsuffix, c))):
            cls = 'pageoff'
        else:
            cls = 'code'
        rows[u] = {'class': cls, 'base_insns': len(a) if a else None, 'cand_insns': len(c) if c else None}
    code = {k: v for k, v in rows.items() if v['class'] == 'code'}
    counts = {c: sum(1 for v in rows.values() if v['class'] == c) for c in ('code', 'renumbered', 'suffix-only', 'pageoff')}
    res[name] = {'symbols': len(set(sb) | set(sc)), 'differing': counts, 'code': code}
    print(name, counts)
    for k, v in rows.items(): print('  ', v['class'], v['base_insns'], '->', v['cand_insns'], k[:150])
(D / 'codegen-classes.json').write_text(json.dumps(res, indent=1) + '\n')
