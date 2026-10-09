#!/usr/bin/env python3
"""Compare complete ProbeB records; never output address-bearing raw bytes."""
import hashlib
import json
import re
import sys
from pathlib import Path

SIZES = {'et': 16, 'cs': 90, 'tx': 14, 'rx': 20}

def records(text):
    # HOST annotations can interrupt a key, its value, or even one hex byte.
    # Serial reads do not delimit records: only the next PROBE marker does.
    text = re.sub(r'HOST[^\r\n]*(?:\r?\n|$)', '', text)
    text = text.replace('\r', '').replace('\n', '')
    result = []
    for chunk in text.split('PROBE ')[1:]:
        chunk = chunk.replace('\r', '').replace('\n', '').strip()
        fields = dict(re.findall(r'(\w+)=([^ ]+)', chunk))
        if fields.get('kind') != 'mem':
            result.append(fields)
            continue
        for key in ('stage', 'name', 'index', 'logical', 'physical', 'valid', 'data'):
            if key not in fields:
                raise ValueError(f'incomplete memory record: missing {key}')
        size = SIZES.get(fields['name'])
        if size is None: raise ValueError('unknown memory record type')
        if fields['valid'] == '0':
            if fields['data'] != 'none': raise ValueError('invalid record must say data=none')
        elif fields['valid'] == '1':
            data = fields['data']
            if len(data) != size * 2 or re.fullmatch('[0-9a-fA-F]+', data) is None:
                raise ValueError(f"incomplete or malformed {fields['name']} payload; expected {size} bytes")
            fields['bytes'] = bytes.fromhex(data)
        else: raise ValueError('invalid validity flag')
        result.append(fields)
    return result

def summary(path):
    raw = Path(path).read_bytes()
    recs = records(raw.decode(errors='replace'))
    stages = []
    active = None
    cs = []
    rx = []
    rings = []
    static = set()
    for r in recs:
        if r.get('kind') == 'snapshot':
            if r.get('edge') == 'begin':
                if active is not None: raise ValueError('incomplete snapshot')
                active = r['stage']; stages.append(active); rings.append(set())
            elif r.get('edge') == 'end':
                if active != r['stage']: raise ValueError('snapshot end mismatch')
                if rings[-1] != set(range(10)): raise ValueError('incomplete RX ring')
                active = None
        if r.get('kind') != 'mem' or 'bytes' not in r: continue
        if active != r['stage']: raise ValueError('memory record outside snapshot')
        b = r['bytes']; half = lambda n: int.from_bytes(b[n:n+2], 'little')
        if r['name'] == 'cs' and half(0)&31 == 3:
            static.add(static_cs(b))
            hop = (half(22)>>8)&31
            if not 5 <= hop <= 16 or b[39] != hop: raise ValueError('inconsistent hop fields')
            if half(22)&63 >= 37: raise ValueError('invalid data channel')
            if int.from_bytes(b[34:39], 'little').bit_count() < 2 or b[38]&0xe0:
                raise ValueError('invalid channel map')
            cs.append({'stage': active, 'format': half(0)&31, 'activity': half(2)&31,
                       'hop': (half(22)>>8)&31, 'csa2': bool(half(22)&16384),
                       'tx_rate': half(4)&3, 'rx_rate': (half(4)>>2)&3,
                       'map': b[34:39].hex()})
        if r['name'] == 'rx':
            index = int(r['index'])
            if index in rings[-1]: raise ValueError('duplicate RX slot')
            rings[-1].add(index)
            if int(r['logical'],0) != 0x1000+index*20 or half(0)&32767 != 0x1000+(index+1)%10*20:
                raise ValueError('RX ring stride/link mismatch')
            rx.append({'stage': active, 'link': half(0)&32767, 'owned': bool(half(0)&32768),
                       'invalid': bool(half(2)&32768)})
    if active is not None: raise ValueError('unterminated snapshot')
    if stages != ['advertising','connected','advertising','connected','advertising']:
        raise ValueError('need two complete connect/disconnect cycles')
    if not any(c['format']==3 for c in cs): raise ValueError('no format-3 sample')
    unique = lambda rows: [json.loads(v) for v in sorted({json.dumps(r,sort_keys=True) for r in rows})]
    return {'sha256': hashlib.sha256(raw).hexdigest(), 'bytes': len(raw), 'stages': stages,
            'memory_records': sum(r.get('kind')=='mem' for r in recs),
            'static_cs': sorted(static), 'control_structures': unique(cs),
            'rx_by_stage': {stage: {
                'samples': sum(r['stage']==stage for r in rx),
                'owned': sum(r['stage']==stage and r['owned'] for r in rx),
                'invalid': sum(r['stage']==stage and r['invalid'] for r in rx)
            } for stage in sorted(set(stages))},
            'rx_next_links': sorted({r['link'] for r in rx})}

# r_lld_con_start initializes these fields; exclude central-selected identity,
# PHY, hop/CSA/map, pointers, receive windows and live status/counters.
# Compare the remaining masks exactly, including after both disconnects.
STATIC = [(0, 0xf9ff), (2, 0xffff), (4, 0xfff0), (6, 0xffff), (8, 0xffff), (10, 0xffff), (18, 0xff00), (20, 0xffff),
          (22, 0xa0c0), (24, 0xff), (40, 0xffff)] + [
          (i, 0xffff) for i in list(range(42,80,2)) + [82,84,88]]

def static_cs(b):
    return tuple(int.from_bytes(b[i:i+2], 'little') & mask for i,mask in STATIC)

def compare(hardware, emulator):
    if hardware['static_cs'] != emulator['static_cs']:
        raise ValueError('static format-3 CS mismatch')
    for capture, csa, hops, rate in [(hardware, True, {7,9,15}, 1), (emulator, False, {5}, 0)]:
        for cs in capture['control_structures']:
            if cs['csa2'] != csa or cs['hop'] not in hops or cs['map'] != 'ffffffff1f' or cs['tx_rate'] != rate or cs['rx_rate'] != rate:
                raise ValueError('unexpected negotiated configuration; inspect capture before comparing')
    return {
        'static_cs': 'equal under declared masks in every format-3 sample',
        'static_masks': {str(i): hex(mask) for i,mask in STATIC},
        'rx_ring': 'ten 20-byte descriptors; all links checked in every snapshot',
        'post_disconnect': 'advertising snapshot after each disconnect; stale event/CS entries retained',
        'configuration': 'hardware CSA#2, varying hop, 2M PHY; emulator CSA#1 hop 5, 1M PHY; all 37 channels',
        'excluded_cs_bytes': {
            '0 bits9:10': 'coexistence control, r_lld_con_start; asynchronous updates',
            '4 bits0:3': 'TX/RX PHY selectors, r_lld_con_start',
            '12..18': 'central-selected access address/CRC; omitted',
            '22 channel/hop/CSA, 34..39': 'channel state and negotiated configuration; validated separately',
            '24 high byte, 86..87': 'live radio status; exact semantics/updates remain unproven',
            '26..33': 'receive window, TX pointer, event duration; r_lld_con_evt_start_cbk / r_lld_con_evt_time_update',
            '80..81': 'event counter; asynchronous snapshots'},
        'limits': 'No synchronized packet/IRQ capture, CRC-error injection, hardware empty-first case or raw advertising bytes.'}

def main():
    if len(sys.argv)!=3: raise SystemExit('compare.py HARDWARE_LOG EMULATOR_LOG')
    hardware, emulator = map(summary,sys.argv[1:])
    result = compare(hardware, emulator)
    print(json.dumps({'hardware':hardware,'emulator':emulator,'comparison':result},indent=2))

if __name__ == '__main__': main()
