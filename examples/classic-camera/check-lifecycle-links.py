#!/usr/bin/env python3
"""Retain link-map archive inventory, not private build paths or full maps."""
import json
from pathlib import Path
import re
import sys
root, build, output = map(Path, sys.argv[1:4])
old = json.loads((root/'docs/evidence/classic-camera-dma/fixture-linked-archives.json').read_text())
result = {}
for name, expected in old.items():
    header = (build/name).read_text().split('Discarded input sections')[0]
    archives = []
    for archive in re.findall(r'^(\S+\.a)\(', header, re.M):
        normalized = archive if archive.startswith('esp-idf/') else Path(archive).name
        if normalized not in archives: archives.append(normalized)
    assert set(archives) == set(expected), (name, set(archives)^set(expected))
    result[name] = archives
output.write_text(json.dumps(result, indent=2)+'\n')
print({name: len(archives) for name, archives in result.items()})
