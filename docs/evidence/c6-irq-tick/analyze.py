#!/usr/bin/env python3
"""Recompute retained smoke correctness and medians without firmware/builds."""
import json, statistics
from pathlib import Path
from typing import Any
root=Path(__file__).resolve().parent
rows=json.loads((root/'smoke.json').read_text())
assert len(rows)==12
summary=[]
for case in ['hello-delay','public-display-starter']:
    group=[r for r in rows if r['case']==case];assert len(group)==6
    assert {r['variant'] for r in group}=={'base','candidate'}
    assert len(set((r['console_sha256'],r['retired_core0'],r['interrupts'],tuple(r['irq_lines'])) for r in group))==1
    assert all(r['exceptions']==0 and r['exit']==0 and r['builds_before']==0 and r['builds_after']==0 for r in group)
    s: dict[str, Any] = dict(case=case,retired_core0=group[0]['retired_core0'],console_sha256=group[0]['console_sha256'],load_1m_range=[min(r['load'][0] for r in group),max(r['load'][0] for r in group)])
    for metric in ['wall','user']:
        b=statistics.median(r[metric] for r in group if r['variant']=='base');c=statistics.median(r[metric] for r in group if r['variant']=='candidate')
        pairs=[]
        for pair in range(3):
            br=next(r for r in group if r['pair']==pair and r['variant']=='base');cr=next(r for r in group if r['pair']==pair and r['variant']=='candidate')
            pairs.append(100*(br[metric]-cr[metric])/br[metric])
        s[metric]=dict(base_median=b,candidate_median=c,reduction_percent=100*(b-c)/b,pair_reductions_percent=pairs)
    assert s['wall']['reduction_percent'] >= (10 if case=='hello-delay' else 0)
    assert s['user']['reduction_percent'] >= (10 if case=='hello-delay' else 0)
    summary.append(s)
print(json.dumps(summary,indent=2))
