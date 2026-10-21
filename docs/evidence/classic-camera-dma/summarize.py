#!/usr/bin/env python3
"""Curate actual C7 outputs; fail instead of synthesizing missing results."""
import hashlib, json, re, shutil, sys
from pathlib import Path
from typing import Any
root, raw = (Path(s).resolve() for s in sys.argv[1:3])
out = root/'docs/evidence/classic-camera-dma'
def load(path): return json.loads((raw/path).read_text())
status = {name: int(code) for name, code in (line.split() for line in (raw/'status.txt').read_text().splitlines())}
required = ['clippy','wasm-clippy','rom','plain','wasm-build','demos','privacy']
assert all(status.get(name) == 0 for name in required), status
result: dict[str, Any] = dict(base='8f3a9eaa4ab2e4ba0d58721c0abe9fcf7428ecd6',main='142805d5399e0c3cf25995fe21ba524e75a2b3cb',required_checks={k:status[k] for k in required},empty_home=True,toolchain='1.99.0',nice=10,cpu_timing='NOT RUN')
result['implementation']='8ac344b77038631660f87025f76b272ec3a87b4f'
result['source_sha256']={path:hashlib.sha256((root/path).read_bytes()).hexdigest() for path in ['esp32/src/camera_dma.rs','esp32/src/camera_fifo.rs','esp32/src/i2s.rs','esp32/src/periph.rs','esp32/src/bus.rs','esp-periph/src/gdma/receive.rs','esp32/tests/camera_dma/cases.rs']}
result['tests']={}
for name in ['rom','plain']:
    rows = re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;', (raw/(name+'.log')).read_text())
    assert rows, name
    totals = dict(zip(['passed','failed','ignored','measured','filtered'],map(sum,zip(*(map(int,row) for row in rows)))))
    assert totals['failed'] == 0
    result['tests'][name] = totals
for key, file in [('c7','mutations/mutations.json'),('c5','fifo-mutations/mutations.json'),('c4','mode-mutations/mutations.json')]:
    rows = load(file)
    assert len(rows)-1 == {'c7':25,'c5':12,'c4':23}[key]
    assert rows[0]['name'] == 'baseline' and rows[0]['exit'] == 0
    assert all(r['killed'] for r in rows[1:])
    result[key+'_assertion_kills'] = len(rows)-1
    shutil.copyfile(raw/file,out/(key+'-mutations.json'))
rows = load('routing-mutations/mutations.json')
assert len(rows)==35 and all(r['killed'] for r in rows)
result['c6_assertion_kills'] = len(rows)
shutil.copyfile(raw/'routing-mutations/mutations.json',out/'c6-mutations.json')
driver=load('driver/driver-results.json')
result['actual_c_filter_recoveries']=len(driver['rows'])
result['retained_c_filter_partial_group_negatives']=len(driver['partial_group_limits'])
shutil.copyfile(raw/'driver/driver-results.json',out/'driver-results.json')
fixture=load('fixture-comparison.json')
assert fixture['console_equal'] and fixture['stderr_normalized_equal'] and fixture['base']['passed'] and fixture['candidate']['passed']
result['s3_frames']=sum(line.startswith('CAMERA:FRAME:') for line in fixture['candidate']['serial'])
result['s3_console_sha256']=fixture['candidate']['stdout_sha256']
shutil.copyfile(raw/'fixture-comparison.json',out/'s3-fixture.json')
result['codegen']={}
for label in ['base','main']:
    data=load('codegen-'+label+'/codegen.json')
    for row in data['rows']:
        if 'diff' in row:
            diff='\n'.join(row.pop('diff'))
            row['normalized_diff_sha256']=hashlib.sha256(diff.encode()).hexdigest()
            row['normalized_diff_lines']=len(diff.splitlines())
            row['diff_retention']='Full normalized diff retained in local raw receipt; compact counts/hash retained here.'
    result['codegen'][label]=dict(equal=sum(r['same'] for r in data['rows']),total=len(data['rows']))
    (out/('codegen-'+label+'.json')).write_text(json.dumps(data,indent=2)+'\n')
(out/'checks.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
