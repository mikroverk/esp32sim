#!/usr/bin/env python3
"""Serial framing regressions using synthetic data without device identifiers."""
from compare import records, static_cs, compare

def record(kind, size):
    return f'PROBE kind=mem stage=connected name={kind} index=7 logical=0x1000 physical=0x3fc90000 valid=1 data=' + 'ab'*size + '\n'

for kind, size in [('tx',14),('cs',90),('et',16),('rx',20)]:
    clean=record(kind,size)
    expected=records(clean)[0]['bytes']
    # Actual failures: break before data, inside valid=1, physical=, or a hex byte.
    for pos in [2, clean.index(' data='), clean.index('valid=')+6, clean.index('physical')+8,
                clean.index('data=')+6, len(clean)-4]:
        split=clean[:pos]+'\nHOST t=1 connected\n\nHOST t=2 read=ok\n'+clean[pos:]
        assert records(split)[0]['bytes']==expected
    # Arbitrary read fragmentation, including CRLF, still preserves every byte.
    assert records('\r\n'.join(clean[i:i+7] for i in range(0,len(clean),7)))[0]['bytes']==expected
    for bad in [clean.replace('valid=1 data=', 'valid=1 missing='), clean[:-4]+'\n',
                clean.replace('data=ab','data=az'), clean[:-1]+'ff\n']:
        try: records(bad)
        except ValueError: pass
        else: raise AssertionError('malformed record accepted')
print('serial framing checks pass')

# A static-field mutation must fail; central-selected configuration is explicit.
b = bytearray(90)
c = {'static_cs': [static_cs(b)], 'control_structures': []}
compare(c, c)
b[2] = 1
try: compare(c, dict(c, static_cs=[static_cs(b)]))
except ValueError: pass
else: raise AssertionError('static CS mismatch accepted')
print('static comparison checks pass')
