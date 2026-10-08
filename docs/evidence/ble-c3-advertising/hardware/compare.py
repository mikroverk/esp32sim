#!/usr/bin/env python3
"""Compare complete hardware and emulator PROBE captures; no third-party modules."""
import argparse
import hashlib
import json
import statistics
from pathlib import Path

PERIOD = (1 << 28) * 625

def number(s):
    return int(s, 16) if s.startswith('0x') else int(s)

def parse(path):
    records = []
    for line in Path(path).read_text(errors='replace').splitlines():
        if not line.startswith('PROBE '):
            continue
        fields = line[6:].split()
        if any('=' not in x for x in fields):
            raise ValueError('malformed PROBE line')
        r = dict(x.split('=', 1) for x in fields)
        if len(r) != len(fields) or 'kind' not in r:
            raise ValueError('duplicate field or missing kind')
        records.append(r)
    if not records or records[-1].get('kind') != 'done':
        raise ValueError('capture missing final PROBE kind=done; capture at least 15 seconds from reset')
    if sum(r['kind'] == 'meta' for r in records) != 1 or sum(r['kind'] == 'done' for r in records) != 1:
        raise ValueError('expected exactly one boot, not multiple captures concatenated')
    if any(r['kind'] == 'error' for r in records):
        raise ValueError('firmware reported an error')
    for stage in ('pre', 'init', 'adv'):
        for kind, expected in [('reg', 76), ('map', 56), ('snapshot', 2)]:
            if sum(r['kind'] == kind and r.get('stage') == stage for r in records) != expected:
                raise ValueError(f'incomplete {stage} {kind} records')
    for stage in ('init', 'adv'):
        if sum(r['kind'] == 'latch' and r.get('stage') == stage for r in records) != 8:
            raise ValueError(f'incomplete {stage} latch records')
    windows = [r for r in records if r['kind'] == 'window']
    if len(windows) != 1:
        raise ValueError('expected one sampling window')
    if sum(r['kind'] == 'event' for r in records) != number(windows[0]['count']):
        raise ValueError('missing event records')
    return records

def key(r):
    return '/'.join(r.get(k, '') for k in ('kind','stage','name','index','address','edge'))

def dynamic_reason(r, field):
    kind = r['kind']
    if kind in ('event','window','snapshot','latch'):
        return 'asynchronous timing/state; see derived timing checks'
    if kind == 'map' and field == 'value':
        return 'allocation-dependent physical mapping; logical start compared separately'
    if kind == 'mem' and field == 'physical':
        return 'allocation-dependent SRAM address'
    # LC+2cc: ROM r_rwble_isr at 0x4002e7e4 reads this only under
    # LC+60 bit21, and prints 'EM BASE ERROR'. It is live diagnostic state,
    # not static configuration. Keep its numeric mismatch in the report.
    if kind == 'reg' and field == 'value' and number(r['address']) in (
        0x60031010,0x6003101c,0x60031020,0x60031024,0x600310ec,0x600310f0,0x60031100,0x600312cc,0x600312d8):
        return 'live controller state; exact difference retained, no equality claim'
    return None

def summarize(records):
    checks, rates, latency, intervals = [], {}, {}, {}
    def check(name, ok, detail):
        checks.append(dict(name=name,ok=bool(ok),detail=detail))
    for stage in ('init','adv'):
        rows = [r for r in records if r['kind']=='latch' and r['stage']==stage]
        valid = [r for r in rows if number(r['busy'])==0 and number(r['polls'])>0
                 and number(r['coarse']) < 1<<28 and number(r['fine'])<=624]
        check(stage+'/latch_complete',len(valid)==8, f'{len(valid)}/8 samples')
        rate=[]
        for a,b in zip(valid,valid[1:]):
            dt=(number(b['t0'])+number(b['t1'])-number(a['t0'])-number(a['t1']))/2
            ha=number(a['coarse'])*625+624-number(a['fine'])
            hb=number(b['coarse'])*625+624-number(b['fine'])
            if dt>0: rate.append(((hb-ha)%PERIOD)/dt)
        rates[stage]=rate
        # 0.5% screens the half-us hypothesis, allows crystal drift, integer-us
        # timer quantization and the measured software brackets. Not calibration.
        check(stage+'/half_us_rate', bool(rate) and all(abs(x-2)<=0.01 for x in rate),rate)
        latency[stage]=[dict(polls=number(r['polls']),
                            write_to_observed_clear_upper_cycles=(number(r['cd'])-number(r['cw']))&0xffffffff,
                            total_timer_bracket_us=number(r['t1'])-number(r['t0'])) for r in valid]
    window=next(r for r in records if r['kind']=='window')
    check('event_buffer_no_loss',number(window['dropped'])==0,number(window['dropped']))
    duration=number(window['end'])-number(window['begin'])
    check('two_second_window',2000000<=duration<=2100000,duration)
    for source in ('et','alarm'):
        rows=[r for r in records if r['kind']=='event' and r['source']==source]
        # Unique programmed deadlines, not RF packet timestamps. All 16 ET slots
        # rotate. Initial snapshots can contain old entries; retain those deltas.
        stamps=sorted(set(number(r['coarse'])*625+624-number(r['fine']) for r in rows
                          if number(r['coarse'])<1<<28 and number(r['fine'])<=624))
        # Exclude initial stale state without assuming timer and LC epochs match.
        # Consecutive differences are still diagnostic; outliers stay in output.
        deltas=[(b-a)/2 for a,b in zip(stamps,stamps[1:])]
        intervals[source]=dict(unique_deadlines=len(stamps),deltas_us=deltas,
                               median_us=statistics.median(deltas) if deltas else None)
        check(source+'/sufficient_deadlines',len(stamps)>=5,len(stamps))
    maps=[r for r in records if r['kind']=='map' and r['stage']=='adv']
    check('adv/mappings_present',any(number(r['value'])&0x3ffff for r in maps),'nonzero SRAM mapping')
    mem=[r for r in records if r['kind']=='mem' and r['stage']=='adv']
    for name, size in [('et',16),('cs',90),('tx',14)]:
        rows=[r for r in mem if r['name']==name]
        check('adv/'+name+'_mapped',bool(rows) and all(r['valid']=='1' and len(r['data'])==size*2 for r in rows),len(rows))
    cs=next((bytes.fromhex(r['data']) for r in mem if r['name']=='cs' and r['valid']=='1'),b'')
    if len(cs)==90:
        check('adv/cs_format4',int.from_bytes(cs[:2],'little')&31==4,cs[:2].hex())
        check('adv/channels_all3',(int.from_bytes(cs[38:40],'little')>>5)&7==7,cs[38:40].hex())
    payloads=[bytes.fromhex(r['data']) for r in mem if r['name']=='payload' and r['valid']=='1']
    check('adv/service_uuid',any(bytes.fromhex('4b9131c3c9c5cc8f9e45b51f01c2af4f') in b for b in payloads),'128-bit service UUID bytes')
    check('adv/scan_response_name',any(b'BLE Server Example' in b for b in payloads),'complete name bytes')
    for r in mem:
        if r['valid']!='1': continue
        off=number(r['logical']); size=len(bytes.fromhex(r['data']))
        candidates=[number(m['value']) for m in maps if number(m['value']) and (number(m['value'])>>18)*4<=off]
        v=max(candidates,key=lambda v:v>>18) if candidates else 0
        end=min([(number(m['value'])>>18)*4 for m in maps if (number(m['value'])>>18)*4>off]+[0x10000])
        addr=(0x3fc00000|((v<<2)&0xffffc))+off-(v>>18)*4
        check('adv/mapping/'+r['name']+'/'+r['index'],bool(v&0x3ffff) and off+size<=end and addr==number(r['physical']), 'decoder consistency only')
    return dict(checks=checks,latch_rate_half_us_per_us=rates,latch_latency_bounds=latency,
                programmed_deadline_intervals=intervals,max_poll_gap_us=number(window['max_gap_us']))

def compare(hardware, emulator):
    left, right = {key(r):r for r in hardware}, {key(r):r for r in emulator}
    if len(left)!=len(hardware) or len(right)!=len(emulator):
        raise ValueError('duplicate PROBE record identity')
    diffs=[]; equal=0
    for k in sorted(left.keys()|right.keys()):
        a,b=left.get(k),right.get(k)
        if a is None or b is None:
            diffs.append(dict(key=k,hardware=a,emulator=b,reason='record absent; asynchronous events may differ'))
            continue
        for f in sorted(a.keys()|b.keys()):
            if a.get(f)==b.get(f): equal+=1; continue
            reason=dynamic_reason(a,f)
            if a['kind']=='mem' and f=='data' and a['valid']==b['valid']=='1':
                av,bv=bytes.fromhex(a[f]),bytes.fromhex(b[f])
                for offset in range(max(len(av),len(bv))):
                    x=av[offset] if offset<len(av) else None; y=bv[offset] if offset<len(bv) else None
                    if x==y: continue
                    note='raw byte mismatch; inspect inferred layout'
                    if a['name']=='cs' and 6<=offset<12: note='device-specific advertiser address'
                    if a['name']=='et': note='live event state/timestamp/pointer; asynchronous snapshot'
                    if a['name']=='rx': note='live RX state; no controlled incoming packet in this run'
                    diffs.append(dict(key=k,field=f,offset=offset,hardware=x,emulator=y,reason=note))
            else:
                diffs.append(dict(key=k,field=f,hardware=a.get(f),emulator=b.get(f),reason=reason or 'exact mismatch; investigate'))
        if a['kind']=='map':
            x,y=number(a['value'])>>18,number(b['value'])>>18
            if x!=y: diffs.append(dict(key=k,field='logical_start',hardware=x*4,emulator=y*4,reason='exact mismatch; investigate'))
    hs,es=summarize(hardware),summarize(emulator)
    timing=[]
    for source in ('et','alarm'):
        h=hs['programmed_deadline_intervals'][source]['median_us']; e=es['programmed_deadline_intervals'][source]['median_us']
        timing.append(dict(source=source,hardware_median_us=h,emulator_median_us=e,tolerance_us=10000,
                           within_tolerance=h is not None and e is not None and abs(h-e)<=10000,
                           reason='independent 0..10-ms advertising jitter; programmed deadlines, not RF timing'))
    return dict(equal_fields=equal,differences=diffs,hardware=hs,emulator=es,interval_comparison=timing,
                verdict='comparison_only_hardware_semantics_not_proven',
                latency_policy='Report measured software upper bounds and poll counts without pass tolerance. The model assumes 80 cycles; instruction and APB timing differ. No exact latch-latency claim.')

def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('hardware'); ap.add_argument('emulator')
    args=ap.parse_args()
    try:
        result=compare(parse(args.hardware),parse(args.emulator))
        result['capture_sha256']={k:hashlib.sha256(Path(v).read_bytes()).hexdigest() for k,v in vars(args).items()}
    except (ValueError,KeyError,OSError) as e:
        print(json.dumps(dict(error=str(e)))); return 2
    print(json.dumps(result,indent=2))
    # Exact static mismatches fail; asynchronous differences remain diagnostic.
    return int(any(d.get('reason') in ('exact mismatch; investigate','raw byte mismatch; inspect inferred layout') for d in result['differences']) or any(not c['ok'] for side in ('hardware','emulator') for c in result[side]['checks']) or any(not c['within_tolerance'] for c in result['interval_comparison']))

if __name__=='__main__':
    raise SystemExit(main())
