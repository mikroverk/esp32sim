#!/usr/bin/env python3
"""Run unchanged Arduino BLE examples; keep raw logs outside Git."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import time
import tempfile

p = argparse.ArgumentParser()
p.add_argument('--emulator', type=Path, required=True)
p.add_argument('--firmware', type=Path, required=True)
p.add_argument('--roms', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--chips', nargs='+', choices=['s3', 'c3', 'c6'], default=['s3', 'c3', 'c6'])
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def clean(text):
    return text.replace(str(Path.home()), '/Users/alice')

results = []
for chip, board, rom in [('s3', 'esp32-s3-devkitc-1', 'esp32s3_rev0_rom.elf'),
                         ('c3', 'esp32-c3-devkitm-1', 'esp32c3_rev3_rom.elf'),
                         ('c6', 'esp32-c6-devkitc-1', 'esp32c6_rev0_rom.elf')]:
    if chip not in a.chips:
        continue
    for example, seconds, action, required in [
        ('Server', 1.3, '1.0 ble read 0x0010\n', ['Hello World says Neil']),
        ('Notify', 2.6, '0.7 ble subscribe 0x0011\n', ['notification handle=0x0010']),
        ('Write', 1.2, '0.7 ble write 0x0010 48656c6c6f2066726f6d20686f7374\n0.8 ble read 0x0010\n',
         ['New value: Hello from host', 'text="Hello from host"']),
        ('Scan', 5.6, '', ['Name: esp32sim', 'Devices found: 1', 'Scan done!']),
    ]:
        firmware = a.firmware / chip / example / '.pio' / 'build' / board
        script = a.output / f'{chip}-{example}.script'
        script.write_text(('0.5 ble connect\n0.6 ble discover\n' + action) if example != 'Scan' else '')
        cmd = [str(a.emulator.resolve()), '--chip', 'esp32' + chip, '--boot', 'rom', '--rom', str(a.roms / rom),
               '--flash-image', str(firmware / 'firmware.factory.bin'), '--elf', str(firmware / 'firmware.elf'),
               '--ble', '--max-seconds', str(seconds), '--no-reboot', '--no-dump', '--script', str(script)]
        if chip == 'c6':
            cmd += ['--flash-mb', '8']
        start = time.perf_counter()
        serial_capture = tempfile.TemporaryFile(mode="w+t")
        proc = subprocess.Popen(cmd, stdout=serial_capture, stderr=subprocess.PIPE, text=True)
        lines = []
        first_ad = None
        modeled = None
        for line in proc.stderr:
            lines.append(line)
            if first_ad is None and (m := re.search(r'\[ble\] t=([\d.]+)s advertising ', line)):
                first_ad = time.perf_counter() - start
                modeled = float(m[1])
        code = proc.wait()
        wall = time.perf_counter() - start
        serial_capture.seek(0)
        serial = serial_capture.read()
        serial_capture.close()
        original = serial + ''.join(lines)
        serial_log = a.output / f'{chip}-{example}.serial.log'
        serial_log.write_text(clean(serial))
        output = clean(original)
        raw = a.output / f'{chip}-{example}.log'
        raw.write_text(output)
        checks = {text: text in output for text in required}
        checks['clean_stop'] = '[emu] stop: Halted' in output and 'Guru Meditation' not in output and 'assert failed' not in output
        if example != 'Scan':
            checks.update({text: text in output for text in [
                'uuid=4fafc201-1fb5-459e-8fcc-c5c9c331914b', 'name="',
                'connected handle=0x0001 guest=peripheral', 'discovery complete']})
        if example == 'Notify':
            checks['incrementing_notifications'] = all(f'value={n:02x}000000' in output for n in range(1, 5))
        results.append(dict(chip=chip, example=example, command=[clean(x) for x in cmd], script=script.read_text(),
                            exit_code=code, checks=checks, passed=code == 0 and all(checks.values()),
                            modeled_boot_to_advertising_s=modeled, wall_to_advertising_s=first_ad, wall_s=wall,
                            hashes={name: digest(firmware / name) for name in ['firmware.elf', 'firmware.factory.bin']},
                            sketch_sha256=digest(a.firmware / chip / example / 'src/main.cpp'),
                            rom_sha256=digest(a.roms / rom), log_sha256=digest(raw), serial_sha256=digest(serial_log), capture_order="stdout then stderr; advertising timed while reading stderr", original_log_sha256=hashlib.sha256(original.encode()).hexdigest(),
                            stop_summary=next((line for line in output.splitlines() if 'stop:' in line), None),
                            notification_values=re.findall(r'notification handle=0x0010 value=([0-9a-f]+)', output)))
        print(chip, example, results[-1]['passed'], checks, flush=True)
(a.output / 'runs.json').write_text(json.dumps(dict(emulator_sha256=digest(a.emulator), runs=results), indent=2) + '\n')
raise SystemExit(0 if all(r['passed'] for r in results) else 1)
