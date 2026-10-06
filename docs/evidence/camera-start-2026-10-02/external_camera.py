#!/usr/bin/env python3
"""Check the unchanged driver on the public board; caller supplies firmware and ROM."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

p = argparse.ArgumentParser()
p.add_argument('--emulator', type=Path, required=True)
p.add_argument('--firmware', type=Path, required=True)
p.add_argument('--rom', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--expect', choices=['frames', 'empty', 'vsync-only'], required=True)
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
image = a.output / 'red.ppm'
image.write_bytes(b'P6\n1 1\n255\n' + bytes([255, 0, 0]))
script = a.output / 'capture.script'
script.write_text('2 uart0 C\n8 uart0 C\n14 uart0 C\n20 uart0 Y\n'
                  '23 uart0 C\n29 uart0 C\n35 uart0 C\n41 uart0 B\n44 uart0 C\n50 stop\n')
cmd = [str(a.emulator.resolve()), '--board', 'waveshare-cam', '--boot', 'rom',
       '--flash-mb', '8', '--rom', str(a.rom),
       '--bootloader', str(a.firmware / 'CameraFixture.ino.bootloader.bin'),
       '--ptable', str(a.firmware / 'CameraFixture.ino.partitions.bin'),
       '--app', str(a.firmware / 'CameraFixture.ino.bin'), '--cam-image', str(image),
       '--script', str(script), '--console', 'uart0', '--max-seconds', '51', '--no-dump']
r = subprocess.run(cmd, capture_output=True, check=True, timeout=180)
(a.output / 'stdout.log').write_bytes(r.stdout)
(a.output / 'stderr.log').write_bytes(r.stderr)
lines = [line for line in r.stdout.decode().splitlines() if line.startswith('CAMERA:')]
ready = 'CAMERA:READY:0:5640'

def frame(fmt, pixel):
    data = bytes(pixel) * (96 * 96 * 2 // len(pixel))
    h = 2166136261
    for b in data:
        h = ((h ^ b) * 16777619) & 0xffffffff
    return f'CAMERA:FRAME:96:96:{fmt}:18432:{h:08x}:{data[0]:02x}:{data[-1]:02x}'

# Solid red: RGB565 F800; full-range BT.601 Y=76, U=84, V=255.
rgb, yuv = frame(0, [248, 0]), frame(1, [76, 84, 76, 255])
if a.expect == 'vsync-only':
    rgb = frame(0, [76, 84, 76, 255])
if a.expect == 'empty':
    rgb = yuv = 'CAMERA:EMPTY'
expected = [ready] + [rgb] * 3 + [ready] + [yuv] * 3 + [ready, rgb]
assert lines == expected, (lines, expected)
result = dict(expect=a.expect, passed=True, serial=lines,
              stdout_sha256=hashlib.sha256(r.stdout).hexdigest(),
              emulator_sha256=hashlib.sha256(a.emulator.read_bytes()).hexdigest(),
              image_sha256=hashlib.sha256(image.read_bytes()).hexdigest(),
              script_sha256=hashlib.sha256(script.read_bytes()).hexdigest())
(a.output / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result))
