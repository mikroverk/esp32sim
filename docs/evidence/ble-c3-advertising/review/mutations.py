#!/usr/bin/env python3
"""Run isolated source mutations, restore every source, and retain a compact verdict."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

root = Path(__file__).resolve().parents[4]
os.chdir(root)
out = root / 'target/ble-mutations'
out.mkdir(parents=True, exist_ok=True)
os.environ['TMPDIR'] = str(out)
os.environ['ESP32SIM_ROM_DIR'] = str(root / 'web/wasm/fw')
unit = ['cargo', '+1.99.0', 'test', '--release', '-p', 'esp32c3', '--lib', 'ble_lc::tests::']
reboot = ['cargo', '+1.99.0', 'test', '--release', '-p', 'esp32c3', '--test', 'ble_full', 'guest_esp_restart', '--', '--include-ignored']
lc = 'esp32c3/src/ble_lc.rs'
mutations = [
    ('latch_latency', lc, 's.latch = Some(s.cycles + HALF_US_CYCLES);', 's.latch = Some(s.cycles + HALF_US_CYCLES + 1);', unit),
    ('fine_latch_counter', lc, '624 - (hus % 625) as u32', '0', unit),
    ('reset_latency', lc, 's.reset = Some(s.cycles + HALF_US_CYCLES);', 's.reset = Some(s.cycles + HALF_US_CYCLES + 1);', unit),
    ('reset_keeps_alarm', lc, 's.reset = None;\n            s.alarm = None;', 's.reset = None;', unit),
    ('alarm_one_half_us_late', lc, '(now + delta) * HALF_US_CYCLES', '(now + delta + 1) * HALF_US_CYCLES', unit),
    ('end_status_aborted', lc, 'self.complete(event.entry, 3, sram);', 'self.complete(event.entry, 4, sram);', unit),
    ('half_slot_bit_removed', lc, 'if self.ram.read(0) & 0x100 != 0 && s.cycles /', 'if false && self.ram.read(0) & 0x100 != 0 && s.cycles /', unit),
    ('event_past_due_wraps', lc, 'let due = if delta >= PERIOD / 2 { now }', 'let due = if false { now }', unit),
    ('power_gate_removed', lc, 'if !self.accessible { return 0 }', 'if false { return 0 }', unit),
    ('event_fine_removed', lc, '624u64.saturating_sub(half(sram, entry + 6) as u64)', '0', unit),
    ('alarm_past_due_wraps', lc, 'Some(if delta >= PERIOD / 2 { s.cycles }', 'Some(if false { s.cycles }', unit),
    ('end_coalescing', lc, 'self.fifo.push_back(source);', 'if !self.fifo.contains(&source) { self.fifo.push_back(source); }', unit),
    ('stale_timer_retained', lc, 's.fifo.retain(|&source| source != TIMER);', '', unit),
    ('reboot_disables_controller', 'esp32c3/src/soc.rs', 'if old.ble_lc.enabled() {', 'if false {', reboot),
    ('enable_skips_optional_refresh', 'esp32c3/src/periph.rs', 'self.refresh_optional(0x31);', '', unit),
    ('enable_skips_work_refresh', 'esp32c3/src/periph.rs', 'self.ble_lc.observe(observe);\n        self.refresh_work();', 'self.ble_lc.observe(observe);', unit),
    ('half_slot_deadline_removed', lc, '[s.reset, s.latch, s.alarm, half_slot,', '[s.reset, s.latch, s.alarm, None,', unit),
    ('mapping_start_mismatch', lc, 'entry >> 18 != start / 4 || ', '', unit[:-1] + ['mapping_boundaries_match_rom_em_base_reg_lut', '--', '--include-ignored']),
    ('gated_write_accepted', lc, 'if !self.accessible { return WriteEffect::NONE }', '', unit),
]
results = []
for name, file, before, after, command in mutations:
    if sys.argv[1:] and name not in sys.argv[1:]: continue
    path = Path(file)
    original = path.read_bytes()
    source = original.decode()
    assert before in source, name
    try:
        path.write_text(source.replace(before, after, 1))
        with (out / (name + '.log')).open('w') as log:
            run = subprocess.run(command, stdout=log, stderr=log, timeout=180)
        text = (out / (name + '.log')).read_text()
        failed = re.findall(r'^test (\S+) \.\.\. FAILED$', text, re.M)
        assert run.returncode and failed and 'error[E' not in text, (name, run.returncode, text[-1000:])
        results.append(dict(mutation=name, file=file, before=before, after=after,
                            source_sha256=hashlib.sha256(original).hexdigest(), command=' '.join(command),
                            exit_code=run.returncode, failing_tests=failed))
        print(name + ': killed', flush=True)
    finally:
        path.write_bytes(original)
    (out / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
