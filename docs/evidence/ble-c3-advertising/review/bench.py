#!/usr/bin/env python3
"""Seven alternating off-mode pairs: bench.py MAIN_BINS CANDIDATE_BINS FW_DIR."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import resource
import statistics
import subprocess
import sys

before, after, fw = map(Path, sys.argv[1:])
sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
result = dict(system=platform.system(), release=platform.release(), architecture=platform.machine(),
              conditions='Sequential alternating arms, no concurrent builds or tests; unrelated host load uncontrolled.',
              measurement='Child user CPU seconds, process startup included; one warmup pair followed by seven measured pairs.', demos={})
for name, binary in [('hello','esp32sim'),('c3-hello','esp32sim-c3'),('c6-hello','esp32sim-c6'),('c3-wifi-station','esp32sim-c3')]:
    wifi = name == 'c3-wifi-station'
    if wifi:
        files = dict(rom='esp32c3_rev3_rom.elf', bootloader='public/c3-wifi-bootloader.bin',
                     ptable='public/c3-wifi-ptable.bin', app='public/c3-wifi_station.bin')
        flash, seconds = 4, 14
    else:
        manifest=json.loads((fw/(name+'.json')).read_text()); files=manifest['files']; flash=manifest['flash_mb']; seconds=30
    args=['--boot','rom','--flash-mb',str(flash),'--max-seconds',str(seconds),'--no-dump','--board','none']
    for flag in ['rom','bootloader','ptable','app']: args += ['--'+flag,str(fw/files[flag])]
    if wifi: args += ['--console','usb','--wifi','ssid=esp32sim,psk=esp32sim-pass','--net','none']
    arms=[('main',before/binary),('candidate',after/binary)]
    record=dict(args=[a.replace(str(fw),'<FW_DIR>') for a in args],input_sha256={p:sha(fw/p) for p in files.values()},
                binary_sha256={label:sha(path) for label,path in arms},pairs=[])
    for pair in range(8):
        samples={}
        for label,path in arms:
            load=os.getloadavg()[0]; start=resource.getrusage(resource.RUSAGE_CHILDREN)
            run=subprocess.run([str(path),*args],capture_output=True,check=True)
            end=resource.getrusage(resource.RUSAGE_CHILDREN)
            stop=next(l for l in run.stderr.decode().splitlines() if l.startswith('[emu] stop:'))
            cores=[int(n) for n in re.findall(r'core\d+ (\d+)',stop)]
            count=sum(cores) if cores else int(re.search(r'(\d+) insns',stop)[1])
            if wifi: assert b'PING done sent=5 received=5' in run.stdout
            samples[label]=dict(user_seconds=end.ru_utime-start.ru_utime,load_before=load,instructions=count,
                                console_sha256=hashlib.sha256(run.stdout).hexdigest())
        assert samples['main']['instructions']==samples['candidate']['instructions']
        assert samples['main']['console_sha256']==samples['candidate']['console_sha256']
        record['pairs'].append(dict(pair=pair,warmup=pair==0,order=['main','candidate'],**samples))
    record['median_user_seconds']={label:statistics.median(p[label]['user_seconds'] for p in record['pairs'][1:]) for label,_ in arms}
    med=record['median_user_seconds']; record['change_percent']=100*(med['candidate']/med['main']-1)
    result['demos'][name]=record
print(json.dumps(result,indent=2))
