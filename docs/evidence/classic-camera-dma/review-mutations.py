#!/usr/bin/env python3
"""Compiled assertion-only negative controls for the PR243 review fixes.
Usage: python3 review-mutations.py TREE OUTPUT
Run without concurrent builds in TREE; CARGO_TARGET_DIR/ESP32SIM_ROM_DIR are explicit.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

root, out = map(lambda p: Path(p).resolve(), sys.argv[1:3])
out.mkdir(parents=True, exist_ok=True)
board = 'esp32/src/board/aithinker.rs'
cli = 'cli/src/lib.rs'
rows = [
    ('missing unsupported-output notice', board, 'if !self.unsupported_reported {', 'if false {', 'esp32', 'aithinker', 'unsupported_sensor_output_warns_once'),
    ('notice repeated on every query', board, 'self.unsupported_reported = true;', 'self.unsupported_reported = false;', 'esp32', 'aithinker', 'unsupported_sensor_output_warns_once'),
    ('classic CLI image refused', cli, 'for (flag, on) in [("--psram-mb", o.psram_mb.is_some()),', 'for (flag, on) in [("--cam-image", o.cam_image.is_some()), ("--psram-mb", o.psram_mb.is_some()),', 'esp32sim', 'classic_camera', 'classic_camera_image_captures_nonzero_frame'),
    ('image never attached', cli, 'if let Some(p) = path {', 'if let Some(p) = None::<&str> {', 'esp32sim', 'classic_camera', 'classic_camera_image_captures_nonzero_frame'),
    ('camera missing from usage', cli, '(none, bare, esp32dev, ttgo-tdisplay, ai-thinker-cam)', '(none, bare, esp32dev, ttgo-tdisplay)', 'esp32sim', 'classic_camera', 'classic_board_usage_lists_camera'),
    ('inverted frame not reused', 'esp32/src/camera_dma.rs', '*old_mask == mask &&', 'false && *old_mask == mask &&', 'esp32', 'aithinker', 'c7::inverted_route_reuses_capture_cache_and_invalidates_source_and_mask'),
    ('inverted mask stays stale', 'esp32/src/camera_dma.rs', '*old_mask == mask &&', 'true &&', 'esp32', 'aithinker', 'c7::inverted_route_reuses_capture_cache_and_invalidates_source_and_mask'),
    ('inverted source stays stale', 'esp32/src/camera_dma.rs', 'Arc::ptr_eq(old, &source)', 'true', 'esp32', 'aithinker', 'c7::inverted_route_reuses_capture_cache_and_invalidates_source_and_mask'),
]
selected = set(sys.argv[3:])
if selected:
    rows = [row for row in rows if row[0] in selected]
    assert len(rows) == len(selected), 'unknown mutation name'
original = {f: (root/f).read_text() for _,f,*_ in rows}
results = []
try:
    for name, file, before, after, package, test, case in rows:
        assert original[file].count(before) == 1, (name, before)
        cmd = ['nice','-n','10','cargo','+1.99.0','test','--release','-j2','-p',package,'--test',test,case,'--','--exact','--include-ignored','--nocapture']
        for baseline in [True, False]:
            (root/file).write_text(original[file] if baseline else original[file].replace(before, after, 1))
            run = subprocess.run(cmd, cwd=root, env=os.environ, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=300)
            # assert!(status, "{stdout}...") emits only its custom message;
            # require this fixture's panic marker and reject unwrap failures.
            assertion = 'assertion' in run.stdout or (
                'panicked at cli/tests/classic_camera.rs:' in run.stdout
                and 'called `Result::unwrap()`' not in run.stdout
                and 'called `Option::unwrap()`' not in run.stdout)
            killed = run.returncode != 0 and f'test {case} ... FAILED' in run.stdout and assertion and 'error[E' not in run.stdout
            label = name.replace(' ', '-') + ('-baseline' if baseline else '-mutant')
            (out/(label+'.log')).write_text(run.stdout)
            results.append(dict(name=name,baseline=baseline,command=cmd,exit=run.returncode,assertion_killed=killed))
            (out/'results.json').write_text(json.dumps(results,indent=2)+'\n')
            print(label,run.returncode,killed,flush=True)
            assert run.returncode == 0 if baseline else killed, run.stdout[-4000:]
        (root/file).write_text(original[file])
finally:
    for file,text in original.items(): (root/file).write_text(text)
