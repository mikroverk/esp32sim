#!/usr/bin/env python3
"""Compare clean builds and committed camera inputs; emit portable hashes."""
import hashlib
import json
from pathlib import Path
import sys

a, b, committed = (Path(p) for p in sys.argv[1:4])
rows = []
for name in ['bootloader/bootloader.bin', 'partition_table/partition-table.bin', 'classic_camera.bin']:
    data = (a / name).read_bytes()
    assert data == (b / name).read_bytes(), f'different clean builds: {name}'
    target = sys.argv[4] if name == 'classic_camera.bin' and len(sys.argv) > 4 else name
    assert data == (committed / target).read_bytes(), f'committed input differs: {target}'
    rows.append(dict(file=name, bytes=len(data), sha256=hashlib.sha256(data).hexdigest(), clean_builds_equal=True, committed_equal=True))
print(json.dumps(rows, indent=2))
