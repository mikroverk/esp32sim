#!/usr/bin/env python3
"""Run against a complete emulator capture: python3 check_compare.py CAPTURE."""
import copy
import sys
from compare import compare, parse, summarize

rows=parse(sys.argv[1])
same=compare(rows,rows)
assert not same['differences']
assert all(c['ok'] for c in same['emulator']['checks'])
changed=copy.deepcopy(rows)
r=next(r for r in changed if r['kind']=='reg' and r['address']=='0x60031004')
r['value']='0xdeadbeef'
assert any(d['reason']=='exact mismatch; investigate' for d in compare(changed,rows)['differences'])
changed=copy.deepcopy(rows)
r=next(r for r in changed if r['kind']=='latch')
r['coarse']='0x80000000'
assert any(not c['ok'] for c in summarize(changed)['checks'])
changed=copy.deepcopy(rows)
r=next(r for r in changed if r['kind']=='mem' and r['stage']=='adv' and r['name']=='payload')
r['data']='00'*len(bytes.fromhex(r['data']))
assert any(c['name']=='adv/service_uuid' and not c['ok'] for c in summarize(changed)['checks'])
changed=copy.deepcopy(rows)
r=next(r for r in changed if r['kind']=='reg' and r['stage']=='adv' and r['address']=='0x600312cc')
r['value']='0x00340034'
diffs=compare(changed,rows)['differences']
assert len(diffs)==1 and diffs[0]['hardware']=='0x00340034' and diffs[0]['reason'].startswith('live controller state')
print('PASS: identical capture, identity mismatch, latch timeout, corrupted UUID')
