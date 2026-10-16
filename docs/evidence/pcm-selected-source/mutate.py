#!/usr/bin/env python3
"""Run targeted semantic mutations; restores source even on failure.

Run from repository root with CARGO_TARGET_DIR set. Each mutation must fail an
assertion, not compilation. Requires cargo +1.99.0; never measures CPU time.
"""
import json
import pathlib
import subprocess
import sys

rx = pathlib.Path('esp-periph/src/i2s/rx.rs')
sources = pathlib.Path('esp-periph/src/i2s/sources.rs')
original = {p: p.read_text() for p in (rx, sources)}
query_test = 'selected_source_query_tracks_ports_routes_stop_reset_and_legacy'
cases = [
    ('omit-selected-slot', rx, 'rx.selected_source = id + 1', 'rx.selected_source = 0', query_test),
    ('stale-legacy-or-unmatched', rx, 'if let Some(rx) = &mut self.rx { rx.selected_source = 0; }', '', query_test),
    ('ignore-stopped-rx', rx, '.filter(|_| self.rx_running())', '', query_test),
    ('retain-peripheral-reset-slot', rx, 'rx.acc = 0;\n            rx.selected_source = 0;\n        }', 'rx.acc = 0;\n        }', query_test),
    ('retain-chip-reset-slot', rx, 'rx.acc = 0;\n            rx.selected_source = 0;\n            rx', 'rx.acc = 0;\n            rx', query_test),
    ('wrong-selected-id', rx, 'rx.selected_source = id + 1', 'rx.selected_source = 1', query_test),
    ('prefer-last-source', sources, 'self.inputs.iter().position(', 'self.inputs.iter().rposition(', 'lowest_matching_source_wins_and_unmatched_receivers_keep_controller_input'),
    ('ignore-clock-inversion', sources, 'gpio.func_out_sel[ws as usize] & output_mask', 'gpio.func_out_sel[ws as usize] & (output_mask & 0xff)', query_test),
]
results = []
try:
    for name, path, old, new, test in cases:
        assert original[path].count(old) == 1, name
        path.write_text(original[path].replace(old, new))
        run = subprocess.run(['nice', '-n', '10', 'cargo', '+1.99.0', 'test', '-j2', '--release', '-p', 'esp-periph', test, '--', '--nocapture'], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        killed = run.returncode != 0 and 'test result: FAILED' in run.stdout and 'panicked at' in run.stdout
        results.append(dict(mutation=name, test=test, exit_code=run.returncode, killed=killed))
        print(json.dumps(results[-1]), flush=True)
        path.write_text(original[path])
finally:
    for path, text in original.items():
        path.write_text(text)
if len(sys.argv) > 1:
    pathlib.Path(sys.argv[1]).write_text(json.dumps(results, indent=2)+'\n')
sys.exit(0 if all(r['killed'] for r in results) else 1)
