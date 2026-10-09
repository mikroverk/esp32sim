#!/usr/bin/env python3
"""Run unchanged ProbeB: run.py C3_BINARY ROM_ELF PROBE_BUILD OUTPUT_DIR."""
import hashlib
import json
import subprocess
import sys
from pathlib import Path

binary, rom, build, output = map(Path, sys.argv[1:])
output.mkdir(parents=True, exist_ok=True)
read = 'ble read-uuid 4fafc201-1fb5-459e-8fcc-c5c9c331914b beb5483e-36e1-4688-b7f5-ea07361b26a8'
scenarios = [
    ('normal', f'3.3 ble connect\n3.6 {read}\n6.6 ble disconnect\n9.9 ble connect\n10.2 {read}\n13.2 ble disconnect\n', 15, [19,19]),
    ('timeout', f'3.3 ble connect\n3.6 {read}\n5.6 ble central-stop\n21 ble connect\n21.3 {read}\n23.3 ble disconnect\n', 25, [8,19]),
]
results = {}
for name, script, seconds, reasons in scenarios:
    path = output / (name + '.script')
    path.write_text(script)
    args = [str(binary), '--boot', 'rom', '--rom', str(rom), '--ble', 'full', '--ble-scan',
            '--ble-observe', '--max-seconds', str(seconds), '--no-dump', '--script', str(path),
            '--trace-fn', 'r_llc_disconnect_end']
    for flag, suffix in [('bootloader','bootloader.bin'), ('ptable','partitions.bin'), ('app','bin')]:
        args += ['--'+flag, str(build / ('ProbeB.ino.'+suffix))]
    run = subprocess.run(args, capture_output=True, check=True)
    (output / ('emulator-'+name+'.log')).write_bytes(run.stdout)
    (output / ('emulator-'+name+'.stderr')).write_bytes(run.stderr)
    console, trace = run.stdout.decode(), run.stderr.decode()
    assert trace.count('[ble-att] value=48656c6c6f20576f726c642073617973204e65696c') == 2
    assert trace.count('disconnected advertising_resumed') == 2
    assert trace.count('[ble-state] connected') == 2
    assert not any(s in trace for s in ['[ble-error]', 'still pending', 'dropped='])
    assert not any(s in console for s in ['assert', 'panic', 'ble_uuid_flat'])
    assert not any(line.startswith('E (') for line in console.splitlines())
    actual = [int(line.split('a2=')[1].split(')')[0],16) for line in trace.splitlines()
              if 'r_llc_disconnect_end(a0=' in line]
    assert actual == reasons, actual
    for kind in ['ADV_IND','SCAN_RSP']:
        packets = {line.split('pdu=')[1] for line in trace.splitlines() if 'type='+kind+' ' in line}
        assert len(packets) == 1, 'advertising payload changed'
    results[name] = dict(reads=2, disconnect_reasons=reasons, advertising_resumed=2,
                         advertising_payloads_identical=True, guest_errors=0, modeled_seconds=seconds,
                         console_sha256=hashlib.sha256(run.stdout).hexdigest(),
                         trace_sha256=hashlib.sha256(run.stderr).hexdigest())
print(json.dumps(results, indent=2))
