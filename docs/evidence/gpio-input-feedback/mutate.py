#!/usr/bin/env python3
"""Run EX217 mutations from a clean source tree; restore each file before continuing."""
import json
from pathlib import Path
import re
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
output = Path(sys.argv[1]) if len(sys.argv) > 1 else root / 'target/ex217-mutations.json'
mutations = []


def add(name, path, before, after, package, target, test=''):
    mutations.append((name, path, before, after, package, target, test))


def gpio(name, before, after):
    add(name, 'esp-periph/src/gpio.rs', before, after,
        'esp-periph', '--test=gpio_release')


gpio('Do not remember host drive mask', 'self.external_mask |= mask;', 'self.external_mask |= 0;')
gpio('Do not remember host low level', 'self.external_levels &= !mask;', 'self.external_levels |= mask;')
gpio('Keep released drive active', 'self.external_mask &= !mask;', 'self.external_mask |= mask;')
gpio('Ignore pull-down', 'let passive = self.pull_up | !self.pull_down;', 'let passive = u64::MAX;')
gpio('Floating pad defaults low', 'let passive = self.pull_up | !self.pull_down;', 'let passive = self.pull_up;')
gpio('Ignore enabled output', '(passive & !self.enable)', 'passive')
gpio('Skip output pad resolution', 'self.resolve((old ^ self.out) | (old_enable ^ self.enable));', '')
gpio('Do not latch GPIO edges', 'self.status |= bit;', 'self.status |= 0;')
gpio('Do not guard invalid release pin', 'if pin >= 49 { return false; }\n        let mask = 1u64 << pin;\n        self.external_mask &= !mask;', 'let mask = 1u64 << pin;\n        self.external_mask &= !mask;')

for chip in ['c3', 'c6', 's3']:
    package = f'esp32{chip}'
    bus = f'{package}/src/bus.rs'
    soc = f'{package}/src/soc.rs'
    base = '0x6009_103c | 0x6009_1040' if chip == 'c6' else '0x6000_403c | 0x6000_4040'
    hook = f'if matches!(addr & !3, {base})'
    add(f'{chip}: skip same-cycle read delivery', bus, hook, 'if false', package, '--test=gpio_feedback', 'gpio_reads_')
    add(f'{chip}: do not restore host drives', soc, 'p.gpio.restore_external(&old.gpio);', '', package, '--test=gpio_feedback', 'reboot_')
    add(f'{chip}: ignore host release', soc, 'self.periph.gpio.release_input(pin);', '', package, '--test=gpio_feedback', 'released_inputs_')
    add(f'{chip}: skip board release callbacks', bus,
        'for pin in self.board.released_inputs() { esp_soc::SocBus::gpio_release_input(self, pin); }',
        '', package, '--test=gpio_feedback', 'board_releases_')
    add(f'{chip}: ignore pull registers', f'{package}/src/periph.rs',
        'v & (1 << 8) != 0, v & (1 << 7) != 0);',
        'false, false);', package, '--test=gpio_feedback', 'released_inputs_')

add('S3: ignore edge IRQs on output writes', 'esp32s3/src/bus.rs',
    'matches!((config >> 7) & 7, 1..=5)', 'matches!((config >> 7) & 7, 4 | 5)',
    'esp32s3', '--test=gpio_feedback', 'released_inputs_')
add('S3: keep quiet cadence after a host release', 'esp32s3/src/soc.rs',
    'self.periph.gpio.release_input(pin);\n        self.refresh_tick_budget();',
    'self.periph.gpio.release_input(pin);', 'esp32s3', '--lib', 'released_and_same_cycle_')
add('S3: keep quiet cadence after a same-cycle read', 'esp32s3/src/bus.rs',
    'self.deliver_board_inputs();\n            self.refresh_tick_budget();',
    'self.deliver_board_inputs();', 'esp32s3', '--lib', 'released_and_same_cycle_')
add('Reboot: drop high host drive levels', 'esp-periph/src/gpio.rs',
    'self.external_levels = old.external_levels;', 'self.external_levels = 0;',
    'esp32c3', '--test=gpio_feedback', 'reboot_')
add('Reboot: leave synthetic input changes queued', 'esp-periph/src/gpio.rs',
    'self.input_changes.clear();', '', 'esp32s3', '--test=gpio_feedback', 'reboot_')
add('C6: use consecutive CS matrix signals', 'esp32c6/src/bus.rs',
    '[63, 64, 65, 68, 101, 102, 103, 104, 105]', '[63, 64, 65, 68, 69, 70, 71, 72, 73]',
    'esp32c6', '--test=spi_routes', 'spi_additional_')
add('C6: omit native additional CS pins', 'esp32c6/src/bus.rs',
    '&[16, 17, 18, 19, 20, 21]', '&[16]',
    'esp32c6', '--test=spi_routes', 'spi_additional_')
add('C6: bypass pin-aware callback', 'esp32c6/src/bus.rs',
    'if self.board.uses_spi_pins()', 'if false',
    'esp32c6', '--test=spi_routes', 'spi_native_')
add('SPI: ignore CS disable and polarity', 'esp-soc/src/pins.rs',
    'spi.read(0x20) & ((1 << cs) | (1 << (cs + 7))) == 0', 'true',
    'esp32c6', '--test=spi_routes', 'spi_additional_')
add('SPI: omit native CS routes', 'esp-soc/src/pins.rs',
    'selects.get(cs) == Some(&pin) && self.function(pin, f)', 'selects.get(cs) == Some(&pin) && self.function(pin, f) && false',
    'esp32c6', '--test=spi_routes', 'spi_native_')

add('Pull-up must win simultaneous pulls', 'esp-periph/src/gpio.rs',
    'let passive = self.pull_up | !self.pull_down;', 'let passive = !self.pull_down;',
    'esp32c3', '--test=gpio_feedback', 'released_inputs_')
for chip in ['s3', 'c3', 'c6']:
    add(f'{chip}: omit deadline-driven release', f'esp32{chip}/src/bus.rs',
        '        for pin in self.board.released_inputs() { esp_soc::SocBus::gpio_release_input(self, pin); }\n    }\n\n    fn periph_read',
        '    }\n\n    fn periph_read', f'esp32{chip}', '--test=gpio_feedback', 'board_releases_')

add('S3: omit upper-bank input-read delivery', 'esp32s3/src/bus.rs',
    '0x6000_403c | 0x6000_4040', '0x6000_403c',
    'esp32s3', '--test=gpio_feedback', 'first_read_')

results = []
for name, rel, before, after, package, target, test in mutations:
    if len(sys.argv) > 2 and sys.argv[2] not in name:
        continue
    path = root / rel
    original = path.read_text()
    assert before in original, (name, before)
    cmd = ['cargo', '+1.99.0', 'test', '-p', package, target]
    if test:
        cmd.append(test)
    try:
        path.write_text(original.replace(before, after))
        run = subprocess.run(cmd, cwd=root, capture_output=True, text=True)
    finally:
        path.write_text(original)
    killed_by = re.findall(r'^test (\S+) \.\.\. FAILED$', run.stdout, re.M)
    result = {'mutation': name, 'file': rel, 'before': before, 'after': after,
              'command': ' '.join(cmd), 'exit': run.returncode, 'killed_by': killed_by}
    results.append(result)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(results, indent=2) + '\n')
    print(name, 'KILLED' if killed_by else 'SURVIVED/BUILD ERROR', flush=True)
    if not killed_by:
        print(run.stdout[-4000:], run.stderr[-4000:])
        sys.exit(1)
