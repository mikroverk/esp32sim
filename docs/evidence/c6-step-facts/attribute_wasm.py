#!/usr/bin/env python3
"""Attribute named wasm-dis differences by suffix-normalized body hash.

Usage: python3 attribute_wasm.py BASE.wat CANDIDATE.wat comparison.json output.json
The comparison input is the output of compare_wat.py for the same two inputs.
Suffix normalization is textual and does not prove runtime performance.
"""
import collections
import hashlib
import importlib.util
import json
import re
import sys
from pathlib import Path

base, candidate, comparison, output = sys.argv[1:]
# compare_wat.py also writes the requested literal comparison when imported.
sys.argv = ['compare_wat.py', base, candidate, comparison]
spec = importlib.util.spec_from_file_location('compare_wat', Path(__file__).with_name('compare_wat.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
a, b = module.parse(base), module.parse(candidate)
rows = json.loads(Path(comparison).read_text())['rows']

def digest(body):
    normalized = re.sub(r'(\$[^\s()]+?)_\d+(?=[\s()])', r'\1_SUFFIX', body)
    return hashlib.sha256(normalized.encode()).hexdigest()

added = [r['name'] for r in rows if r['status'] == 'added']
removed = [r['name'] for r in rows if r['status'] == 'removed']
by_hash = collections.defaultdict(list)
for name in added:
    by_hash[digest(b[name])].append(name)
pairs = {name: by_hash[digest(a[name])] for name in removed if by_hash[digest(a[name])]}
matched = {name for names in pairs.values() for name in names}
changed = {r['name']: ('suffix-only' if digest(a[r['name']]) == digest(b[r['name']]) else 'body-changed') for r in rows if r['status'] == 'changed'}
result = {'method': 'SHA-256 of type-resolved wasm-dis body text after normalizing numeric symbol suffixes; same hashes may map multiple duplicate bodies', 'renamed_body_matches': pairs, 'unmatched_removed': [n for n in removed if n not in pairs], 'unmatched_added': [n for n in added if n not in matched], 'changed': changed}
Path(output).write_text(json.dumps(result, indent=2) + '\n')
print('matched removed', len(pairs), 'matched added', len(matched), 'unmatched', len(result['unmatched_removed']), len(result['unmatched_added']))
print('changed', dict(collections.Counter(changed.values())))
