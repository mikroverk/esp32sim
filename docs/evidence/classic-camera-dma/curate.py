#!/usr/bin/env python3
"""Compact historical receipts, without reinterpreting a kill as an assertion kill.

Usage: python3 curate.py INPUT.json OUTPUT.json
Writes summaries only; run capture/build/mutation harnesses in disposable trees
with raw outputs under target/, never directly into these curated receipts.
The archive manifests identify the complete original input for each summary.
"""
import collections
import json
from pathlib import Path
import sys


def mutation_rows(rows):
    groups = {}
    commands = []
    for row in rows:
        # Replacement text and per-mutant source paths remain in the archive and
        # runnable harnesses. Keep every name and outcome, including baseline;
        # failed-test arrays are represented by counts, not repeated names.
        status = {k: v for k, v in row.items() if k not in {
            'name', 'mutation', 'old', 'new', 'replace', 'with_text',
            'replace_first', 'before', 'after', 'file', 'path', 'command'}}
        if 'failed_tests' in status:
            status['failed_test_count'] = len(status.pop('failed_tests'))
        if 'command' in row:
            command = row['command']
            if command not in commands:
                commands.append(command)
            status['command_index'] = commands.index(command)
        key = json.dumps(status, sort_keys=True)
        if key not in groups:
            groups[key] = dict(outcome=status, names=[])
        groups[key]['names'].append(row.get('mutation', row.get('name')))
    counts = {}
    for field in ['killed', 'assertion', 'assertion_killed', 'runtime_semantic_killed']:
        if any(field in r for r in rows):
            counts[field] = dict(collections.Counter(str(r.get(field, 'unrecorded')).lower() for r in rows))
    result = dict(recorded_rows=len(rows), outcome_counts=counts,
                  groups=[dict(count=len(g['names']), **g) for g in groups.values()])
    if commands:
        result['commands'] = commands
    return result


# These are instruction captures, not measured cycle counts or semantic results.
CAPTURES = {'diff', 'examples', 'camera_call_windows', 'base_guard_window',
            'candidate_guard_window', 'base_call_window', 'candidate_call_window'}


def compact(value):
    if isinstance(value, list):
        return [compact(v) for v in value]
    if not isinstance(value, dict):
        return value
    out = {}
    for key, item in value.items():
        if key in CAPTURES:
            out[key + '_omitted_entries'] = len(item)
        elif key == 'results' and isinstance(item, list) and item and all(
                isinstance(r, dict) and 'passed' in r for r in item):
            out['result_batches'] = len(item)
            out['result_totals'] = {f: sum(r.get(f, 0) for r in item)
                                    for f in sorted(set().union(*(r.keys() for r in item)))}
        else:
            out[key] = compact(item)
    return out


def curate(value, filename):
    if 'mutation' in filename:
        if isinstance(value, list):
            return mutation_rows(value)
        if isinstance(value, dict) and 'results' in value:
            return {**{k: v for k, v in value.items() if k != 'results'},
                    **mutation_rows(value['results'])}
        if isinstance(value, dict):
            return {k: mutation_rows(v) if isinstance(v, list) else v for k, v in value.items()}
    if isinstance(value, dict) and isinstance(value.get('rows'), list) and value['rows'] and all(
            'same' in row for row in value['rows']):
        # Equal-body inventory stays archived; preserve selected/identical counts,
        # instruction totals per binary, and every differing body's counts.
        groups = {}
        for row in value['rows']:
            label = row.get('binary', 'selected')
            group = groups.setdefault(label, dict(compared=0, identical=0,
                                                 base_instructions=0, candidate_instructions=0))
            group['compared'] += 1
            group['identical'] += int(row['same'])
            group['base_instructions'] += row['base_instructions']
            group['candidate_instructions'] += row['candidate_instructions']
        metadata = compact({k: v for k, v in value.items() if k != 'rows'})
        assert isinstance(metadata, dict)
        return {**metadata, 'body_totals': groups,
                'different_bodies': compact([row for row in value['rows'] if not row['same']])}
    return compact(value)


if __name__ == '__main__':
    source, destination = map(Path, sys.argv[1:3])
    destination.write_text(json.dumps(curate(json.loads(source.read_text()), source.name),
                                      separators=(',', ':'), ensure_ascii=False) + '\n')
