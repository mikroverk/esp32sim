#!/usr/bin/env python3
"""Run focused removal mutations; always restore the original source bytes."""
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[3]
RX = 'esp-periph/src/i2s/rx.rs'
UNIT = ['cargo', '+1.99.0', 'test', '-p', 'esp-periph', '--lib']
BUS = ['cargo', '+1.99.0', 'test', '-p', 'esp32sim', '--test', 'i2s_rx']
CASES = [
    ('RX start', RX, 'self.rx_conf & 4 != 0', 'true', UNIT, 'pcm_clock_slots_width_silence_and_reset'),
    ('slot mask', RX, 'mask & (1 << lane) == 0', 'false', UNIT, 'pcm_clock_slots_width_silence_and_reset'),
    ('wide sample alignment', RX, 'i32::from(sample) << 16', 'i32::from(sample)', UNIT, 'pcm_clock_slots_width_silence_and_reset'),
    ('PDM capability', RX, 'pdm2pcm && bits == 16', 'true && bits == 16', UNIT, 'pcm_clock_slots_width_silence_and_reset'),
    ('PDM conversion', RX, 'self.rx_conf & (1 << 21) != 0', 'true', UNIT, 'unsupported_modes_preserve_queued_input'),
    ('PDM decimation', RX, '\n                64\n', '\n                32\n', UNIT, 'pcm_clock_slots_width_silence_and_reset'),
    ('sample width', RX, '[16, 24, 32].contains(&bits)', 'true', UNIT, 'unsupported_modes_preserve_queued_input'),
    ('two slots', RX, '(tdm >> 16) & 15 == 1', 'true', UNIT, 'unsupported_modes_preserve_queued_input'),
    ('frame accumulator', RX, 'self.rx_acc %= self.cpu_hz;', 'self.rx_acc = 0;', UNIT, 'reset_discards_fractional_receiver_time'),
    ('reset accumulator', 'esp-periph/src/i2s.rs', 'if v & 3 != 0 { self.rx_acc = 0; }', '', UNIT, 'reset_discards_fractional_receiver_time'),
    ('input bound', RX, '65536 - self.frames.len()', '65537 - self.frames.len()', UNIT, 'bounded_input_and_tone'),
    ('tone clear', RX, 'self.tone = None;', 'let _ = &self.tone;', UNIT, 'bounded_input_and_tone'),
    ('C3 pump', 'esp32c3/src/bus.rs', 'self.i2s_rx_step(u64::from(cycles));', '', BUS, 'pcm_to_guest_dma_on_all_chips'),
    ('C6 clock', 'esp32c6/src/bus.rs', 'self.periph.i2s0.rx_pcr_clock(self.periph.pcr.read(0x78), self.periph.pcr.read(0x7c));', '', BUS, 'pcm_to_guest_dma_on_all_chips'),
    ('S3 RX port one', 'esp32s3/src/bus/dma.rs', 'self.dma_i2s_rx(cycles, 1);', '', BUS, 'absent_controllers_and_s3_second_receiver'),
]
CONTRACT = ['cargo', '+1.99.0', 'test', '-p', 'esp-periph', '--test', 'i2s_rx_contracts']
CASES.extend([
    ('mono packing', RX, 'self.rx_conf & (1 << 5) != 0', 'false', CONTRACT, 'mono_consumes_one_sample_with_both_slots_enabled'),
    ('PDM double decimation', RX, 'self.rx_conf & (1 << 22) != 0', 'false', CONTRACT, 'pdm_double_decimation_halves_pcm_rate'),
    ('tone replaces queue', RX, 'self.frames.clear();', 'let _ = &self.frames;', CONTRACT, 'tone_replaces_queue_and_push_replaces_tone'),
    ('push replaces tone', RX, 'self.tone = None;', 'let _ = &self.tone;', CONTRACT, 'tone_replaces_queue_and_push_replaces_tone'),
])
for chip in ['c3', 'c6']:
    CASES.append((chip + ' RX deadline', 'esp32' + chip + '/src/soc.rs', 'if self.periph.i2s0.rx_running()', 'if false', BUS, 'active_receiver_bounds_scheduler_sleep_with_and_without_pin_work'))
CASES.append(('C3 pin deadline bypass', 'esp32c3/src/soc.rs', 'let timer = if self.pins_active { self.pin_deadline() }', 'let timer = if self.pins_active { return self.pin_deadline(); }', BUS, 'active_receiver_bounds_scheduler_sleep_with_and_without_pin_work'))
for bit in ['1 << 3', '1 << 7', '3 << 10', '1 << 18']:
    CASES.append(('unsupported ' + bit, RX, '(' + bit + ')', '(0)', UNIT, 'unsupported_modes_preserve_queued_input'))
for chip in ['c3', 'c6', 's3']:
    CASES.append((chip + ' reset persistence', 'esp32' + chip + '/src/soc.rs', 'p.i2s0.rx_input = old.i2s0.rx_input;', '', BUS, 'pcm_to_guest_dma_on_all_chips'))

for name, old, new in [
    ('receive error flag', 'self.int_raw |= 1 << 3;', 'self.int_raw |= 0;'),
    ('receive interrupt change', '*irq_changed |= self.int_raw != before;', '*irq_changed |= false;'),
    ('receive stops on fault', 'self.running = false;', 'self.running = true;'),
]:
    CASES.append((name, 'esp-periph/src/gdma/receive.rs', old, new, BUS, 'shared_receive_reports_interrupt_changes'))

def run():
    results = []
    for name, file, old, new, command, test in CASES:
        path = ROOT / file
        original = path.read_text()
        assert old in original, (name, old)
        try:
            path.write_text(original.replace(old, new))
            p = subprocess.run(command + [test], cwd=ROOT, capture_output=True, text=True)
            killed = p.returncode != 0 and 'test result: FAILED' in p.stdout
            results.append({'mutation': name, 'test': test, 'killed': killed})
            print(json.dumps(results[-1]), flush=True)
        finally:
            path.write_text(original)
    return results

if __name__ == '__main__':
    results = run()
    Path(__file__).with_name('mutations.json').write_text(json.dumps(results, indent=2) + '\n')
    raise SystemExit(0 if all(row['killed'] for row in results) else 1)
