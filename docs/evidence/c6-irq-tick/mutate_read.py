#!/usr/bin/env python3
"""Read-side rule removal. Use an external CARGO_TARGET_DIR; restores source on failure."""
import json
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
out = Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
bus = root / 'esp32c6/src/bus.rs'
old = 'self.irq_dirty |= addr & !3 == 0x6000_f000;'
mutations = [
    ('drop-fifo-read-invalidation', '// no read invalidation', 'next_usb_packet_irq_is_presented_after_guest_fifo_read'),
    ('miss-subword-fifo-aliases', 'self.irq_dirty |= addr == 0x6000_f000;', 'fifo_read_widths_preserve_invalidation_until_sync'),
    ('invalidate-every-read', 'self.irq_dirty = true;', 'unrelated_reads_do_not_invalidate_a_clean_irq_gate'),
    ('overwrite-pending-invalidation', 'self.irq_dirty = addr & !3 == 0x6000_f000;', 'fifo_read_widths_preserve_invalidation_until_sync'),
]
original = bus.read_text()
assert original.count(old) == 1
rows = []
for name, replacement, test in mutations:
    try:
        bus.write_text(original.replace(old, replacement))
        cmd = ['nice', '-n', '10', 'cargo', '+1.99.0', 'test', '--release', '-p', 'esp32c6', '--test', 'irq_read', test, '--', '--exact']
        p = subprocess.run(cmd, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        text = p.stdout.decode(); (out / (name + '.log')).write_text(text)
        killed = p.returncode != 0 and 'test result: FAILED' in text and f'{test} ... FAILED' in text
        rows.append(dict(mutation=name, test=test, exit=p.returncode, killed=killed))
        print(rows[-1], flush=True)
    finally:
        bus.write_text(original)
(out / 'mutations.json').write_text(json.dumps(rows, indent=2) + '\n')
assert all(r['killed'] for r in rows)
