#!/usr/bin/env python3
"""Fresh-frame semantic mutations; only assertion failures count as kills."""
import json, os, re, subprocess, sys
from pathlib import Path
root, out = (Path(p).resolve() for p in sys.argv[1:3])
out.mkdir(parents=True, exist_ok=True)
cam = 'esp32/src/camera_dma.rs'
board = 'esp32/src/board/aithinker.rs'
rows = [
    ('exclusive boundary', cam, 'capture.armed_at - at <= ARM_WINDOW', 'capture.armed_at - at < ARM_WINDOW'),
    ('window disabled', cam, 'capture.armed_at - at <= ARM_WINDOW', 'capture.armed_at - at <= 0'),
    ('unbounded window', cam, 'capture.armed_at - at <= ARM_WINDOW', 'capture.armed_at - at <= u64::MAX'),
    ('delivery timed from start', cam, 'self.cycles.saturating_sub(capture.frame_at?)', 'self.cycles.saturating_sub(capture.frame_at?.max(capture.armed_at))'),
    ('forced low counted as start', board, 'self.next = None;\n            self.frame_start = None;', 'self.next = None;\n            self.frame_start = Some(cycle);'),
    ('frame claim replayed', board, 'self.frame_start.take()', 'self.frame_start'),
    ('route loss ignored', board, 'fn invalidate_camera_frame_start(&mut self) { self.frame_start = None; }', 'fn invalidate_camera_frame_start(&mut self) {}'),
    ('unqualified start aborts early', cam, 'self.periph.i2s[0].camera.as_ref().is_some_and(|c| c.qualified)', 'self.periph.i2s[0].camera.is_some()'),
    ('unrelated pad bits close window', 'esp32/src/bus.rs', '=> (7 << 12) | (1 << 9),', '=> u32::MAX,'),
]
original = {f: (root/f).read_text() for _, f, _, _ in rows}
cmd = ['nice', '-n', '10', 'cargo', '+1.99.0', 'test', '--release', '-p', 'esp32', '--test', 'aithinker', 'c7::arming::', '--', '--nocapture']
results = []
try:
    for name, file, old, new in [('baseline', cam, None, None)] + rows:
        if old:
            assert new is not None
            assert old in original[file], name
            (root/file).write_text(original[file].replace(old, new))
        run = subprocess.run(cmd, cwd=root, env=os.environ, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        failed = re.findall(r'^test (\S+) \.\.\. FAILED', run.stdout, re.M)
        killed = run.returncode != 0 and bool(failed) and 'assertion' in run.stdout and 'error[E' not in run.stdout
        results.append(dict(name=name, file=file, before=old, after=new, exit=run.returncode, killed=killed, failed_tests=failed))
        (out/f'{len(results)-1:02}.log').write_text(run.stdout)
        (out/'mutations.json').write_text(json.dumps(results, indent=2)+'\n')
        print(name, run.returncode, failed, flush=True)
        (root/file).write_text(original[file])
        assert killed if old else run.returncode == 0, run.stdout
finally:
    for file, text in original.items():
        (root/file).write_text(text)
