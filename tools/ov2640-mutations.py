#!/usr/bin/env python3
"""Semantic mutations for the OV2640 contract; restore source even on failure.
Run with CARGO_TARGET_DIR outside the control checkout and --out for raw logs.
Compilation errors, timeouts and missing tests do not count as kills.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess

MUTATIONS = [
    ('bank_selection', 'self.bank = value & 1;', 'self.bank = 1;', 'ov2640_bank_isolated_ordinary_writes'),
    ('software_reset', 'self.reset();\n        } else {', 'self.sync_state();\n        } else {', 'ov2640_com7_reset_clears_both_banks_and_stream'),
    ('width_high_bits', 'dsp[0x5c] & 3', 'dsp[0x5c] & 0', 'ov2640_public_uxga_requires_high_geometry_bits'),
    ('height_high_bit', 'dsp[0x5c] & 4', 'dsp[0x5c] & 0', 'ov2640_public_uxga_requires_high_geometry_bits'),
    ('width_low_bits', 'u32::from(dsp[0x5a])', 'u32::from(0u8)', 'ov2640_public_qqvga_vga_and_format_sequences'),
    ('height_low_bits', 'u32::from(dsp[0x5b])', 'u32::from(0u8)', 'ov2640_public_qqvga_vga_and_format_sequences'),
    ('format', 'image_mode: dsp[0xda]', 'image_mode: 0', 'ov2640_public_qqvga_vga_and_format_sequences'),
    ('standby', 'self.regs[1][0x09] & 0x10 == 0', 'true', 'ov2640_stream_gates_are_bank_specific_and_reversible'),
    ('dvp_reset', 'dsp[0xe0] & 4 == 0', 'true', 'ov2640_stream_gates_are_bank_specific_and_reversible'),
    ('dsp_bypass', 'dsp[0x05] & 1 == 0', 'true', 'ov2640_stream_gates_are_bank_specific_and_reversible'),
    ('read_pointer', 'if !read { self.pointer = true; }', 'if !read { self.pointer = true; } else { self.addr = 0; }', 'ov2640_public_probe_reset_and_stop_reads'),
    ('write_phase', 'if !read { self.pointer = true; }', 'if !read { self.pointer = false; }', 'ov2640_bank_isolated_ordinary_writes'),
    ('ordinary_storage', 'self.regs[self.bank as usize][self.addr as usize] = value;', 'self.regs[self.bank as usize][self.addr as usize] = 0;', 'ov2640_bank_isolated_ordinary_writes'),
    ('reset_ids', '(0x0a, 0x26)', '(0x0a, 0x00)', 'ov2640_public_probe_reset_and_stop_reads'),
    ('id_write_protection', 'if self.bank != 1 || ![0x0a, 0x0b, 0x1c, 0x1d].contains(&self.addr)', 'if true', 'ov2640_public_probe_reset_and_stop_reads'),
    ('reset_bank_scope', 'self.bank == 1 && self.addr == 0x12', 'self.addr == 0x12', 'ov2640_stream_gates_are_bank_specific_and_reversible'),
    ('reset_leaves_stale_state', 'self.regs = [[0; 256]; 2];', 'self.regs[1][0x12] = 0;', 'ov2640_com7_reset_clears_both_banks_and_stream'),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    if not os.environ.get('CARGO_TARGET_DIR'):
        parser.error('set an isolated CARGO_TARGET_DIR')
    args.out.mkdir(parents=True, exist_ok=True)
    path = Path('esp-soc/src/camera/ov2640.rs')
    source = path.read_text()
    results = []
    try:
        for name, old, new, test in MUTATIONS:
            assert source.count(old) == 1, (name, source.count(old))
            path.write_text(source.replace(old, new))
            command = ['nice', '-n', '10', 'cargo', '+1.99.0', 'test', '--release', '-p', 'esp-soc', '--test', 'ov2640', test, '--', '--exact']
            run = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=180)
            (args.out / f'{name}.log').write_text(run.stdout)
            killed = run.returncode == 101 and f'test {test} ... FAILED' in run.stdout and 'panicked at' in run.stdout and '1 failed' in run.stdout
            results.append({'mutation': name, 'test': test, 'killed': killed, 'returncode': run.returncode})
            print(json.dumps(results[-1]), flush=True)
    finally:
        path.write_text(source)
    (args.out / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
    if not all(row['killed'] for row in results):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
